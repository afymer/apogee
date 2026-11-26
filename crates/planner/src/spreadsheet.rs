use std::time::Duration;

use client::GlobalDaemonState;
use components::{StepPrecision, TextField, TextFieldEvent};
use gpui::{
    Anchor, AnyElement, App, Context, Entity, FontWeight, MouseButton, Render, Subscription,
    Window, anchored, deferred, div, hsla, point, prelude::*, px,
};
use observer::GlobalObserver;
use protocol::{
    FrameType, PlannedTarget, SequencePhase, SequenceStatus, SequenceStep, StageKind,
};
use ui::{
    ActiveTheme, ButtonStyle, Color, Divider, Icon, IconButton, IconButtonShape, IconName,
    IconSize, Label, LabelSize, StyledTypography, TintColor, Tooltip, prelude::*,
};

use crate::state::{
    GlobalNightPlan, format_duration_compact, format_radec_compact, format_unix_hm,
    is_session_locked, parse_hm_to_night_unix_secs,
};

const STRIPE_W: f32 = 3.0;
const COL_GRIP_W: f32 = 28.0;
const COL_STAGE_W: f32 = 108.0;
const COL_DURATION_W: f32 = 78.0;
const COL_PROGRESS_W: f32 = 68.0;
const COL_ACTIONS_W: f32 = 56.0;
const HEADER_H: f32 = 26.0;
const ROW_H: f32 = 28.0;

const ISO_STOPS: &[u32] = &[100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600];

#[derive(Clone, Debug, PartialEq, Eq)]
enum OpenDropdown {
    AddStage { target_id: String },
    StageCell { target_id: String, step_idx: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StageChoice {
    Light,
    Dark,
    Flat,
    Bias,
    Autofocus,
    PlateSolve,
    Wait,
}

const IMAGING_CHOICES: &[StageChoice] = &[
    StageChoice::Light,
    StageChoice::Dark,
    StageChoice::Flat,
    StageChoice::Bias,
];

const RIG_CHOICES: &[StageChoice] = &[
    StageChoice::Autofocus,
    StageChoice::PlateSolve,
    StageChoice::Wait,
];

impl StageChoice {
    fn from_step(step: &SequenceStep) -> Self {
        match step.kind {
            StageKind::Capture => match step.frame_type {
                FrameType::Light => Self::Light,
                FrameType::Dark => Self::Dark,
                FrameType::Flat => Self::Flat,
                FrameType::Bias => Self::Bias,
            },
            StageKind::Autofocus => Self::Autofocus,
            StageKind::PlateSolve => Self::PlateSolve,
            StageKind::Wait => Self::Wait,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Flat => "Flat",
            Self::Bias => "Bias",
            Self::Autofocus => "Autofocus",
            Self::PlateSolve => "Plate Solve",
            Self::Wait => "Wait",
        }
    }

    fn default_step(self) -> SequenceStep {
        match self {
            Self::Light => SequenceStep::capture(
                FrameType::Light,
                10,
                Duration::from_secs(60),
                Some(800),
                Duration::ZERO,
            ),
            Self::Dark => SequenceStep::capture(
                FrameType::Dark,
                10,
                Duration::from_secs(60),
                Some(800),
                Duration::ZERO,
            ),
            Self::Flat => SequenceStep::capture(
                FrameType::Flat,
                15,
                Duration::from_secs(2),
                Some(100),
                Duration::ZERO,
            ),
            Self::Bias => SequenceStep::capture(
                FrameType::Bias,
                20,
                Duration::from_millis(10),
                Some(800),
                Duration::ZERO,
            ),
            Self::Autofocus => SequenceStep::autofocus(),
            Self::PlateSolve => SequenceStep::plate_solve(),
            Self::Wait => SequenceStep::wait(Duration::from_secs(60)),
        }
    }

    fn apply_to_step(self, step: &mut SequenceStep) {
        let prev_kind = step.kind;
        match self {
            Self::Light | Self::Dark | Self::Flat | Self::Bias => {
                let ft = match self {
                    Self::Light => FrameType::Light,
                    Self::Dark => FrameType::Dark,
                    Self::Flat => FrameType::Flat,
                    Self::Bias => FrameType::Bias,
                    _ => unreachable!(),
                };
                step.kind = StageKind::Capture;
                step.frame_type = ft;
                if prev_kind == StageKind::Wait {
                    step.delay_between_frames = Duration::ZERO;
                }
                if !prev_kind.is_capture() || step.count == 0 {
                    step.count = 10;
                }
                if step.params.exposure.is_zero() {
                    step.params.exposure = Duration::from_secs(60);
                }
            }
            Self::Autofocus => {
                step.kind = StageKind::Autofocus;
            }
            Self::PlateSolve => {
                step.kind = StageKind::PlateSolve;
            }
            Self::Wait => {
                step.kind = StageKind::Wait;
                if step.delay_between_frames.is_zero() {
                    step.delay_between_frames = Duration::from_secs(60);
                }
            }
        }
    }
}

struct StageMenuTarget {
    id_base: usize,
    anchor: Anchor,
    selected: Option<StageChoice>,
    target_id: String,
    step_idx: Option<usize>,
}

#[derive(Clone)]
struct DraggedTargetGroup {
    from_idx: usize,
    name: String,
    hue: f32,
}

#[derive(Clone)]
struct DraggedStageRow {
    target_id: String,
    from_idx: usize,
    label: String,
    hue: f32,
}

struct DragGhostView {
    label: String,
    hue: f32,
}

impl Render for DragGhostView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border_focused)
            .child(
                div()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(hsla(self.hue, 0.85, 0.62, 1.0)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(self.label.clone()),
            )
    }
}

struct StepRowFields {
    exposure: Entity<TextField>,
    iso: Entity<TextField>,
    count: Entity<TextField>,
    delay: Entity<TextField>,
    _subs: Vec<Subscription>,
}

struct TargetRowGroup {
    target_id: String,
    start_time: Entity<TextField>,
    _start_sub: Subscription,
    rows: Vec<StepRowFields>,
}

struct SpreadsheetRenderCtx {
    seq_status: Option<SequenceStatus>,
    locked: bool,
}

#[derive(Clone, Copy)]
enum StepFieldKind {
    Exposure,
    Iso,
    Count,
    Delay,
}

