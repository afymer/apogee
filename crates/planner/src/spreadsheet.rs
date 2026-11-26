use std::time::Duration;

use client::GlobalDaemonState;
use components::{TextField, TextFieldEvent};
use gpui::{
    AnyElement, App, Context, Entity, Render, Subscription, Window, div, hsla, prelude::*, px,
};
use observer::GlobalObserver;
use protocol::{CaptureParams, FrameType, PlannedTarget, SequenceStatus, SequenceStep};
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
    synced_key: Option<(String, u64)>,
    rows: Vec<StepRowFields>,
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
                synced_key: None,
                rows: Vec::new(),
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

    fn ensure_rows_synced(
        &mut self,
        target: Option<&PlannedTarget>,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = target else {
            if !self.rows.is_empty() || self.synced_key.is_some() {
                self.rows.clear();
                self.synced_key = None;
            }
            return;
        };

        let next_key = (target.id.clone(), revision);
        if self.synced_key.as_ref() == Some(&next_key) && self.rows.len() == target.steps.len() {
            return;
        }

        let same_target_and_len = self
            .synced_key
            .as_ref()
            .is_some_and(|(id, _)| id == &target.id)
            && self.rows.len() == target.steps.len();

        if same_target_and_len {
            for (row, step) in self.rows.iter().zip(&target.steps) {
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
        } else {
            self.rows = target
                .steps
                .iter()
                .enumerate()
                .map(|(idx, step)| Self::build_row_fields(&target.id, idx, step, window, cx))
                .collect();
        }
        self.synced_key = Some(next_key);
    }

    fn render_target_header(
        &self,
        target: &PlannedTarget,
        utc_offset_secs: i32,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
        let tid_prev = target.id.clone();
        let tid_next = target.id.clone();
        let start_s = target.start_unix_secs;
        let start_hm = format_unix_hm(start_s, utc_offset_secs);
        let dur_txt = format_duration_compact(target.estimated_duration());

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(10.0)).rounded_full().bg(hsla(hue, 0.80, 0.60, 1.0)))
                    .child(
                        Label::new(target.name.clone())
                            .size(LabelSize::Default)
                            .color(Color::Default),
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
                    .gap_1p5()
                    .child(
                        Button::new("sp_start_minus_btn", "-15m")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_prev.clone();
                                    plan.update(cx, |m, cx| {
                                        m.set_target_start_unix_secs(&tid, start_s - 900, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Label::new(format!("Start {start_hm}"))
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .child(
                        Button::new("sp_start_plus_btn", "+15m")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let tid = tid_next.clone();
                                    plan.update(cx, |m, cx| {
                                        m.set_target_start_unix_secs(&tid, start_s + 900, cx);
                                    });
                                }
                            })),
                    )
                    .child(
                        Label::new(format!("· {dur_txt} ({} frames)", target.total_frames()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .into_any_element()
    }

    fn render_table_header(&self) -> AnyElement {
        h_flex()
            .w_full()
            .gap_1p5()
            .px_2()
            .py_1()
            .child(div().w(px(24.0)).child(Label::new("#").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(36.0)).child(Label::new("On").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(58.0)).child(Label::new("Type").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(76.0)).child(Label::new("Exp").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(64.0)).child(Label::new("ISO").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(56.0)).child(Label::new("Count").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(64.0)).child(Label::new("Delay").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().w(px(58.0)).child(Label::new("Progress").size(LabelSize::XSmall).color(Color::Muted)))
            .child(div().flex_1().child(Label::new("Actions").size(LabelSize::XSmall).color(Color::Muted)))
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
        let matches_target = seq.target_name == target.name;
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

    fn render_step_row(
        &self,
        target: &PlannedTarget,
        step_idx: usize,
        row: &StepRowFields,
        seq_status: Option<&SequenceStatus>,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let step = &target.steps[step_idx];
        let target_id = target.id.clone();
        let total_rows = target.steps.len();
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
            .rounded_sm()
            .bg(row_bg)
            .child(
                div().w(px(24.0)).child(
                    Label::new(format!("{}", step_idx + 1))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(36.0)).child(
                    Button::new(("sp_on", step_idx), if cur_enabled { "On" } else { "Off" })
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
                div().w(px(58.0)).child(
                    Button::new(("sp_type", step_idx), cur_ft.label())
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
            .child(div().w(px(76.0)).child(row.exposure.clone()))
            .child(div().w(px(64.0)).child(row.iso.clone()))
            .child(div().w(px(56.0)).child(row.count.clone()))
            .child(div().w(px(64.0)).child(row.delay.clone()))
            .child(
                div().w(px(58.0)).child(
                    Label::new(progress_txt)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        Button::new(("sp_up", step_idx), "^")
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
                        Button::new(("sp_dn", step_idx), "v")
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
                        Button::new(("sp_dup", step_idx), "Dup")
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
                        Button::new(("sp_del", step_idx), "x")
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
}

impl Render for SequenceSpreadsheetView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (target_opt, revision) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (m.selected_target().cloned(), m.revision())
            })
            .unwrap_or((None, 0));

        self.ensure_rows_synced(target_opt.as_ref(), revision, window, cx);

        let Some(target) = target_opt else {
            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    Label::new("Select a target or click Add to Plan in Explore")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        };

        let locked = is_session_locked(cx);
        let utc_offset_secs = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());
        let seq_status = cx
            .try_global::<GlobalDaemonState>()
            .and_then(|g| g.0.read(cx).state.sequence.clone());
        let target_id_add = target.id.clone();

        v_flex()
            .id("sequence_spreadsheet_root")
            .w_full()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(self.render_target_header(&target, utc_offset_secs, locked, cx))
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
                v_flex().w_full().gap_1().children(
                    self.rows.iter().enumerate().map(|(idx, row)| {
                        self.render_step_row(
                            &target,
                            idx,
                            row,
                            seq_status.as_ref(),
                            locked,
                            cx,
                        )
                    }),
                ),
            )
            .child(
                h_flex().pt_1().child(
                    Button::new("sp_add_stage_btn", "+ Add Stage")
                        .style(ButtonStyle::Subtle)
                        .label_size(LabelSize::Small)
                        .disabled(locked)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(plan) =
                                cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                            {
                                let tid = target_id_add.clone();
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
                ),
            )
            .into_any_element()
    }
}
