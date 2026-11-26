//! Minimal live GPUI & Bevy frame-time HUD (`F3` / optional status bar icon).
//!
//! Collects lock-free GPUI foreground journal intervals (`Window::draw`, `Window::present`,
//! task polls, input dispatches, actions, dirty→present latency) alongside per-stage
//! Bevy planetarium timings (`gpu_sync`, `commands`, `camera/grid`, `celestial`, `hips`,
//! `render`, `present_copy`, `overlay`) and exports full diagnostics on clipboard copy.

use std::{
    collections::VecDeque,
    fmt::Write as _,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardItem, Context, FocusHandle, Focusable, Hsla,
    IntoElement, MouseButton, Pixels, Render, Task, Window, div, fill, point, prelude::*, px,
    rgb,
    profiler::journal::{
        ForegroundEvent, ForegroundJournalCollector, FrameSnapshot, IntervalBoundary, IntervalSealer,
    },
};
use jiff::Zoned;
use planetarium::{BevyFrameSample, metrics::duration_to_us};
use settings::{GlobalSettings, ToggleFrameProfiler};
use ui::{
    ActiveTheme, ButtonCommon, ButtonStyle, Clickable, IconButton, IconButtonShape, IconName,
    IconSize, StyledTypography, Tooltip, h_flex, v_flex,
};
use workspace::workbench::Workbench;

const HISTORY_CAPACITY: usize = 180;
const SPARKLINE_BARS: usize = 80;
const SPARKLINE_CEILING_MS: f32 = 33.33;
const SPARKLINE_WIDTH_PX: f32 = 112.0;
const SPARKLINE_HEIGHT_PX: f32 = 16.0;
const MAX_SPIKES: usize = 32;
const SPIKE_THRESHOLD_US: u32 = 16_667;
const LATENCY_SPIKE_THRESHOLD_US: u32 = 25_000;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------------------------
// Pure Diagnostic Model
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockerKind {
    TaskPoll,
    Input,
    Action,
    SmallPolls,
}