impl StepFieldKind {
    fn read_text(self, step: &SequenceStep) -> String {
        match self {
            Self::Exposure => format_secs(step.params.exposure),
            Self::Iso => step.params.iso.map_or_else(String::new, |v| v.to_string()),
            Self::Count => step.count.to_string(),
            Self::Delay => format_secs(step.delay_between_frames),
        }
    }

    fn apply_parsed(self, txt: &str, step: &mut SequenceStep) -> Result<(), &'static str> {
        let trimmed = txt.trim();
        match self {
            Self::Exposure => match trimmed.parse::<f64>() {
                Ok(v) if v.is_finite() && v > 0.0 && v <= 3600.0 => {
                    step.params.exposure = Duration::from_secs_f64(v);
                    Ok(())
                }
                _ => Err("Exposure must be (0, 3600] s"),
            },
            Self::Iso => {
                if trimmed.is_empty() {
                    step.params.iso = None;
                    Ok(())
                } else {
                    match trimmed.parse::<u32>() {
                        Ok(v) if v > 0 => {
                            step.params.iso = Some(v);
                            Ok(())
                        }
                        _ => Err("ISO must be positive or blank"),
                    }
                }
            }
            Self::Count => match trimmed.parse::<u32>() {
                Ok(v) if v > 0 && v <= 10_000 => {
                    step.count = v;
                    Ok(())
                }
                _ => Err("Count must be 1..=10000"),
            },
            Self::Delay => match trimmed.parse::<f64>() {
                Ok(v) if v.is_finite() && v >= 0.0 && v <= 3600.0 => {
                    step.delay_between_frames = Duration::from_secs_f64(v);
                    Ok(())
                }
                _ => Err("Delay must be [0, 3600] s"),
            },
        }
    }

    fn step_and_apply(
        self,
        cur_txt: &str,
        step: &mut SequenceStep,
        direction: i32,
        precision: StepPrecision,
    ) -> String {
        let trimmed = cur_txt.trim();
        match self {
            Self::Exposure => {
                let cur = trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v > 0.0 && *v <= 3600.0)
                    .unwrap_or(step.params.exposure.as_secs_f64());
                let delta =
                    precision.choose(1.0_f64, 10.0_f64, 60.0_f64) * f64::from(direction);
                let next = ((cur + delta).clamp(0.1, 3600.0) * 100.0).round() / 100.0;
                step.params.exposure = Duration::from_secs_f64(next);
                format_secs(step.params.exposure)
            }
            Self::Iso => {
                let cur = if trimmed.is_empty() {
                    step.params.iso
                } else {
                    trimmed
                        .parse::<u32>()
                        .ok()
                        .filter(|&v| v > 0)
                        .or(step.params.iso)
                };
                let next = step_iso(cur, direction, precision);
                step.params.iso = next;
                next.map_or_else(String::new, |v| v.to_string())
            }
            Self::Count => {
                let cur = trimmed
                    .parse::<u32>()
                    .ok()
                    .filter(|&v| v > 0 && v <= 10_000)
                    .unwrap_or(step.count);
                let delta = precision.choose(1_i64, 5_i64, 20_i64) * i64::from(direction);
                let next = (i64::from(cur) + delta).clamp(1, 10_000) as u32;
                step.count = next;
                next.to_string()
            }
            Self::Delay => {
                let cur = trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 3600.0)
                    .unwrap_or(step.delay_between_frames.as_secs_f64());
                let delta =
                    precision.choose(1.0_f64, 5.0_f64, 30.0_f64) * f64::from(direction);
                let next = ((cur + delta).clamp(0.0, 3600.0) * 100.0).round() / 100.0;
                step.delay_between_frames = Duration::from_secs_f64(next);
                format_secs(step.delay_between_frames)
            }
        }
    }
}

fn format_secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if (s - s.round()).abs() < 1e-3 {
        format!("{}", s.round() as u64)
    } else {
        format!("{s:.2}")
    }
}

fn step_iso(cur: Option<u32>, direction: i32, precision: StepPrecision) -> Option<u32> {
    if direction == 0 {
        return cur;
    }
    match precision {
        StepPrecision::Fine => match cur {
            Some(v) => {
                let next = (v as i64 + 100 * i64::from(direction)).clamp(100, 25_600);
                Some(next as u32)
            }
            None => (direction > 0).then_some(800),
        },
        StepPrecision::Normal | StepPrecision::Coarse => {
            let Some(v) = cur else {
                return (direction > 0).then_some(800);
            };
            let stride = match precision {
                StepPrecision::Coarse => 2_usize,
                _ => 1_usize,
            };
            if direction > 0 {
                let idx = match ISO_STOPS.binary_search(&v) {
                    Ok(i) => (i + stride).min(ISO_STOPS.len() - 1),
                    Err(i) => (i + stride.saturating_sub(1)).min(ISO_STOPS.len() - 1),
                };
                Some(ISO_STOPS[idx])
            } else {
                let idx = match ISO_STOPS.binary_search(&v) {
                    Ok(i) => i.saturating_sub(stride),
                    Err(i) => i.saturating_sub(stride),
                };
                Some(ISO_STOPS[idx])
            }
        }
    }
}

fn grid_cell(border: gpui::Hsla) -> gpui::Div {
    h_flex()
        .h_full()
        .items_center()
        .border_r_1()
        .border_color(border)
}

pub struct SequenceSpreadsheetView {
    synced_revision: Option<u64>,
    synced_utc_offset: Option<i32>,
    groups: Vec<TargetRowGroup>,
    input_error: Option<String>,
    hovered_target_id: Option<String>,
    hovered_step_row: Option<(String, usize)>,
    open_dropdown: Option<OpenDropdown>,
}

