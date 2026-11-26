use std::time::Duration;

use client::GlobalDaemonState;
use components::{TextField, TextFieldEvent};
use gpui::{
    AnyElement, App, Context, Entity, Render, Subscription, Window, div, hsla, prelude::*, px,
};
use observer::GlobalObserver;
use protocol::{CaptureParams, FrameType, PlannedTarget, SequencePhase, SequenceStatus, SequenceStep};
use ui::{ActiveTheme, Button, ButtonStyle, Color, Label, LabelSize, prelude::*};

use crate::state::{
    GlobalNightPlan, format_duration_compact, format_radec_compact, format_unix_hm,
    is_session_locked,
};

struct StepRowFields {
    exposure: Entity<TextField>,
    iso: Entity<TextField>,
    count: Entity<TextField>,
    delay: Entity<TextField>,
    _subs: Vec<Subscription>,
}

struct TargetRowGroup {
    target_id: String,
    rows: Vec<StepRowFields>,
}

struct SpreadsheetRenderCtx {
    total_targets: usize,
    utc_offset_secs: i32,
    seq_status: Option<SequenceStatus>,
    locked: bool,
}

fn next_frame_type(ft: FrameType) -> FrameType {
    match ft {
        FrameType::Light => FrameType::Dark,
        FrameType::Dark => FrameType::Flat,
        FrameType::Flat => FrameType::Bias,
        FrameType::Bias => FrameType::Light,
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

pub struct SequenceSpreadsheetView {
    synced_revision: Option<u64>,
    groups: Vec<TargetRowGroup>,
    input_error: Option<String>,
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
                groups: Vec::new(),
                input_error: None,
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
        });
        let iso_initial = step.params.iso.map_or_else(String::new, |v| v.to_string());
        let iso = cx.new(|cx| {
            TextField::new(iso_initial, window, cx)
                .placeholder("Auto")
                .monospace(true)
        });
        let count = cx.new(|cx| TextField::new(step.count.to_string(), window, cx).monospace(true));
        let delay = cx.new(|cx| {
            TextField::new(format_secs(step.delay_between_frames), window, cx)
                .suffix("s")
                .monospace(true)
        });

        let mut subs = Vec::with_capacity(4);

        let tid_exp = target_id.to_string();
        let exp_handle = exposure.clone();
        subs.push(cx.subscribe(&exposure, move |this, _, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) = ev {
                match txt.trim().parse::<f64>() {
                    Ok(v) if v.is_finite() && v > 0.0 && v <= 3600.0 => {
                        this.input_error = None;
                        exp_handle.update(cx, |f, cx| f.set_invalid(false, cx));
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid_exp.clone();
                            plan.update(cx, |m, cx| {
                                m.update_step(
                                    &tid,
                                    step_idx,
                                    |s| s.params.exposure = Duration::from_secs_f64(v),
                                    cx,
                                );
                            });
                        }
                    }
                    _ => {
                        exp_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                        this.input_error = Some("Exposure must be (0, 3600] s".to_string());
                        cx.notify();
                    }
                }
            }
        }));

        let tid_iso = target_id.to_string();
        let iso_handle = iso.clone();
        subs.push(cx.subscribe(&iso, move |this, _, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) = ev {
                let trimmed = txt.trim();
                let parsed = if trimmed.is_empty() {
                    Ok(None)
                } else {
                    match trimmed.parse::<u32>() {
                        Ok(v) if v > 0 => Ok(Some(v)),
                        _ => Err(()),
                    }
                };
                match parsed {
                    Ok(iso_val) => {
                        this.input_error = None;
                        iso_handle.update(cx, |f, cx| f.set_invalid(false, cx));
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid_iso.clone();
                            plan.update(cx, |m, cx| {
                                m.update_step(&tid, step_idx, |s| s.params.iso = iso_val, cx);
                            });
                        }
                    }
                    Err(()) => {
                        iso_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                        this.input_error = Some("ISO must be positive or blank".to_string());
                        cx.notify();
                    }
                }
            }
        }));

        let tid_cnt = target_id.to_string();
        let cnt_handle = count.clone();
        subs.push(cx.subscribe(&count, move |this, _, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) = ev {
                match txt.trim().parse::<u32>() {
                    Ok(v) if v > 0 && v <= 10_000 => {
                        this.input_error = None;
                        cnt_handle.update(cx, |f, cx| f.set_invalid(false, cx));
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid_cnt.clone();
                            plan.update(cx, |m, cx| {
                                m.update_step(&tid, step_idx, |s| s.count = v, cx);
                            });
                        }
                    }
                    _ => {
                        cnt_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                        this.input_error = Some("Count must be 1..=10000".to_string());
                        cx.notify();
                    }
                }
            }
        }));

        let tid_del = target_id.to_string();
        let del_handle = delay.clone();
        subs.push(cx.subscribe(&delay, move |this, _, ev: &TextFieldEvent, cx| {
            if let TextFieldEvent::Submit(txt) | TextFieldEvent::Blur(txt) = ev {
                match txt.trim().parse::<f64>() {
                    Ok(v) if v.is_finite() && v >= 0.0 && v <= 3600.0 => {
                        this.input_error = None;
                        del_handle.update(cx, |f, cx| f.set_invalid(false, cx));
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid_del.clone();
                            plan.update(cx, |m, cx| {
                                m.update_step(
                                    &tid,
                                    step_idx,
                                    |s| s.delay_between_frames = Duration::from_secs_f64(v),
                                    cx,
                                );
                            });
                        }
                    }
                    _ => {
                        del_handle.update(cx, |f, cx| f.set_invalid(true, cx));
                        this.input_error = Some("Delay must be [0, 3600] s".to_string());
                        cx.notify();
                    }
                }
            }
        }));

        StepRowFields {
            exposure,
            iso,
            count,
            delay,
            _subs: subs,
        }
    }

    fn ensure_groups_synced(
        &mut self,
        targets: &[PlannedTarget],
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let structure_matches = self.groups.len() == targets.len()
            && self
                .groups
                .iter()
                .zip(targets)
                .all(|(g, t)| g.target_id == t.id && g.rows.len() == t.steps.len());

        if self.synced_revision == Some(revision) && structure_matches {
            return;
        }

        if structure_matches {
            for (group, target) in self.groups.iter().zip(targets) {
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
                .map(|target| TargetRowGroup {
                    target_id: target.id.clone(),
                    rows: target
                        .steps
                        .iter()
                        .enumerate()
                        .map(|(step_idx, step)| {
                            Self::build_row_fields(&target.id, step_idx, step, window, cx)
                        })
                        .collect(),
                })
                .collect();
        }
        self.synced_revision = Some(revision);
    }

    fn render_table_header(&self) -> AnyElement {
        h_flex()
            .w_full()
            .gap_1p5()
            .px_2()
            .py_1()
            .child(
                div()
                    .w(px(24.0))
                    .child(Label::new("#").size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                div()
                    .w(px(40.0))
                    .child(Label::new("On").size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                div()
                    .w(px(62.0))
                    .child(Label::new("Stage").size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                div().flex_1().child(
                    Label::new("Parameters (Exp / ISO / Count / Delay)")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(64.0)).child(
                    Label::new("Duration")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(62.0)).child(
                    Label::new("Progress")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(108.0)).child(
                    Label::new("Actions")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .into_any_element()
    }

    fn step_progress_label(
        target: &PlannedTarget,
        step_idx: usize,
        step: &SequenceStep,
        seq_status: Option<&SequenceStatus>,
    ) -> String {
        let Some(seq) = seq_status else {
            return "—".to_string();
        };
        let matches_target = seq.current_target_id.as_deref() == Some(target.id.as_str())
            || (seq.plan_id.is_none() && seq.target_name == target.name);
        if !matches_target || !step.enabled {
            return "—".to_string();
        }
        let enabled_ordinal = target
            .steps
            .iter()
            .take(step_idx + 1)
            .filter(|s| s.enabled)
            .count() as u32;
        if enabled_ordinal == 0 {
            "—".to_string()
        } else if enabled_ordinal < seq.current_step {
            format!("{}/{}", step.count, step.count)
        } else if enabled_ordinal == seq.current_step {
            format!("{}/{}", seq.current_step_completed, seq.current_step_total)
        } else {
            format!("0/{}", step.count)
        }
    }

    fn slew_progress_label(target: &PlannedTarget, seq_status: Option<&SequenceStatus>) -> &'static str {
        if !target.slew_to_target {
            return "—";
        }
        let Some(seq) = seq_status else {
            return "—";
        };
        if seq.current_target_id.as_deref() != Some(target.id.as_str()) {
            return "—";
        }
        match seq.phase {
            SequencePhase::SlewingToTarget => "Slewing",
            SequencePhase::Running
            | SequencePhase::WaitingForMeridianFlip
            | SequencePhase::Parking
            | SequencePhase::Completed => "Done",
            _ => "—",
        }
    }

    fn render_target_group_header(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        is_selected: bool,
        rctx: &SpreadsheetRenderCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total_targets = rctx.total_targets;
        let utc_offset_secs = rctx.utc_offset_secs;
        let locked = rctx.locked;
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let tid_sel = target.id.clone();
        let tid_en = target.id.clone();
        let tid_prev = target.id.clone();
        let tid_next = target.id.clone();
        let tid_exp = target.id.clone();
        let tid_add = target.id.clone();
        let tid_del = target.id.clone();

        let start_hm = format_unix_hm(target.start_unix_secs, utc_offset_secs);
        let dur_txt = format_duration_compact(target.estimated_duration());
        let enabled = target.enabled;
        let header_bg = hsla(hue, 0.55, 0.25, if is_selected { 0.34 } else { 0.16 });

        h_flex()
            .id(("sp_grp_hdr", target_idx))
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .bg(header_bg)
            .cursor_pointer()
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
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(9.0)).rounded_full().bg(hsla(hue, 0.85, 0.62, 1.0)))
                    .child(
                        Button::new(("sp_grp_en", target_idx), if enabled { "On" } else { "Off" })
                            .style(if enabled {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_en.clone();
                                    plan.update(cx, |m, cx| {
                                        m.set_target_enabled(&tid, !enabled, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Label::new(target.name.clone())
                            .size(LabelSize::Small)
                            .color(if enabled { Color::Default } else { Color::Muted }),
                    )
                    .child(
                        Label::new(format_radec_compact(target.ra_deg, target.dec_deg))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new(("sp_grp_m15", target_idx), "-15m")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_prev.clone();
                                    plan.update(cx, |m, cx| m.nudge_target_start(&tid, -900, cx));
                                }
                            })),
                    )
                    .child(
                        Label::new(start_hm)
                            .size(LabelSize::XSmall)
                            .color(Color::Default),
                    )
                    .child(
                        Button::new(("sp_grp_p15", target_idx), "+15m")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_next.clone();
                                    plan.update(cx, |m, cx| m.nudge_target_start(&tid, 900, cx));
                                }
                            })),
                    )
                    .child(
                        Label::new(format!("· {dur_txt} ({}f)", target.total_frames()))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new(("sp_grp_up", target_idx), "^")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked || target_idx == 0)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    plan.update(cx, |m, cx| {
                                        m.move_target(target_idx, target_idx.saturating_sub(1), cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_grp_dn", target_idx), "v")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked || target_idx + 1 >= total_targets)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    plan.update(cx, |m, cx| {
                                        m.move_target(target_idx, target_idx + 1, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_grp_exp", target_idx), "Explore")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_exp.clone();
                                    plan.update(cx, |m, cx| {
                                        m.request_show_in_explore(&tid, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_grp_add", target_idx), "+ Stage")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_add.clone();
                                    plan.update(cx, |m, cx| {
                                        m.add_step(
                                            &tid,
                                            SequenceStep {
                                                enabled: true,
                                                frame_type: FrameType::Light,
                                                count: 10,
                                                params: CaptureParams {
                                                    exposure: Duration::from_secs(60),
                                                    iso: Some(800),
                                                },
                                                delay_between_frames: Duration::ZERO,
                                            },
                                            cx,
                                        );
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_grp_del", target_idx), "x")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
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
                    ),
            )
            .into_any_element()
    }

    fn render_slew_row(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        seq_status: Option<&SequenceStatus>,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tid = target.id.clone();
        let slew = target.slew_to_target;
        let flip = target.auto_meridian_flip;
        let param_txt = if flip {
            format!("GoTo {} · Auto Flip", format_radec_compact(target.ra_deg, target.dec_deg))
        } else {
            format!("GoTo {}", format_radec_compact(target.ra_deg, target.dec_deg))
        };
        let progress = Self::slew_progress_label(target, seq_status);

        h_flex()
            .w_full()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .bg(cx.theme().colors().surface_background)
            .child(
                div().w(px(24.0)).child(
                    Label::new("S")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(40.0)).child(
                    Button::new(("sp_slew_on", target_idx), if slew { "On" } else { "Off" })
                        .style(if slew {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .label_size(LabelSize::XSmall)
                        .disabled(locked)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(plan) =
                                cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                            {
                                let t = tid.clone();
                                plan.update(cx, |m, cx| {
                                    m.set_target_policy(&t, !slew, flip, cx);
                                });
                            }
                        })),
                ),
            )
            .child(
                div().w(px(62.0)).child(
                    Label::new("Slew")
                        .size(LabelSize::Small)
                        .color(if slew { Color::Default } else { Color::Muted }),
                ),
            )
            .child(
                div().flex_1().child(
                    Label::new(param_txt)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(64.0)).child(
                    Label::new(if slew { "Auto" } else { "0s" })
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(62.0)).child(
                    Label::new(progress)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(div().w(px(108.0)))
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
        let total_rows = target.steps.len();
        let row_key = target_idx * 1024 + step_idx;
        let dur_txt = format_duration_compact(step.estimated_duration());
        let progress_txt = Self::step_progress_label(target, step_idx, step, seq_status);
        let row_bg = if step_idx % 2 == 0 {
            cx.theme().colors().elevated_surface_background
        } else {
            cx.theme().colors().surface_background
        };

        let tid_on = target_id.clone();
        let tid_ft = target_id.clone();
        let tid_up = target_id.clone();
        let tid_dn = target_id.clone();
        let tid_dup = target_id.clone();
        let tid_del = target_id.clone();
        let cur_enabled = step.enabled;
        let cur_ft = step.frame_type;

        h_flex()
            .w_full()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .bg(row_bg)
            .child(
                div().w(px(24.0)).child(
                    Label::new(format!("{}", step_idx + 1))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(40.0)).child(
                    Button::new(("sp_on", row_key), if cur_enabled { "On" } else { "Off" })
                        .style(if cur_enabled {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .label_size(LabelSize::XSmall)
                        .disabled(locked)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(plan) =
                                cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                            {
                                let tid = tid_on.clone();
                                plan.update(cx, |m, cx| {
                                    m.update_step(&tid, step_idx, |s| s.enabled = !cur_enabled, cx);
                                });
                            }
                        })),
                ),
            )
            .child(
                div().w(px(62.0)).child(
                    Button::new(("sp_type", row_key), cur_ft.label())
                        .style(ButtonStyle::Subtle)
                        .label_size(LabelSize::XSmall)
                        .disabled(locked)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(plan) =
                                cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                            {
                                let tid = tid_ft.clone();
                                plan.update(cx, |m, cx| {
                                    m.update_step(
                                        &tid,
                                        step_idx,
                                        |s| s.frame_type = next_frame_type(cur_ft),
                                        cx,
                                    );
                                });
                            }
                        })),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .child(div().flex_1().child(row.exposure.clone()))
                    .child(div().flex_1().child(row.iso.clone()))
                    .child(div().flex_1().child(row.count.clone()))
                    .child(div().flex_1().child(row.delay.clone())),
            )
            .child(
                div().w(px(64.0)).child(
                    Label::new(dur_txt)
                        .size(LabelSize::XSmall)
                        .color(if cur_enabled { Color::Default } else { Color::Muted }),
                ),
            )
            .child(
                div().w(px(62.0)).child(
                    Label::new(progress_txt)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                h_flex()
                    .w(px(108.0))
                    .gap_1()
                    .child(
                        Button::new(("sp_up", row_key), "^")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked || step_idx == 0)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_up.clone();
                                    plan.update(cx, |m, cx| {
                                        m.move_step(&tid, step_idx, step_idx.saturating_sub(1), cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_dn", row_key), "v")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked || step_idx + 1 >= total_rows)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_dn.clone();
                                    plan.update(cx, |m, cx| {
                                        m.move_step(&tid, step_idx, step_idx + 1, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Button::new(("sp_dup", row_key), "Dup")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
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
                        Button::new(("sp_del", row_key), "x")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
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
                    ),
            )
            .into_any_element()
    }

    fn render_target_group(
        &self,
        target_idx: usize,
        target: &PlannedTarget,
        group_rows: &TargetRowGroup,
        is_selected: bool,
        rctx: &SpreadsheetRenderCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let border_col = if is_selected {
            hsla(hue, 0.85, 0.65, 0.95)
        } else {
            cx.theme().colors().border
        };

        h_flex()
            .w_full()
            .rounded_sm()
            .border_1()
            .border_color(border_col)
            .overflow_hidden()
            .child(div().w(px(4.0)).h_full().bg(hsla(hue, 0.80, 0.58, 1.0)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_target_group_header(
                        target_idx,
                        target,
                        is_selected,
                        rctx,
                        cx,
                    ))
                    .child(self.render_slew_row(
                        target_idx,
                        target,
                        rctx.seq_status.as_ref(),
                        rctx.locked,
                        cx,
                    ))
                    .children(group_rows.rows.iter().enumerate().map(|(step_idx, row)| {
                        self.render_step_row(
                            target_idx,
                            target,
                            step_idx,
                            row,
                            rctx,
                            cx,
                        )
                    })),
            )
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

        self.ensure_groups_synced(&targets, revision, window, cx);

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
        let utc_offset_secs = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());
        let seq_status = cx
            .try_global::<GlobalDaemonState>()
            .and_then(|g| g.0.read(cx).state.sequence.clone());
        let rctx = SpreadsheetRenderCtx {
            total_targets: targets.len(),
            utc_offset_secs,
            seq_status,
            locked,
        };

        v_flex()
            .id("sequence_spreadsheet_root")
            .w_full()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .when(locked, |el| {
                el.child(
                    Label::new("Session running — Pause in Policy panel to edit plan")
                        .size(LabelSize::XSmall)
                        .color(Color::Warning),
                )
            })
            .children(self.input_error.clone().map(|err| {
                Label::new(err)
                    .size(LabelSize::XSmall)
                    .color(Color::Error)
            }))
            .child(self.render_table_header())
            .child(
                v_flex().w_full().gap_2().children(
                    targets
                        .iter()
                        .zip(&self.groups)
                        .enumerate()
                        .map(|(target_idx, (target, group_rows))| {
                            let is_sel = selected_ids.contains(&target.id);
                            self.render_target_group(
                                target_idx,
                                target,
                                group_rows,
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