impl BlockerKind {
    fn tag(self) -> &'static str {
        match self {
            Self::TaskPoll => "Task",
            Self::Input => "Input",
            Self::Action => "Action",
            Self::SmallPolls => "SmallPolls",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockerSummary {
    pub kind: BlockerKind,
    pub label: String,
    pub duration_us: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct GpuiFrameSample {
    pub draw_us: u32,
    pub present_us: u32,
    pub fg_other_us: u32,
    pub total_us: u32,
    pub dirty_to_present_us: Option<u32>,
    pub input_to_present_us: Option<u32>,
    pub top_blocker: Option<BlockerSummary>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameStats {
    pub count: usize,
    pub cur_us: u32,
    pub avg_us: u32,
    pub p50_us: u32,
    pub p95_us: u32,
    pub p99_us: u32,
    pub max_us: u32,
    pub over_16ms: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpikeSource {
    Gpui,
    FgStall,
    Bevy,
}

impl SpikeSource {
    fn badge(self) -> &'static str {
        match self {
            Self::Gpui => "GPUI",
            Self::FgStall => "FG",
            Self::Bevy => "BEVY",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpikeRecord {
    pub timestamp: String,
    pub source: SpikeSource,
    pub total_us: u32,
    pub detail: String,
}

fn nearest_rank_percentile(sorted: &[u32], pct: usize) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() * pct).div_ceil(100))
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[idx]
}

pub(crate) fn compute_frame_stats(durations_us: &[u32]) -> FrameStats {
    if durations_us.is_empty() {
        return FrameStats::default();
    }

    let count = durations_us.len();
    let cur_us = *durations_us.last().unwrap_or(&0);
    let sum_us: u64 = durations_us.iter().map(|&d| u64::from(d)).sum();
    let avg_us = (sum_us / count as u64).min(u64::from(u32::MAX)) as u32;

    let mut sorted = durations_us.to_vec();
    sorted.sort_unstable();
    let p50_us = nearest_rank_percentile(&sorted, 50);
    let p95_us = nearest_rank_percentile(&sorted, 95);
    let p99_us = nearest_rank_percentile(&sorted, 99);
    let max_us = *sorted.last().unwrap_or(&0);

    let over_16ms = durations_us
        .iter()
        .filter(|&&d| d > SPIKE_THRESHOLD_US)
        .count();

    FrameStats {
        count,
        cur_us,
        avg_us,
        p50_us,
        p95_us,
        p99_us,
        max_us,
        over_16ms,
    }
}

pub(crate) fn shorten_location(file: &str, line: u32) -> String {
    let normalized = file.replace('\\', "/");
    let trimmed = if let Some(idx) = normalized.find("/crates/") {
        &normalized[idx + 8..]
    } else if let Some(idx) = normalized.find("crates/") {
        &normalized[idx + 7..]
    } else {
        let mut slash_positions = normalized.rmatch_indices('/');
        let _ = slash_positions.next();
        let _ = slash_positions.next();
        if let Some((idx, _)) = slash_positions.next() {
            &normalized[idx + 1..]
        } else {
            normalized.as_str()
        }
    };
    format!("{trimmed}:{line}")
}

fn find_top_blocker(snapshot: &FrameSnapshot) -> Option<BlockerSummary> {
    let mut best: Option<BlockerSummary> = None;

    for event in &snapshot.events {
        let candidate = match event {
            ForegroundEvent::TaskPoll(timing) => {
                let dur = duration_to_us(timing.poll_duration());
                BlockerSummary {
                    kind: BlockerKind::TaskPoll,
                    label: shorten_location(timing.location.file(), timing.location.line()),
                    duration_us: dur,
                }
            }
            ForegroundEvent::Input(timing) => {
                let dur = duration_to_us(timing.end.saturating_duration_since(timing.start));
                BlockerSummary {
                    kind: BlockerKind::Input,
                    label: timing.kind.to_string(),
                    duration_us: dur,
                }
            }
            ForegroundEvent::Action(timing) => {
                let dur = duration_to_us(timing.duration());
                BlockerSummary {
                    kind: BlockerKind::Action,
                    label: timing.name.to_string(),
                    duration_us: dur,
                }
            }
            ForegroundEvent::Draw(_)
            | ForegroundEvent::Present(_)
            | ForegroundEvent::SmallPolls(_) => continue,
        };

        if best
            .as_ref()
            .is_none_or(|cur| candidate.duration_us > cur.duration_us)
        {
            best = Some(candidate);
        }
    }

    let small = snapshot.small_poll_summary();
    let small_us = duration_to_us(small.total);
    if small_us >= 1_000
        && best
            .as_ref()
            .is_none_or(|cur| small_us > cur.duration_us)
    {
        best = Some(BlockerSummary {
            kind: BlockerKind::SmallPolls,
            label: format!("{} tiny polls", small.count),
            duration_us: small_us,
        });
    }

    best
}

pub(crate) fn extract_gpui_sample(snapshot: &FrameSnapshot) -> Option<GpuiFrameSample> {
    let IntervalBoundary::Presented(presented) = snapshot.boundary else {
        return None;
    };

    let draw_us = duration_to_us(presented.frame.draw_duration());
    let present_us = duration_to_us(presented.presentation.present_duration());
    let occupancy_us = duration_to_us(snapshot.occupancy());
    let fg_other_us = occupancy_us.saturating_sub(draw_us.saturating_add(present_us));
    let total_us = draw_us
        .saturating_add(present_us)
        .saturating_add(fg_other_us);

    let finished_at = presented.presentation.present_end;
    let dirty_to_present_us = presented.dirty_to_present_duration().map(duration_to_us);
    let earliest_invalidating_input = snapshot
        .events
        .iter()
        .filter_map(|event| match event {
            ForegroundEvent::Input(inp) if inp.caused_invalidation => Some(inp.start),
            _ => None,
        })
        .min();
    let input_to_present_us = earliest_invalidating_input
        .map(|start| duration_to_us(finished_at.saturating_duration_since(start)));

    Some(GpuiFrameSample {
        draw_us,
        present_us,
        fg_other_us,
        total_us,
        dirty_to_present_us,
        input_to_present_us,
        top_blocker: find_top_blocker(snapshot),
    })
}

fn format_ms_compact(us: u32) -> String {
    let ms = f64::from(us) / 1000.0;
    if ms >= 100.0 {
        format!("{ms:.0}ms")
    } else if ms >= 10.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.2}ms")
    }
}

fn frame_time_color(us: u32) -> Hsla {
    if us <= 10_000 {
        rgb(0x4ade80).into()
    } else if us <= SPIKE_THRESHOLD_US {
        rgb(0xfacc15).into()
    } else if us <= 33_333 {
        rgb(0xfb923c).into()
    } else {
        rgb(0xf87171).into()
    }
}

pub(crate) fn describe_gpui_spike(sample: &GpuiFrameSample) -> String {
    let mut text = format!(
        "draw {:.1} + pres {:.1} + fg {:.1}ms",
        f64::from(sample.draw_us) / 1000.0,
        f64::from(sample.present_us) / 1000.0,
        f64::from(sample.fg_other_us) / 1000.0,
    );
    if let Some(lat) = sample.dirty_to_present_us {
        if lat > LATENCY_SPIKE_THRESHOLD_US {
            let _ = write!(text, " (lat {:.1}ms)", f64::from(lat) / 1000.0);
        }
    }
    if let Some(inp_lat) = sample.input_to_present_us {
        if inp_lat > LATENCY_SPIKE_THRESHOLD_US {
            let _ = write!(text, " (in->pres {:.1}ms)", f64::from(inp_lat) / 1000.0);
        }
    }
    if let Some(blocker) = &sample.top_blocker {
        if blocker.duration_us >= 1_000 {
            let _ = write!(
                text,
                " — {} {} ({:.1}ms)",
                blocker.kind.tag(),
                blocker.label,
                f64::from(blocker.duration_us) / 1000.0
            );
        }
    }
    text
}

pub(crate) fn describe_bevy_spike(sample: &BevyFrameSample) -> String {
    let mut parts: Vec<String> = Vec::new();
    let stages = [
        ("sync", sample.gpu_sync_us),
        ("cmd", sample.commands_us),
        ("cam", sample.camera_grid_us),
        ("cel", sample.celestial_us),
        ("hips", sample.hips_us),
        ("render", sample.render_us),
        ("copy", sample.present_copy_us),
        ("mount", sample.overlay_us),
    ];
    let mut ranked: Vec<(&str, u32)> = stages.into_iter().filter(|(_, us)| *us >= 500).collect();
    ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (name, us) in ranked.into_iter().take(4) {
        parts.push(format!("{name} {:.1}", f64::from(us) / 1000.0));
    }
    let breakdown = if parts.is_empty() {
        format!("work {:.1}ms", f64::from(sample.work_us()) / 1000.0)
    } else {
        format!("{}ms", parts.join(" + "))
    };
    let mode = if sample.is_mount_mode { " Mount" } else { "" };
    let resize = if sample.resized { " resize" } else { "" };
    format!(
        "{breakdown} [{}×{}{mode}{resize}]",
        sample.width, sample.height
    )
}

#[derive(Default)]
pub(crate) struct ProfilerHistory {
    pub gpui_frames: VecDeque<GpuiFrameSample>,
    pub bevy_frames: VecDeque<BevyFrameSample>,
    pub spikes: VecDeque<SpikeRecord>,
    pub total_gpui_frames: u64,
    pub total_bevy_frames: u64,
}

impl ProfilerHistory {
    pub fn reset(&mut self) {
        self.gpui_frames.clear();
        self.bevy_frames.clear();
        self.spikes.clear();
        self.total_gpui_frames = 0;
        self.total_bevy_frames = 0;
    }

    fn push_spike(&mut self, spike: SpikeRecord) {
        if self.spikes.len() >= MAX_SPIKES {
            self.spikes.pop_front();
        }
        self.spikes.push_back(spike);
    }

    pub fn push_gpui_snapshot(&mut self, snapshot: &FrameSnapshot) {
        let now_stamp = || Zoned::now().strftime("%H:%M:%S").to_string();
        match snapshot.boundary {
            IntervalBoundary::Presented(_) => {
                let Some(sample) = extract_gpui_sample(snapshot) else {
                    return;
                };
                self.total_gpui_frames += 1;

                let is_spike = sample.total_us > SPIKE_THRESHOLD_US
                    || sample
                        .dirty_to_present_us
                        .is_some_and(|lat| lat > LATENCY_SPIKE_THRESHOLD_US)
                    || sample
                        .input_to_present_us
                        .is_some_and(|lat| lat > LATENCY_SPIKE_THRESHOLD_US);
                if is_spike {
                    let spike_us = sample
                        .input_to_present_us
                        .or(sample.dirty_to_present_us)
                        .filter(|&lat| lat > sample.total_us && sample.total_us <= SPIKE_THRESHOLD_US)
                        .unwrap_or(sample.total_us);
                    let detail = describe_gpui_spike(&sample);
                    self.push_spike(SpikeRecord {
                        timestamp: now_stamp(),
                        source: SpikeSource::Gpui,
                        total_us: spike_us,
                        detail,
                    });
                }

                if self.gpui_frames.len() >= HISTORY_CAPACITY {
                    self.gpui_frames.pop_front();
                }
                self.gpui_frames.push_back(sample);
            }
            IntervalBoundary::Idle { .. } => {
                let occ_us = duration_to_us(snapshot.occupancy());
                if occ_us >= SPIKE_THRESHOLD_US {
                    let detail = match find_top_blocker(snapshot) {
                        Some(blocker) => format!(
                            "fg stall {:.1}ms — {} {} ({:.1}ms)",
                            f64::from(occ_us) / 1000.0,
                            blocker.kind.tag(),
                            blocker.label,
                            f64::from(blocker.duration_us) / 1000.0
                        ),
                        None => format!("fg stall {:.1}ms without frame", f64::from(occ_us) / 1000.0),
                    };
                    self.push_spike(SpikeRecord {
                        timestamp: now_stamp(),
                        source: SpikeSource::FgStall,
                        total_us: occ_us,
                        detail,
                    });
                }
            }
        }
    }

    pub fn push_bevy_sample(&mut self, sample: BevyFrameSample) {
        self.total_bevy_frames += 1;
        if sample.total_us > SPIKE_THRESHOLD_US {
            let detail = describe_bevy_spike(&sample);
            self.push_spike(SpikeRecord {
                timestamp: Zoned::now().strftime("%H:%M:%S").to_string(),
                source: SpikeSource::Bevy,
                total_us: sample.total_us,
                detail,
            });
        }
        if self.bevy_frames.len() >= HISTORY_CAPACITY {
            self.bevy_frames.pop_front();
        }
        self.bevy_frames.push_back(sample);
    }

    pub fn gpui_stats(&self) -> FrameStats {
        let durations: Vec<u32> = self.gpui_frames.iter().map(|s| s.total_us).collect();
        compute_frame_stats(&durations)
    }

    pub fn bevy_stats(&self) -> FrameStats {
        let durations: Vec<u32> = self.bevy_frames.iter().map(|s| s.total_us).collect();
        compute_frame_stats(&durations)
    }

    pub fn gpui_phase_p95s(&self) -> (u32, u32, u32, u32, u32) {
        let mut draw: Vec<u32> = self.gpui_frames.iter().map(|s| s.draw_us).collect();
        let mut present: Vec<u32> = self.gpui_frames.iter().map(|s| s.present_us).collect();
        let mut fg: Vec<u32> = self.gpui_frames.iter().map(|s| s.fg_other_us).collect();
        let mut lat: Vec<u32> = self
            .gpui_frames
            .iter()
            .filter_map(|s| s.dirty_to_present_us)
            .collect();
        let mut inp_lat: Vec<u32> = self
            .gpui_frames
            .iter()
            .filter_map(|s| s.input_to_present_us)
            .collect();
        draw.sort_unstable();
        present.sort_unstable();
        fg.sort_unstable();
        lat.sort_unstable();
        inp_lat.sort_unstable();
        (
            nearest_rank_percentile(&draw, 95),
            nearest_rank_percentile(&present, 95),
            nearest_rank_percentile(&fg, 95),
            nearest_rank_percentile(&lat, 95),
            nearest_rank_percentile(&inp_lat, 95),
        )
    }

    pub fn bevy_phase_p95s(&self) -> (u32, u32, u32, u32, u32, u32, u32) {
        let mut sync: Vec<u32> = self.bevy_frames.iter().map(|s| s.gpu_sync_us).collect();
        let mut cam: Vec<u32> = self
            .bevy_frames
            .iter()
            .map(|s| s.commands_us.saturating_add(s.camera_grid_us))
            .collect();
        let mut cel: Vec<u32> = self.bevy_frames.iter().map(|s| s.celestial_us).collect();
        let mut hips: Vec<u32> = self.bevy_frames.iter().map(|s| s.hips_us).collect();
        let mut render: Vec<u32> = self.bevy_frames.iter().map(|s| s.render_us).collect();
        let mut copy: Vec<u32> = self.bevy_frames.iter().map(|s| s.present_copy_us).collect();
        let mut overlay: Vec<u32> = self.bevy_frames.iter().map(|s| s.overlay_us).collect();
        for v in [
            &mut sync,
            &mut cam,
            &mut cel,
            &mut hips,
            &mut render,
            &mut copy,
            &mut overlay,
        ] {
            v.sort_unstable();
        }
        (
            nearest_rank_percentile(&sync, 95),
            nearest_rank_percentile(&cam, 95),
            nearest_rank_percentile(&cel, 95),
            nearest_rank_percentile(&hips, 95),
            nearest_rank_percentile(&render, 95),
            nearest_rank_percentile(&copy, 95),
            nearest_rank_percentile(&overlay, 95),
        )
    }

    pub fn format_report(&self) -> String {
        let gpui = self.gpui_stats();
        let bevy = self.bevy_stats();
        let (gd_p95, gp_p95, gf_p95, gl_p95, gi_p95) = self.gpui_phase_p95s();
        let (bsync, bcam, bcel, bhips, brend, bcopy, bovl) = self.bevy_phase_p95s();

        let mut out = String::new();
        let _ = writeln!(
            out,
            "Apogee Frame Time Report ({})",
            Zoned::now().strftime("%Y-%m-%d %H:%M:%S")
        );
        let _ = writeln!(
            out,
            "GPUI ({} window frames, {} total): cur={} avg={} p50={} p95={} p99={} max={} >16.7ms={}",
            gpui.count,
            self.total_gpui_frames,
            format_ms_compact(gpui.cur_us),
            format_ms_compact(gpui.avg_us),
            format_ms_compact(gpui.p50_us),
            format_ms_compact(gpui.p95_us),
            format_ms_compact(gpui.p99_us),
            format_ms_compact(gpui.max_us),
            gpui.over_16ms,
        );
        let _ = writeln!(
            out,
            "  GPUI p95 phases: draw={} present={} fg={} dirty->present={} input->present={}",
            format_ms_compact(gd_p95),
            format_ms_compact(gp_p95),
            format_ms_compact(gf_p95),
            format_ms_compact(gl_p95),
            format_ms_compact(gi_p95),
        );

        let res_note = self.bevy_frames.back().map_or_else(String::new, |s| {
            format!(
                " res={}x{}{}",
                s.width,
                s.height,
                if s.is_mount_mode { " mount" } else { "" }
            )
        });
        let _ = writeln!(
            out,
            "Bevy ({} window frames, {} total): cur={} avg={} p50={} p95={} p99={} max={} >16.7ms={}{res_note}",
            bevy.count,
            self.total_bevy_frames,
            format_ms_compact(bevy.cur_us),
            format_ms_compact(bevy.avg_us),
            format_ms_compact(bevy.p50_us),
            format_ms_compact(bevy.p95_us),
            format_ms_compact(bevy.p99_us),
            format_ms_compact(bevy.max_us),
            bevy.over_16ms,
        );
        let _ = writeln!(
            out,
            "  Bevy p95 phases: sync={} cam/grid={} celestial={} hips={} render={} copy={} mount={}",
            format_ms_compact(bsync),
            format_ms_compact(bcam),
            format_ms_compact(bcel),
            format_ms_compact(bhips),
            format_ms_compact(brend),
            format_ms_compact(bcopy),
            format_ms_compact(bovl),
        );

        if self.spikes.is_empty() {
            let _ = writeln!(out, "Spikes (>16.7ms): none");
        } else {
            let _ = writeln!(out, "Recent Spikes (latest {}):", self.spikes.len());
            for spike in self.spikes.iter().rev() {
                let _ = writeln!(
                    out,
                    "  {} [{}] {} — {}",
                    spike.timestamp,
                    spike.source.badge(),
                    format_ms_compact(spike.total_us),
                    spike.detail
                );
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// HUD Overlay State & Lifecycle
// ---------------------------------------------------------------------------------------------

pub fn init(cx: &mut App) {
    cx.on_action(|_: &ToggleFrameProfiler, cx| {
        if let Some(settings) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings.update(cx, |model, cx| {
                model.toggle_frame_profiler_open(cx);
            });
        }
    });

    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        let Some(window) = window else {
            return;
        };
        let overlay = cx.new(|cx| FrameProfilerOverlay::new(window, cx));
        workbench.add_overlay(overlay.into(), cx);
    })
    .detach();
}

pub struct FrameProfilerOverlay {
    focus_handle: FocusHandle,
    visible: bool,
    copied_feedback_until: Option<Instant>,
    history: ProfilerHistory,
    journal_collector: Option<ForegroundJournalCollector>,
    sealer: Option<IntervalSealer>,
    bevy_cursor: u64,
    bevy_scratch: Vec<BevyFrameSample>,
    poll_task: Option<Task<()>>,
    awaiting_self_repaint: bool,
    last_self_notify_at: Option<Instant>,
}

impl Focusable for FrameProfilerOverlay {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FrameProfilerOverlay {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_visible = cx
            .try_global::<GlobalSettings>()
            .is_some_and(|g| g.0.read(cx).is_frame_profiler_open());

        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            cx.observe(&settings_entity, |this: &mut Self, model, cx| {
                let next_visible = model.read(cx).is_frame_profiler_open();
                this.set_visible(next_visible, cx);
            })
            .detach();
        }

        let mut overlay = Self {
            focus_handle: cx.focus_handle(),
            visible: false,
            copied_feedback_until: None,
            history: ProfilerHistory::default(),
            journal_collector: None,
            sealer: None,
            bevy_cursor: 0,
            bevy_scratch: Vec::with_capacity(32),
            poll_task: None,
            awaiting_self_repaint: false,
            last_self_notify_at: None,
        };
        overlay.set_visible(initial_visible, cx);
        overlay
    }

    fn set_visible(&mut self, next_visible: bool, cx: &mut Context<Self>) {
        if self.visible == next_visible {
            return;
        }
        self.visible = next_visible;
        if next_visible {
            self.journal_collector = Some(cx.foreground_journal().collector());
            self.sealer = Some(IntervalSealer::new(Instant::now()));
            self.bevy_cursor = 0;
            self.awaiting_self_repaint = false;
            self.last_self_notify_at = None;
            self.start_poll_loop(cx);
            self.poll_metrics(cx);
        } else {
            self.poll_task = None;
            self.journal_collector = None;
            self.sealer = None;
            self.awaiting_self_repaint = false;
            self.last_self_notify_at = None;
            self.copied_feedback_until = None;
            self.history.reset();
        }
        cx.notify();
    }

    fn start_poll_loop(&mut self, cx: &mut Context<Self>) {
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let keep_going = this
                    .update(cx, |overlay, cx| {
                        if !overlay.visible {
                            overlay.poll_task = None;
                            return false;
                        }
                        overlay.poll_metrics(cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn close(&self, cx: &mut Context<Self>) {
        if let Some(settings) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings.update(cx, |model, cx| {
                model.set_frame_profiler_open(false, cx);
            });
        }
    }

    fn poll_metrics(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }

        let now = Instant::now();
        self.bevy_scratch.clear();
        if let Some(bridge) = cx
            .try_global::<mount_viewer::SharedPlanetariumBridge>()
            .map(|g| g.0.clone())
        {
            bridge.collect_frame_samples(&mut self.bevy_cursor, &mut self.bevy_scratch);
        }

        let mut gpui_snapshots: Vec<FrameSnapshot> = Vec::new();
        if let (Some(collector), Some(sealer)) =
            (self.journal_collector.as_mut(), self.sealer.as_mut())
        {
            let drained = collector.collect_unseen();
            if !drained.entries.is_empty() {
                gpui_snapshots = sealer.push_entries(drained.entries);
            }
        }

        // Expire stale self-repaint flag if GPUI never produced a frame within 500ms.
        if self.awaiting_self_repaint
            && self
                .last_self_notify_at
                .is_some_and(|t| now.saturating_duration_since(t) > Duration::from_millis(500))
        {
            self.awaiting_self_repaint = false;
        }

        let presented_count = gpui_snapshots
            .iter()
            .filter(|s| matches!(s.boundary, IntervalBoundary::Presented(_)))
            .count();
        let has_idle_stall = gpui_snapshots.iter().any(|s| {
            matches!(s.boundary, IntervalBoundary::Idle { .. })
                && duration_to_us(s.occupancy()) >= SPIKE_THRESHOLD_US
        });

        let is_only_self_repaint = self.awaiting_self_repaint
            && self.bevy_scratch.is_empty()
            && !has_idle_stall
            && presented_count == 1
            && gpui_snapshots.iter().any(|snap| {
                let IntervalBoundary::Presented(presented) = snap.boundary else {
                    return false;
                };
                let after_notify = self
                    .last_self_notify_at
                    .is_some_and(|t| presented.frame.draw_start >= t);
                let has_user_activity = snap.events.iter().any(|ev| {
                    matches!(ev, ForegroundEvent::Input(_) | ForegroundEvent::Action(_))
                });
                let total_us = duration_to_us(snap.occupancy()).max(duration_to_us(
                    presented.frame.draw_duration() + presented.presentation.present_duration(),
                ));
                after_notify
                    && presented.frame.invalidations <= 2
                    && !has_user_activity
                    && total_us < SPIKE_THRESHOLD_US
            });

        if is_only_self_repaint {
            self.awaiting_self_repaint = false;
            return;
        }

        let mut changed = false;
        for sample in self.bevy_scratch.drain(..) {
            self.history.push_bevy_sample(sample);
            changed = true;
        }
        for snapshot in &gpui_snapshots {
            let prev_gpui_count = self.history.total_gpui_frames;
            let prev_spike_count = self.history.spikes.len();
            self.history.push_gpui_snapshot(snapshot);
            if self.history.total_gpui_frames != prev_gpui_count
                || self.history.spikes.len() != prev_spike_count
            {
                changed = true;
            }
        }

        let feedback_expired = self
            .copied_feedback_until
            .is_some_and(|until| now >= until);
        if feedback_expired {
            self.copied_feedback_until = None;
            changed = true;
        }

        if changed {
            self.awaiting_self_repaint = true;
            self.last_self_notify_at = Some(now);
            cx.notify();
        }
    }

    fn render_row(
        &self,
        label: &'static str,
        cur_us: Option<u32>,
        durations_us: Vec<u32>,
        action_button: AnyElement,
        cx: &App,
    ) -> AnyElement {
        let colors = cx.theme().colors().clone();
        let bg_color = colors.editor_background.opacity(0.6);
        let budget_16_color: Hsla = rgb(0xfacc15).alpha(0.35).into();

        let (cur_text, cur_color) = match cur_us {
            Some(us) => (format_ms_compact(us), frame_time_color(us)),
            None => ("—".to_string(), colors.text_muted),
        };

        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .w(px(34.0))
                    .text_size(px(10.5))
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .w(px(46.0))
                    .flex()
                    .justify_end()
                    .font_buffer(cx)
                    .text_size(px(11.0))
                    .text_color(cur_color)
                    .child(cur_text),
            )
            .child(
                div()
                    .w(px(SPARKLINE_WIDTH_PX))
                    .h(px(SPARKLINE_HEIGHT_PX))
                    .rounded_xs()
                    .overflow_hidden()
                    .child(
                        gpui::canvas(
                            |_bounds, _window, _cx| {},
                            move |bounds, (), window, _cx| {
                                paint_sparkline(
                                    bounds,
                                    &durations_us,
                                    bg_color,
                                    budget_16_color,
                                    window,
                                );
                            },
                        )
                        .size_full(),
                    ),
            )
            .child(action_button)
            .into_any_element()
    }
}

impl Render for FrameProfilerOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return gpui::Empty.into_any_element();
        }

        let now = Instant::now();
        let colors = cx.theme().colors().clone();
        let copied_active = self.copied_feedback_until.is_some_and(|until| now < until);

        let gpui_cur = self.history.gpui_frames.back().map(|s| s.total_us);
        let gpui_durations: Vec<u32> = self
            .history
            .gpui_frames
            .iter()
            .map(|s| s.total_us)
            .collect();
        let copy_btn = IconButton::new(
            "perf_copy_btn",
            if copied_active {
                IconName::Check
            } else {
                IconName::Copy
            },
        )
        .shape(IconButtonShape::Square)
        .icon_size(IconSize::XSmall)
        .style(ButtonStyle::Subtle)
        .tooltip(Tooltip::text("Copy Detailed Report"))
        .on_click(cx.listener(|this, _, _, cx| {
            let report = this.history.format_report();
            cx.write_to_clipboard(ClipboardItem::new_string(report));
            this.copied_feedback_until = Some(Instant::now() + Duration::from_secs(2));
            cx.notify();
        }))
        .into_any_element();

        let bevy_cur = self.history.bevy_frames.back().map(|s| s.total_us);
        let bevy_durations: Vec<u32> = self
            .history
            .bevy_frames
            .iter()
            .map(|s| s.total_us)
            .collect();
        let close_btn = IconButton::new("perf_close_btn", IconName::Close)
            .shape(IconButtonShape::Square)
            .icon_size(IconSize::XSmall)
            .style(ButtonStyle::Subtle)
            .tooltip(Tooltip::text("Close (F3)"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.close(cx);
            }))
            .into_any_element();