impl SequenceSpreadsheetView {
    pub fn build(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                cx.observe(&plan, |_this: &mut Self, _m, cx| {
                    cx.notify();
                })
                .detach();
            }
            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |_this: &mut Self, _m, cx| {
                    cx.notify();
                })
                .detach();
            }
            if let Some(obs) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
                cx.observe(&obs, |_this: &mut Self, _m, cx| {
                    cx.notify();
                })
                .detach();
            }

            Self {
                synced_revision: None,
                synced_utc_offset: None,
                groups: Vec::new(),
                input_error: None,
                hovered_target_id: None,
                hovered_step_row: None,
                open_dropdown: None,
            }
        })
    }

    fn render_menu_backdrop(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .anchor(Anchor::TopLeft)
                .child(
                    div()
                        .id("sp_menu_backdrop")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.open_dropdown = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.open_dropdown = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                ),
        )
        .with_priority(5)
        .into_any_element()
    }

    fn render_stage_menu(
        &self,
        menu: StageMenuTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let target_id = menu.target_id;
        let step_idx = menu.step_idx;
        let id_base = menu.id_base;
        let selected = menu.selected;

        let render_option = |opt_idx: usize, choice: StageChoice| {
            let tid = target_id.clone();
            let is_selected = selected == Some(choice);
            h_flex()
                .id(("sp_menu_opt", id_base * 16 + opt_idx))
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .justify_between()
                .rounded_sm()
                .cursor_pointer()
                .when(is_selected, |el| el.bg(colors.ghost_element_selected))
                .hover(|s| s.bg(colors.element_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_dropdown = None;
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        let t_ref = tid.clone();
                        plan.update(cx, |m, cx| match step_idx {
                            Some(idx) => {
                                m.update_step(&t_ref, idx, |s| choice.apply_to_step(s), cx);
                            }
                            None => {
                                m.add_step(&t_ref, choice.default_step(), cx);
                            }
                        });
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.0))
                        .text_color(colors.text)
                        .child(choice.label()),
                )
                .when(is_selected, |el| {
                    el.child(
                        Icon::new(IconName::Check)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                })
        };

        let panel = v_flex()
            .id(("sp_stage_menu", id_base))
            .w(px(148.0))
            .occlude()
            .shadow_lg()
            .p_1()
            .gap_0p5()
            .rounded_md()
            .bg(colors.elevated_surface_background.alpha(1.0))
            .border_1()
            .border_color(colors.border)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child("Imaging"),
            )
            .children(
                IMAGING_CHOICES
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, c)| render_option(i, c)),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child("Rig & Alignment"),
            )
            .children(
                RIG_CHOICES
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, c)| render_option(IMAGING_CHOICES.len() + i, c)),
            );

        deferred(
            anchored()
                .anchor(menu.anchor)
                .snap_to_window_with_margin(px(8.0))
                .child(panel),
        )
        .with_priority(10)
        .into_any_element()
    }

    fn bind_start_time_field(
        target_id: &str,
        start_time: &Entity<TextField>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let tid = target_id.to_string();
        let field_handle = start_time.clone();
        cx.subscribe(start_time, move |this, _, ev: &TextFieldEvent, cx| {
            let utc_offset_secs = cx
                .try_global::<GlobalObserver>()
                .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());
            let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) else {
                return;
            };

            if is_session_locked(cx) {
                if matches!(
                    ev,
                    TextFieldEvent::Step { .. }
                        | TextFieldEvent::Submit(_)
                        | TextFieldEvent::Blur(_)
                        | TextFieldEvent::Cancel
                ) {
                    if let Some(start) = plan
                        .read(cx)
                        .plan()
                        .targets
                        .iter()
                        .find(|t| t.id == tid)
                        .map(|t| t.start_unix_secs)
                    {
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            f.set_text(format_unix_hm(start, utc_offset_secs), cx);
                        });
                    }
                    this.input_error = None;
                }
                return;
            }

            match ev {
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let delta = precision.choose(60_i64, 300_i64, 1800_i64) * i64::from(*direction);
                    let t_ref = tid.clone();
                    let next_start = plan.update(cx, |m, cx| {
                        m.nudge_target_start(&t_ref, delta, cx);
                        m.plan()
                            .targets
                            .iter()
                            .find(|t| t.id == t_ref)
                            .map(|t| t.start_unix_secs)
                    });
                    if let Some(start) = next_start {
                        this.input_error = None;
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            f.set_text(format_unix_hm(start, utc_offset_secs), cx);
                        });
                    }
                }
                TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) => {
                    let (night_date, ref_start) = {
                        let m = plan.read(cx);
                        let start = m
                            .plan()
                            .targets
                            .iter()
                            .find(|t| t.id == tid)
                            .map_or(0, |t| t.start_unix_secs);
                        (m.plan().night_date.clone(), start)
                    };
                    if let Some(ts) =
                        parse_hm_to_night_unix_secs(txt, &night_date, utc_offset_secs, ref_start)
                    {
                        this.input_error = None;
                        let t_ref = tid.clone();
                        let actual_start = plan.update(cx, |m, cx| {
                            m.set_target_start_unix_secs(&t_ref, ts, cx);
                            m.plan()
                                .targets
                                .iter()
                                .find(|t| t.id == t_ref)
                                .map(|t| t.start_unix_secs)
                        });
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            if let Some(start) = actual_start {
                                f.set_text(format_unix_hm(start, utc_offset_secs), cx);
                            }
                        });
                    } else {
                        field_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                        this.input_error = Some("Start time must be HH:MM".to_string());
                        cx.notify();
                    }
                }
                TextFieldEvent::Cancel => {
                    this.input_error = None;
                    if let Some(start) = plan
                        .read(cx)
                        .plan()
                        .targets
                        .iter()
                        .find(|t| t.id == tid)
                        .map(|t| t.start_unix_secs)
                    {
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            f.set_text(format_unix_hm(start, utc_offset_secs), cx);
                        });
                    }
                    cx.notify();
                }
                TextFieldEvent::Change(_) => {}
            }
        })
    }

    fn bind_step_field(
        target_id: &str,
        step_idx: usize,
        field: &Entity<TextField>,
        kind: StepFieldKind,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let tid = target_id.to_string();
        let field_handle = field.clone();
        cx.subscribe(field, move |this, _, ev: &TextFieldEvent, cx| {
            let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) else {
                return;
            };
            if is_session_locked(cx) {
                if !matches!(ev, TextFieldEvent::Change(_))
                    && let Some(step) = plan
                        .read(cx)
                        .plan()
                        .targets
                        .iter()
                        .find(|t| t.id == tid)
                        .and_then(|t| t.steps.get(step_idx))
                {
                    let txt = kind.read_text(step);
                    field_handle.update(cx, |f, cx| {
                        f.set_invalid(false, cx);
                        f.set_text(txt, cx);
                    });
                }
                return;
            }

            match ev {
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let cur_txt = field_handle.read(cx).text().to_string();
                    let t_ref = tid.clone();
                    let mut next_txt = None;
                    plan.update(cx, |m, cx| {
                        m.update_step(
                            &t_ref,
                            step_idx,
                            |s| {
                                next_txt = Some(kind.step_and_apply(
                                    &cur_txt,
                                    s,
                                    *direction,
                                    *precision,
                                ));
                            },
                            cx,
                        );
                    });
                    if let Some(txt) = next_txt {
                        this.input_error = None;
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            f.set_text(txt, cx);
                        });
                    }
                }
                TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) => {
                    let t_ref = tid.clone();
                    let mut result = Ok(());
                    plan.update(cx, |m, cx| {
                        m.update_step(
                            &t_ref,
                            step_idx,
                            |s| result = kind.apply_parsed(txt, s),
                            cx,
                        );
                    });
                    match result {
                        Ok(()) => {
                            this.input_error = None;
                            field_handle.update(cx, |f, cx| f.set_invalid(false, cx));
                        }
                        Err(msg) => {
                            field_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                            this.input_error = Some(msg.to_string());
                            cx.notify();
                        }
                    }
                }
                TextFieldEvent::Cancel => {
                    this.input_error = None;
                    if let Some(step) = plan
                        .read(cx)
                        .plan()
                        .targets
                        .iter()
                        .find(|t| t.id == tid)
                        .and_then(|t| t.steps.get(step_idx))
                    {
                        let txt = kind.read_text(step);
                        field_handle.update(cx, |f, cx| {
                            f.set_invalid(false, cx);
                            f.set_text(txt, cx);
                        });
                    }
                    cx.notify();
                }
                TextFieldEvent::Change(_) => {}
            }
        })
    }

    fn build_row_fields(
        target_id: &str,
        step_idx: usize,
        step: &SequenceStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> StepRowFields {
        let exposure = cx.new(|cx| {
            TextField::new(format_secs(step.params.exposure), window, cx)
                .suffix("s")
                .monospace(true)
                .frameless(true)
        });
        let iso_initial = step.params.iso.map_or_else(String::new, |v| v.to_string());
        let iso = cx.new(|cx| {
            TextField::new(iso_initial, window, cx)
                .placeholder("Auto")
                .monospace(true)
                .frameless(true)
        });
        let count = cx.new(|cx| {
            TextField::new(step.count.to_string(), window, cx)
                .monospace(true)
                .frameless(true)
        });
        let delay = cx.new(|cx| {
            TextField::new(format_secs(step.delay_between_frames), window, cx)
                .suffix("s")
                .monospace(true)
                .frameless(true)
        });

        let subs = vec![
            Self::bind_step_field(target_id, step_idx, &exposure, StepFieldKind::Exposure, cx),
            Self::bind_step_field(target_id, step_idx, &iso, StepFieldKind::Iso, cx),
            Self::bind_step_field(target_id, step_idx, &count, StepFieldKind::Count, cx),
            Self::bind_step_field(target_id, step_idx, &delay, StepFieldKind::Delay, cx),
        ];

        StepRowFields {
            exposure,
            iso,
            count,
            delay,
            _subs: subs,
        }
    }

    fn build_target_group_fields(
        target: &PlannedTarget,
        utc_offset_secs: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TargetRowGroup {
        let initial_hm = format_unix_hm(target.start_unix_secs, utc_offset_secs);
        let start_time = cx.new(|cx| {
            TextField::new(initial_hm, window, cx)
                .placeholder("HH:MM")
                .monospace(true)
                .frameless(true)
        });
        let start_sub = Self::bind_start_time_field(&target.id, &start_time, cx);
        let rows = target
            .steps
            .iter()
            .enumerate()
            .map(|(step_idx, step)| {
                Self::build_row_fields(&target.id, step_idx, step, window, cx)
            })
            .collect();

        TargetRowGroup {
            target_id: target.id.clone(),
            start_time,
            _start_sub: start_sub,
            rows,
        }
    }

    fn ensure_groups_synced(
        &mut self,
        targets: &[PlannedTarget],
        revision: u64,
        utc_offset_secs: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let structure_matches = self.groups.len() == targets.len()
            && self
                .groups
                .iter()
                .zip(targets)
                .all(|(g, t)| g.target_id == t.id && g.rows.len() == t.steps.len());

        if self.synced_revision == Some(revision)
            && self.synced_utc_offset == Some(utc_offset_secs)
            && structure_matches
        {
            return;
        }

        if structure_matches {
            for (group, target) in self.groups.iter().zip(targets) {
                let start_txt = format_unix_hm(target.start_unix_secs, utc_offset_secs);
                if !group.start_time.read(cx).is_focused(window) {
                    group.start_time.update(cx, |f, cx| f.set_text(start_txt, cx));
                }
                for (row, step) in group.rows.iter().zip(&target.steps) {
                    let exp_txt = format_secs(step.params.exposure);
                    if !row.exposure.read(cx).is_focused(window) {
                        row.exposure.update(cx, |f, cx| f.set_text(exp_txt, cx));
                    }
                    let iso_txt = step.params.iso.map_or_else(String::new, |v| v.to_string());
                    if !row.iso.read(cx).is_focused(window) {
                        row.iso.update(cx, |f, cx| f.set_text(iso_txt, cx));
                    }
                    let cnt_txt = step.count.to_string();
                    if !row.count.read(cx).is_focused(window) {
                        row.count.update(cx, |f, cx| f.set_text(cnt_txt, cx));
                    }
                    let del_txt = format_secs(step.delay_between_frames);
                    if !row.delay.read(cx).is_focused(window) {
                        row.delay.update(cx, |f, cx| f.set_text(del_txt, cx));
                    }
                }
            }
        } else {
            self.groups = targets
                .iter()
                .map(|target| Self::build_target_group_fields(target, utc_offset_secs, window, cx))
                .collect();
        }
        self.synced_revision = Some(revision);
        self.synced_utc_offset = Some(utc_offset_secs);
    }

    fn render_table_header(&self, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let grid_border = colors.border.opacity(0.6);
        let header_cell = |label: &'static str, w_px: Option<f32>, has_right_border: bool| {
            let mut cell = h_flex()
                .h_full()
                .items_center()
                .px_2()
                .when(has_right_border, |el| el.border_r_1().border_color(grid_border));
            cell = match w_px {
                Some(w) => cell.w(px(w)).flex_none(),
                None => cell.flex_1().min_w(px(52.0)),
            };
            cell.child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child(label),
            )
        };

        h_flex()
            .w_full()
            .h(px(HEADER_H))
            .bg(colors.surface_background)
            .border_b_1()
            .border_color(grid_border)
            .child(div().w(px(STRIPE_W)).h_full().flex_none())
            .child(header_cell("#", Some(COL_GRIP_W), true).justify_center())
            .child(header_cell("Stage", Some(COL_STAGE_W), true))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(header_cell("Exp", None, true))
                    .child(header_cell("ISO", None, true))
                    .child(header_cell("Count", None, true))
                    .child(header_cell("Delay", None, true)),
            )
            .child(header_cell("Duration", Some(COL_DURATION_W), true))
            .child(header_cell("Progress", Some(COL_PROGRESS_W), true))
            .child(header_cell("", Some(COL_ACTIONS_W), false))
            .into_any_element()
    }

    fn step_progress_label(
        target: &PlannedTarget,
        step_idx: usize,
        seq_status: Option<&SequenceStatus>,
    ) -> String {
        let Some(seq) = seq_status else {
            return String::new();
        };
        let matches_target = seq.current_target_id.as_deref() == Some(target.id.as_str())
            || (seq.plan_id.is_none() && seq.target_name == target.name);
        if !matches_target {
            return String::new();
        }
        let step = &target.steps[step_idx];
        let step_ordinal = (step_idx + 1) as u32;
        match step_ordinal.cmp(&seq.current_step) {
            std::cmp::Ordering::Less => {
                if step.kind.is_capture() {
                    format!("{}/{}", step.count, step.count)
                } else {
                    "Done".to_string()
                }
            }
            std::cmp::Ordering::Equal => {
                if step.kind.is_capture() {
                    format!("{}/{}", seq.current_step_completed, seq.current_step_total)
                } else {
                    "Running".to_string()
                }
            }
            std::cmp::Ordering::Greater => {
                if step.kind.is_capture() {
                    format!("0/{}", step.count)
                } else {
                    String::new()
                }
            }
        }
    }

    fn slew_progress_label(target: &PlannedTarget, seq_status: Option<&SequenceStatus>) -> &'static str {
        if !target.slew_to_target {
            return "";
        }
        let Some(seq) = seq_status else {
            return "";
        };
        if seq.current_target_id.as_deref() != Some(target.id.as_str()) {
            return "";
        }
        match seq.phase {
            SequencePhase::SlewingToTarget => "Slewing",
            SequencePhase::Running
            | SequencePhase::WaitingForMeridianFlip
            | SequencePhase::Parking
            | SequencePhase::Completed => "Done",
            _ => "",
        }
    }

    fn render_target_group_header(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        group: &TargetRowGroup,
        is_selected: bool,
        rctx: &SpreadsheetRenderCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locked = rctx.locked;
        let colors = cx.theme().colors();
        let grid_border = colors.border.opacity(0.6);
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let is_hovered = self.hovered_target_id.as_deref() == Some(target.id.as_str());
        let is_add_open = self.open_dropdown
            == Some(OpenDropdown::AddStage {
                target_id: target.id.clone(),
            });

        let tid_hover = target.id.clone();
        let tid_sel = target.id.clone();
        let tid_add = target.id.clone();
        let tid_menu = target.id.clone();
        let tid_del = target.id.clone();

        let dur_txt = format_duration_compact(target.estimated_duration());
        let header_bg = hsla(hue, 0.55, 0.25, if is_selected { 0.30 } else { 0.14 });
        let accent_dot = hsla(hue, 0.85, 0.62, 1.0);
        let drag_name = target.name.clone();

        let target_progress = rctx
            .seq_status
            .as_ref()
            .filter(|s| s.current_target_id.as_deref() == Some(target.id.as_str()))
            .map(|s| format!("{}/{}", s.completed, s.total))
            .unwrap_or_default();

        h_flex()
            .id(("sp_grp_hdr", target_idx))
            .w_full()
            .h(px(ROW_H))
            .bg(header_bg)
            .border_b_1()
            .border_color(grid_border)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered {
                    Some(tid_hover.clone())
                } else if this.hovered_target_id.as_deref() == Some(tid_hover.as_str()) {
                    None
                } else {
                    return;
                };
                if this.hovered_target_id != next {
                    this.hovered_target_id = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_, ev: &gpui::ClickEvent, _, cx| {
                if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                    let tid = tid_sel.clone();
                    let mods = ev.modifiers();
                    let is_mod = mods.control || mods.platform || mods.shift;
                    plan.update(cx, |m, cx| {
                        if is_mod {
                            m.toggle_target_selection(&tid, cx);
                        } else {
                            m.select_target(Some(tid), cx);
                        }
                    });
                }
            }))
            .child(div().w(px(STRIPE_W)).h_full().flex_none().bg(accent_dot))
            .child(
                h_flex()
                    .id(("sp_grp_drag", target_idx))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .border_r_1()
                    .border_color(grid_border)
                    .when(!locked, |el| {
                        el.cursor_grab().on_drag(
                            DraggedTargetGroup {
                                from_idx: target_idx,
                                name: drag_name,
                                hue,
                            },
                            |d, _offset, _window, cx| {
                                cx.new(|_| DragGhostView {
                                    label: d.name.clone(),
                                    hue: d.hue,
                                })
                            },
                        )
                    })
                    .child(
                        grid_cell(grid_border)
                            .w(px(COL_GRIP_W))
                            .flex_none()
                            .justify_center()
                            .child(if is_hovered && !locked {
                                Icon::new(IconName::ExpandVertical)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted)
                                    .into_any_element()
                            } else {
                                div().size(px(8.0)).rounded_full().bg(accent_dot).into_any_element()
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_2()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .child(target.name.clone()),
                            )
                            .child(
                                div()
                                    .id(("sp_start_time", target_idx))
                                    .w(px(56.0))
                                    .px_1()
                                    .rounded_xs()
                                    .when(!locked, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                                    .tooltip(Tooltip::text(
                                        "Start time (HH:MM) - Up/Down: 5m, Shift: 1m, Ctrl: 30m",
                                    ))
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(group.start_time.clone()),
                            ),
                    ),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_DURATION_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text)
                    .child(dur_txt),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_PROGRESS_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child(target_progress),
            )
            .child(
                h_flex()
                    .w(px(COL_ACTIONS_W))
                    .flex_none()
                    .h_full()
                    .px_1()
                    .items_center()
                    .justify_end()
                    .gap_0p5()
                    .when((is_hovered || is_add_open) && !locked, |el| {
                        el.child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new(("sp_grp_add", target_idx), IconName::Plus)
                                        .shape(IconButtonShape::Square)
                                        .icon_size(IconSize::XSmall)
                                        .style(if is_add_open {
                                            ButtonStyle::Tinted(TintColor::Accent)
                                        } else {
                                            ButtonStyle::Subtle
                                        })
                                        .tooltip(Tooltip::text("Add stage"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let next = OpenDropdown::AddStage {
                                                target_id: tid_add.clone(),
                                            };
                                            if this.open_dropdown.as_ref() == Some(&next) {
                                                this.open_dropdown = None;
                                            } else {
                                                this.open_dropdown = Some(next);
                                            }
                                            cx.notify();
                                        })),
                                )
                                .when(is_add_open, |el| {
                                    el.child(
                                        div().absolute().right_0().top_full().child(
                                            self.render_stage_menu(
                                                StageMenuTarget {
                                                    id_base: 100_000 + target_idx,
                                                    anchor: Anchor::TopRight,
                                                    selected: None,
                                                    target_id: tid_menu,
                                                    step_idx: None,
                                                },
                                                cx,
                                            ),
                                        ),
                                    )
                                }),
                        )
                        .child(
                            IconButton::new(("sp_grp_del", target_idx), IconName::Trash)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::XSmall)
                                .style(ButtonStyle::Subtle)
                                .tooltip(Tooltip::text("Remove target"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(plan) =
                                        cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                    {
                                        let tid = tid_del.clone();
                                        plan.update(cx, |m, cx| {
                                            m.remove_target(&tid, cx);
                                        });
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_slew_row(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        seq_status: Option<&SequenceStatus>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let grid_border = colors.border.opacity(0.6);
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let stripe_col = hsla(hue, 0.72, 0.54, 0.75);
        let tid_fov = target.id.clone();
        let slew = target.slew_to_target;
        let flip = target.auto_meridian_flip;
        let radec = format_radec_compact(target.ra_deg, target.dec_deg);
        let progress = Self::slew_progress_label(target, seq_status);

        h_flex()
            .w_full()
            .h(px(ROW_H))
            .bg(colors.surface_background)
            .border_b_1()
            .border_color(grid_border)
            .child(div().w(px(STRIPE_W)).h_full().flex_none().bg(stripe_col))
            .child(
                grid_cell(grid_border)
                    .w(px(COL_GRIP_W))
                    .flex_none()
                    .justify_center()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child("S"),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_STAGE_W))
                    .flex_none()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(if slew { colors.text } else { colors.text_muted })
                    .child("Slew"),
            )
            .child(
                grid_cell(grid_border)
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(12.0))
                                    .text_color(colors.text_muted)
                                    .child(radec),
                            )
                            .when(flip, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(colors.text_muted)
                                        .child("Auto Flip"),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .id(("sp_slew_fov", target_idx))
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .px_1p5()
                            .py_0p5()
                            .rounded_xs()
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.ghost_element_hover))
                            .tooltip(Tooltip::text("Preview target framing in Planetarium"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_fov.clone();
                                    plan.update(cx, |m, cx| {
                                        m.request_show_in_explore(&tid, cx);
                                    });
                                }
                            }))
                            .child(
                                Icon::new(IconName::Crosshair)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.text_muted)
                                    .child("FOV"),
                            ),
                    ),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_DURATION_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child(if slew { "Auto" } else { "Skip" }),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_PROGRESS_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child(progress),
            )
            .child(div().w(px(COL_ACTIONS_W)).flex_none().h_full())
            .into_any_element()
    }

    fn render_step_row(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        step_idx: usize,
        row: &StepRowFields,
        rctx: &SpreadsheetRenderCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locked = rctx.locked;
        let seq_status = rctx.seq_status.as_ref();
        let step = &target.steps[step_idx];
        let target_id = target.id.clone();
        let row_key = target_idx * 1024 + step_idx;
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let stripe_col = hsla(hue, 0.72, 0.54, 0.75);
        let dur_txt = match step.kind {
            StageKind::Capture => format_duration_compact(step.estimated_duration()),
            StageKind::Autofocus | StageKind::PlateSolve => "Auto".to_string(),
            StageKind::Wait => format_duration_compact(step.delay_between_frames),
        };
        let progress_txt = Self::step_progress_label(target, step_idx, seq_status);
        let is_hovered = self
            .hovered_step_row
            .as_ref()
            .is_some_and(|(id, idx)| id == &target_id && *idx == step_idx);
        let is_stage_open = self.open_dropdown
            == Some(OpenDropdown::StageCell {
                target_id: target_id.clone(),
                step_idx,
            });
        let cur_choice = StageChoice::from_step(step);
        let stage_label = step.stage_label();

        let stage_menu = (is_stage_open && !locked).then(|| {
            self.render_stage_menu(
                StageMenuTarget {
                    id_base: row_key,
                    anchor: Anchor::TopLeft,
                    selected: Some(cur_choice),
                    target_id: target_id.clone(),
                    step_idx: Some(step_idx),
                },
                cx,
            )
        });

        let colors = cx.theme().colors();
        let grid_border = colors.border.opacity(0.6);
        let row_bg = if step_idx % 2 == 0 {
            colors.elevated_surface_background
        } else {
            colors.surface_background
        };

        let tid_hover = target_id.clone();
        let tid_drag = target_id.clone();
        let tid_drop_over = target_id.clone();
        let tid_drop = target_id.clone();
        let tid_ft = target_id.clone();
        let tid_dup = target_id.clone();
        let tid_del = target_id.clone();

        h_flex()
            .id(("sp_step_row", row_key))
            .w_full()
            .h(px(ROW_H))
            .bg(row_bg)
            .border_b_1()
            .border_color(grid_border)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let target_key = (tid_hover.clone(), step_idx);
                let next = if *hovered {
                    Some(target_key)
                } else if this.hovered_step_row.as_ref() == Some(&target_key) {
                    None
                } else {
                    return;
                };
                if this.hovered_step_row != next {
                    this.hovered_step_row = next;
                    cx.notify();
                }
            }))
            .drag_over::<DraggedStageRow>(move |style, dragged: &DraggedStageRow, _, cx| {
                if dragged.target_id != tid_drop_over || dragged.from_idx == step_idx {
                    return style;
                }
                let drop_col = cx.theme().colors().border_focused;
                if step_idx < dragged.from_idx {
                    style.border_t_2().border_color(drop_col)
                } else {
                    style.border_b_2().border_color(drop_col)
                }
            })
            .on_drop(cx.listener(move |_, dragged: &DraggedStageRow, _, cx| {
                if is_session_locked(cx)
                    || dragged.target_id != tid_drop
                    || dragged.from_idx == step_idx
                {
                    return;
                }
                if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                    let tid = tid_drop.clone();
                    let from = dragged.from_idx;
                    plan.update(cx, |m, cx| {
                        m.move_step(&tid, from, step_idx, cx);
                    });
                }
            }))
            .child(div().w(px(STRIPE_W)).h_full().flex_none().bg(stripe_col))
            .child(
                h_flex()
                    .id(("sp_step_grip", row_key))
                    .w(px(COL_GRIP_W))
                    .flex_none()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .border_r_1()
                    .border_color(grid_border)
                    .when(!locked, |el| {
                        el.cursor_grab().on_drag(
                            DraggedStageRow {
                                target_id: tid_drag,
                                from_idx: step_idx,
                                label: stage_label.to_string(),
                                hue,
                            },
                            |d, _offset, _window, cx| {
                                cx.new(|_| DragGhostView {
                                    label: d.label.clone(),
                                    hue: d.hue,
                                })
                            },
                        )
                    })
                    .child(if is_hovered && !locked {
                        Icon::new(IconName::ExpandVertical)
                            .size(IconSize::XSmall)
                            .color(Color::Muted)
                            .into_any_element()
                    } else {
                        div()
                            .font_buffer(cx)
                            .text_size(px(12.0))
                            .text_color(colors.text_muted)
                            .child(format!("{}", step_idx + 1))
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .relative()
                    .w(px(COL_STAGE_W))
                    .flex_none()
                    .h_full()
                    .child(
                        grid_cell(grid_border)
                            .id(("sp_type", row_key))
                            .size_full()
                            .px_2()
                            .justify_between()
                            .gap_1()
                            .text_size(px(12.0))
                            .text_color(colors.text)
                            .when(is_stage_open, |el| el.bg(colors.ghost_element_selected))
                            .when(!locked, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(colors.ghost_element_hover))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let next = OpenDropdown::StageCell {
                                            target_id: tid_ft.clone(),
                                            step_idx,
                                        };
                                        if this.open_dropdown.as_ref() == Some(&next) {
                                            this.open_dropdown = None;
                                        } else {
                                            this.open_dropdown = Some(next);
                                        }
                                        cx.notify();
                                    }))
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .child(stage_label),
                            )
                            .when(!locked, |el| {
                                el.child(
                                    Icon::new(if is_stage_open {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                                )
                            }),
                    )
                    .when_some(stage_menu, |el, menu| {
                        el.child(div().absolute().left_0().top_full().child(menu))
                    }),
            )
            .child(match step.kind {
                StageKind::Capture => h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        grid_cell(grid_border)
                            .flex_1()
                            .min_w(px(52.0))
                            .px_1p5()
                            .child(row.exposure.clone()),
                    )
                    .child(
                        grid_cell(grid_border)
                            .flex_1()
                            .min_w(px(52.0))
                            .px_1p5()
                            .child(row.iso.clone()),
                    )
                    .child(
                        grid_cell(grid_border)
                            .flex_1()
                            .min_w(px(52.0))
                            .px_1p5()
                            .child(row.count.clone()),
                    )
                    .child(
                        grid_cell(grid_border)
                            .flex_1()
                            .min_w(px(52.0))
                            .px_1p5()
                            .child(row.delay.clone()),
                    )
                    .into_any_element(),
                StageKind::Autofocus => grid_cell(grid_border)
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child("Autofocus routine (HFR)")
                    .into_any_element(),
                StageKind::PlateSolve => grid_cell(grid_border)
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child("Plate-solve and recenter")
                    .into_any_element(),
                StageKind::Wait => grid_cell(grid_border)
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_muted)
                            .child("Wait"),
                    )
                    .child(div().w(px(72.0)).child(row.delay.clone()))
                    .into_any_element(),
            })
            .child(
                grid_cell(grid_border)
                    .w(px(COL_DURATION_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(if step.kind.is_capture() || step.kind == StageKind::Wait {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .child(dur_txt),
            )
            .child(
                grid_cell(grid_border)
                    .w(px(COL_PROGRESS_W))
                    .flex_none()
                    .px_2()
                    .font_buffer(cx)
                    .text_size(px(12.0))
                    .text_color(colors.text_muted)
                    .child(progress_txt),
            )
            .child(
                h_flex()
                    .w(px(COL_ACTIONS_W))
                    .flex_none()
                    .h_full()
                    .px_1()
                    .items_center()
                    .justify_end()
                    .gap_0p5()
                    .when(is_hovered && !locked, |el| {
                        el.child(
                            IconButton::new(("sp_dup", row_key), IconName::Copy)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::XSmall)
                                .style(ButtonStyle::Subtle)
                                .tooltip(Tooltip::text("Duplicate stage"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    if let Some(plan) =
                                        cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                    {
                                        let tid = tid_dup.clone();
                                        plan.update(cx, |m, cx| {
                                            m.duplicate_step(&tid, step_idx, cx);
                                        });
                                    }
                                })),
                        )
                        .child(
                            IconButton::new(("sp_del", row_key), IconName::Trash)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::XSmall)
                                .style(ButtonStyle::Subtle)
                                .tooltip(Tooltip::text("Remove stage"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    if let Some(plan) =
                                        cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                    {
                                        let tid = tid_del.clone();
                                        plan.update(cx, |m, cx| {
                                            m.remove_step(&tid, step_idx, cx);
                                        });
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_target_group(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        group: &TargetRowGroup,
        is_selected: bool,
        rctx: &SpreadsheetRenderCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .id(("sp_grp", target_idx))
            .w_full()
            .drag_over::<DraggedTargetGroup>(move |style, dragged: &DraggedTargetGroup, _, cx| {
                if dragged.from_idx == target_idx {
                    return style;
                }
                let drop_col = cx.theme().colors().border_focused;
                if target_idx < dragged.from_idx {
                    style.border_t_2().border_color(drop_col)
                } else {
                    style.border_b_2().border_color(drop_col)
                }
            })
            .on_drop(cx.listener(move |_, dragged: &DraggedTargetGroup, _, cx| {
                if is_session_locked(cx) || dragged.from_idx == target_idx {
                    return;
                }
                if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                    let from = dragged.from_idx;
                    plan.update(cx, |m, cx| {
                        m.move_target(from, target_idx, cx);
                    });
                }
            }))
            .child(self.render_target_group_header(
                target_idx,
                target,
                group,
                is_selected,
                rctx,
                cx,
            ))
            .child(self.render_slew_row(target_idx, target, rctx.seq_status.as_ref(), cx))
            .children(group.rows.iter().enumerate().map(|(step_idx, row)| {
                self.render_step_row(target_idx, target, step_idx, row, rctx, cx)
            }))
            .into_any_element()
    }
}

impl Render for SequenceSpreadsheetView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (targets, selected_ids, revision) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.plan().targets.clone(),
                    m.selected_target_ids().clone(),
                    m.revision(),
                )
            })
            .unwrap_or_default();

        let utc_offset_secs = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());

        self.ensure_groups_synced(&targets, revision, utc_offset_secs, window, cx);

        if targets.is_empty() {
            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    Label::new("No objects planned yet. Add targets from Explore or the left Plan panel.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        }

        let locked = is_session_locked(cx);
        let dropdown_valid = match &self.open_dropdown {
            None => true,
            Some(OpenDropdown::AddStage { target_id }) => targets.iter().any(|t| t.id == *target_id),
            Some(OpenDropdown::StageCell {
                target_id,
                step_idx,
            }) => targets
                .iter()
                .find(|t| t.id == *target_id)
                .is_some_and(|t| *step_idx < t.steps.len()),
        };
        if locked || !dropdown_valid {
            self.open_dropdown = None;
        }

        let seq_status = cx
            .try_global::<GlobalDaemonState>()
            .and_then(|g| g.0.read(cx).state.sequence.clone());
        let rctx = SpreadsheetRenderCtx { seq_status, locked };
        let colors = cx.theme().colors();
        let grid_border = colors.border.opacity(0.6);
        let has_open_dropdown = self.open_dropdown.is_some();
        let viewport = window.viewport_size();

        v_flex()
            .id("sequence_spreadsheet_root")
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(has_open_dropdown, |el| {
                el.child(self.render_menu_backdrop(viewport, cx))
            })
            .when(locked, |el| {
                el.child(
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(grid_border)
                        .child(
                            Label::new("Session running - Pause in Policy panel to edit plan")
                                .size(LabelSize::XSmall)
                                .color(Color::Warning),
                        ),
                )
            })
            .children(self.input_error.clone().map(|err| {
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(grid_border)
                    .child(
                        Label::new(err)
                            .size(LabelSize::XSmall)
                            .color(Color::Error),
                    )
            }))
            .child(
                v_flex()
                    .w_full()
                    .min_w(px(560.0))
                    .child(self.render_table_header(cx))
                    .children(
                        targets
                            .iter()
                            .zip(&self.groups)
                            .enumerate()
                            .map(|(target_idx, (target, group))| {
                                let is_sel = selected_ids.contains(&target.id);
                                self.render_target_group(
                                    target_idx,
                                    target,
                                    group,
                                    is_sel,
                                    &rctx,
                                    cx,
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_iso_handles_auto_and_stops() {
        assert_eq!(step_iso(None, 1, StepPrecision::Normal), Some(800));
        assert_eq!(step_iso(None, -1, StepPrecision::Normal), None);
        assert_eq!(step_iso(Some(800), 1, StepPrecision::Normal), Some(1600));
        assert_eq!(step_iso(Some(800), -1, StepPrecision::Normal), Some(400));
        assert_eq!(step_iso(Some(400), 1, StepPrecision::Coarse), Some(1600));
        assert_eq!(step_iso(Some(1600), -1, StepPrecision::Coarse), Some(400));
        assert_eq!(step_iso(Some(800), 1, StepPrecision::Fine), Some(900));
        assert_eq!(step_iso(Some(500), 1, StepPrecision::Normal), Some(800));
        assert_eq!(step_iso(Some(500), -1, StepPrecision::Normal), Some(400));
        assert_eq!(step_iso(Some(25600), 1, StepPrecision::Normal), Some(25600));
        assert_eq!(step_iso(Some(100), -1, StepPrecision::Normal), Some(100));
    }

    #[test]
    fn stage_choice_roundtrip_and_apply_to_step() {
        let mut step = StageChoice::Light.default_step();
        assert_eq!(StageChoice::from_step(&step), StageChoice::Light);
        assert_eq!(step.kind, StageKind::Capture);
        assert_eq!(step.frame_count(), 10);

        StageChoice::Autofocus.apply_to_step(&mut step);
        assert_eq!(StageChoice::from_step(&step), StageChoice::Autofocus);
        assert_eq!(step.kind, StageKind::Autofocus);
        assert_eq!(step.frame_count(), 0);
        assert_eq!(step.params.exposure, Duration::from_secs(60));

        StageChoice::PlateSolve.apply_to_step(&mut step);
        assert_eq!(StageChoice::from_step(&step), StageChoice::PlateSolve);
        assert_eq!(step.kind, StageKind::PlateSolve);

        StageChoice::Wait.apply_to_step(&mut step);
        assert_eq!(StageChoice::from_step(&step), StageChoice::Wait);
        assert_eq!(step.kind, StageKind::Wait);
        assert_eq!(step.delay_between_frames, Duration::from_secs(60));
        assert_eq!(step.estimated_duration(), Duration::from_secs(60));

        StageChoice::Dark.apply_to_step(&mut step);
        assert_eq!(StageChoice::from_step(&step), StageChoice::Dark);
        assert_eq!(step.kind, StageKind::Capture);
        assert_eq!(step.frame_type, FrameType::Dark);
        assert_eq!(step.delay_between_frames, Duration::ZERO);
        assert_eq!(step.frame_count(), 10);
    }
}