        let card = v_flex()
            .px_2()
            .py_1p5()
            .gap_1()
            .rounded_md()
            .bg(colors.elevated_surface_background.opacity(0.90))
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .child(self.render_row("GPUI", gpui_cur, gpui_durations, copy_btn, cx))
            .child(self.render_row("Bevy", bevy_cur, bevy_durations, close_btn, cx));

        div()
            .id("frame_profiler_hud")
            .track_focus(&self.focus_handle)
            .absolute()
            .top(px(40.0))
            .left(px(12.0))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(card)
            .into_any_element()
    }
}

fn paint_sparkline(
    bounds: Bounds<Pixels>,
    durations_us: &[u32],
    bg_color: Hsla,
    budget_16_color: Hsla,
    window: &mut Window,
) {
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    window.paint_quad(fill(bounds, bg_color));

    let y_16 = origin_y + height * (1.0 - (16.67_f32 / SPARKLINE_CEILING_MS).clamp(0.0, 1.0));
    window.paint_quad(fill(
        Bounds::new(point(px(origin_x), px(y_16)), gpui::size(px(width), px(1.0))),
        budget_16_color,
    ));

    if durations_us.is_empty() {
        return;
    }

    let slice = if durations_us.len() > SPARKLINE_BARS {
        &durations_us[durations_us.len() - SPARKLINE_BARS..]
    } else {
        durations_us
    };

    let slot_w = width / SPARKLINE_BARS as f32;
    let start_slot = SPARKLINE_BARS.saturating_sub(slice.len());
    for (idx, &us) in slice.iter().enumerate() {
        let slot = start_slot + idx;
        let x0 = origin_x + slot as f32 * slot_w;
        let x1 = origin_x + (slot + 1) as f32 * slot_w;
        let bar_w = (x1 - x0 - 0.4).max(1.0);
        let ms = us as f32 / 1000.0;
        let top_y = origin_y + height * (1.0 - (ms / SPARKLINE_CEILING_MS).clamp(0.0, 1.0));
        let bar_h = (origin_y + height - top_y).max(1.0);
        window.paint_quad(fill(
            Bounds::new(point(px(x0), px(top_y)), gpui::size(px(bar_w), px(bar_h))),
            frame_time_color(us),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_frame_stats_percentiles_and_over_budget_counts() {
        let durations: Vec<u32> = (1..=100).map(|ms| ms * 1000).collect();
        let stats = compute_frame_stats(&durations);
        assert_eq!(stats.count, 100);
        assert_eq!(stats.cur_us, 100_000);
        assert_eq!(stats.avg_us, 50_500);
        assert_eq!(stats.p50_us, 50_000);
        assert_eq!(stats.p95_us, 95_000);
        assert_eq!(stats.p99_us, 99_000);
        assert_eq!(stats.max_us, 100_000);
        assert_eq!(stats.over_16ms, 84); // 17..=100 ms

        let empty = compute_frame_stats(&[]);
        assert_eq!(empty.count, 0);
        assert_eq!(empty.cur_us, 0);
        assert_eq!(empty.max_us, 0);
    }

    #[test]
    fn shorten_location_strips_workspace_prefixes() {
        assert_eq!(
            shorten_location("/home/remy/data/dev/apogee/crates/apogee/src/bevy_viewport.rs", 622),
            "apogee/src/bevy_viewport.rs:622"
        );
        assert_eq!(
            shorten_location("crates/planetarium/src/hips.rs", 358),
            "planetarium/src/hips.rs:358"
        );
        assert_eq!(
            shorten_location("/opt/rust/vendor/foo/src/bar.rs", 42),
            "foo/src/bar.rs:42"
        );
        assert_eq!(shorten_location("main.rs", 10), "main.rs:10");
    }

    #[test]
    fn bevy_spike_detection_and_report_formatting() {
        let mut history = ProfilerHistory::default();
        let now = Instant::now();

        history.push_bevy_sample(BevyFrameSample {
            seq: 1,
            finished_at: now,
            interval_us: 16_667,
            total_us: 5_200,
            gpu_sync_us: 200,
            commands_us: 100,
            camera_grid_us: 300,
            celestial_us: 400,
            hips_us: 150,
            render_us: 3_800,
            present_copy_us: 250,
            overlay_us: 0,
            width: 1920,
            height: 1080,
            command_count: 1,
            is_mount_mode: false,
            resized: false,
        });
        assert!(history.spikes.is_empty());

        history.push_bevy_sample(BevyFrameSample {
            seq: 2,
            finished_at: now + Duration::from_millis(25),
            interval_us: 25_000,
            total_us: 28_600,
            gpu_sync_us: 18_200,
            commands_us: 80,
            camera_grid_us: 420,
            celestial_us: 500,
            hips_us: 6_100,
            render_us: 3_000,
            present_copy_us: 300,
            overlay_us: 0,
            width: 1920,
            height: 1080,
            command_count: 2,
            is_mount_mode: false,
            resized: false,
        });
        assert_eq!(history.spikes.len(), 1);
        let spike = &history.spikes[0];
        assert_eq!(spike.source, SpikeSource::Bevy);
        assert_eq!(spike.total_us, 28_600);
        assert!(spike.detail.contains("sync 18.2"));
        assert!(spike.detail.contains("hips 6.1"));
        assert!(spike.detail.contains("1920×1080"));

        let report = history.format_report();
        assert!(report.contains("Apogee Frame Time Report"));
        assert!(report.contains("GPUI (0 window frames, 0 total)"));
        assert!(report.contains("Bevy (2 window frames, 2 total)"));
        assert!(report.contains("Recent Spikes (latest 1)"));
        assert!(report.contains("[BEVY]"));
    }
}
