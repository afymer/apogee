use celestial::coords::{parse_dec_degrees, parse_ra_degrees};
use client::{DaemonCommandSender, GlobalDaemonState};
use components::TextField;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, Window, div, prelude::*, px,
};
use jiff::Span;
use jiff::civil::Date;
use observer::GlobalObserver;
use protocol::{
    CameraStatus, ConnectionState, DaemonStatus, NightPlanRunRequest, SequencePhase,
    SequenceStatus, ViewerMessage,
};
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, IconButton, IconButtonShape, IconName,
    IconSize, Label, LabelSize, ToggleState, Tooltip, prelude::*,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::state::{
    GlobalNightPlan, format_unix_hm, is_session_locked, observer_evening_date_and_start,
};

/// Left dock panel holding session-wide controls: schedule date, execution, night policies, and target entry.
pub struct PlanLibraryPanel {
    focus_handle: FocusHandle,
    start_immediately: bool,
    name_input: Entity<TextField>,
    ra_input: Entity<TextField>,
    dec_input: Entity<TextField>,
    add_error: Option<String>,
}

impl EventEmitter<PanelEvent> for PlanLibraryPanel {}

impl PlanLibraryPanel {
    pub fn build(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                cx.observe(&plan, |_this: &mut Self, _m, cx| cx.notify())
                    .detach();
            }
            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |_this: &mut Self, _m, cx| cx.notify())
                    .detach();
            }
            if let Some(obs) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
                cx.observe(&obs, |_this: &mut Self, _m, cx| cx.notify())
                    .detach();
            }

            let name_input = cx.new(|cx| TextField::new("", window, cx).placeholder("M31"));
            let ra_input = cx.new(|cx| {
                TextField::new("", window, cx)
                    .placeholder("00h 42m 44s")
                    .monospace(true)
            });
            let dec_input = cx.new(|cx| {
                TextField::new("", window, cx)
                    .placeholder("+41d 16m 09s")
                    .monospace(true)
            });

            Self {
                focus_handle: cx.focus_handle(),
                start_immediately: false,
                name_input,
                ra_input,
                dec_input,
                add_error: None,
            }
        })
    }

    fn send_viewer_message(&self, msg: ViewerMessage, cx: &App) {
        if let Some(sender) = cx.try_global::<DaemonCommandSender>()
            && sender.0.try_send(msg).is_err()
        {
            tracing::error!("Failed to send command to daemon");
        }
    }

    fn run_night_plan(&self, cx: &mut Context<Self>) {
        let Some(plan) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| g.0.read(cx).plan().clone())
        else {
            return;
        };
        let req = NightPlanRunRequest {
            run_id: client::next_frame_id(),
            plan,
            start_immediately: self.start_immediately,
        };
        self.send_viewer_message(ViewerMessage::StartNightPlan(req), cx);
    }

    fn shift_night_date(&self, delta_days: i64, cx: &mut Context<Self>) {
        let fallback_date = cx
            .try_global::<GlobalObserver>()
            .map(|g| observer_evening_date_and_start(g.0.read(cx)).0)
            .unwrap_or_else(|| "2026-11-26".to_string());

        let Some(plan_entity) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) else {
            return;
        };

        let cur_date_str = {
            let m = plan_entity.read(cx);
            if m.plan().night_date.trim().is_empty() {
                fallback_date
            } else {
                m.plan().night_date.clone()
            }
        };

        let next_date = cur_date_str
            .parse::<Date>()
            .ok()
            .and_then(|d| {
                if delta_days >= 0 {
                    d.checked_add(Span::new().days(delta_days)).ok()
                } else {
                    d.checked_sub(Span::new().days(-delta_days)).ok()
                }
            })
            .map_or(cur_date_str, |d| d.to_string());

        plan_entity.update(cx, |m, cx| m.set_night_date(next_date, cx));
    }

    fn submit_manual_target(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.add_error = Some("Enter a target name".to_string());
            cx.notify();
            return;
        }
        let ra_str = self.ra_input.read(cx).text().trim().to_string();
        let dec_str = self.dec_input.read(cx).text().trim().to_string();
        let Some(ra_deg) = parse_ra_degrees(&ra_str) else {
            self.add_error = Some("Invalid RA (e.g. 00h42m44s or 10.68)".to_string());
            cx.notify();
            return;
        };
        let Some(dec_deg) = parse_dec_degrees(&dec_str) else {
            self.add_error = Some("Invalid Dec (e.g. +41d16m or 41.27)".to_string());
            cx.notify();
            return;
        };
        self.add_error = None;

        let (night_date, default_start) = cx
            .try_global::<GlobalObserver>()
            .map(|g| observer_evening_date_and_start(g.0.read(cx)))
            .unwrap_or_else(|| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                (String::new(), ((now + 299) / 300) * 300)
            });

        if let Some(plan_entity) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
            let date_opt = (!night_date.is_empty()).then_some(night_date);
            plan_entity.update(cx, |m, cx| {
                m.add_target(name, ra_deg, dec_deg, date_opt, default_start, None, cx);
            });
            self.name_input.update(cx, |f, cx| f.set_text("", cx));
            self.ra_input.update(cx, |f, cx| f.set_text("", cx));
            self.dec_input.update(cx, |f, cx| f.set_text("", cx));
        }
        cx.notify();
    }

    fn render_date_header(
        &self,
        night_date: &str,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let display_date = if night_date.trim().is_empty() {
            cx.try_global::<GlobalObserver>()
                .map(|g| observer_evening_date_and_start(g.0.read(cx)).0)
                .unwrap_or_else(|| "Tonight".to_string())
        } else {
            night_date.to_string()
        };

        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("Night Schedule")
                            .size(LabelSize::Default)
                            .color(Color::Default),
                    )
                    .child(
                        Button::new("pl_tonight_btn", "Tonight")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked)
                            .on_click(cx.listener(|_, _, _, cx| {
                                let tonight = cx
                                    .try_global::<GlobalObserver>()
                                    .map(|g| observer_evening_date_and_start(g.0.read(cx)).0)
                                    .unwrap_or_default();
                                if !tonight.is_empty()
                                    && let Some(plan) =
                                        cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    plan.update(cx, |m, cx| m.set_night_date(tonight, cx));
                                }
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        IconButton::new("pl_prev_night_btn", IconName::ChevronLeft)
                            .shape(IconButtonShape::Square)
                            .icon_size(IconSize::XSmall)
                            .style(ButtonStyle::Subtle)
                            .disabled(locked)
                            .tooltip(Tooltip::text("Previous night"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shift_night_date(-1, cx);
                            })),
                    )
                    .child(
                        Label::new(display_date)
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .child(
                        IconButton::new("pl_next_night_btn", IconName::ChevronRight)
                            .shape(IconButtonShape::Square)
                            .icon_size(IconSize::XSmall)
                            .style(ButtonStyle::Subtle)
                            .disabled(locked)
                            .tooltip(Tooltip::text("Next night"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shift_night_date(1, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_sequence_status_card(
        &self,
        seq: &SequenceStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let phase_color = match seq.phase {
            SequencePhase::Running => Color::Accent,
            SequencePhase::WaitingForStart
            | SequencePhase::SlewingToTarget
            | SequencePhase::Paused
            | SequencePhase::WaitingForMeridianFlip
            | SequencePhase::Parking => Color::Warning,
            SequencePhase::Completed => Color::Success,
            SequencePhase::Aborted | SequencePhase::Failed => Color::Error,
            SequencePhase::Idle => Color::Muted,
        };
        let frac = if seq.total > 0 {
            (seq.completed as f32 / seq.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let is_active = seq.phase.is_active();
        let is_paused = seq.phase == SequencePhase::Paused;
        let has_plan_id = seq.plan_id.is_some();
        let utc_offset = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());

        v_flex()
            .gap_1p5()
            .p_2()
            .rounded_sm()
            .bg(cx.theme().colors().elevated_surface_background)
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Label::new(seq.target_name.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            )
                            .child(
                                Label::new(seq.phase.label())
                                    .size(LabelSize::Small)
                                    .color(phase_color),
                            ),
                    )
                    .child(
                        Label::new(format!("{}/{}", seq.completed, seq.total))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .when(seq.total_targets > 0 && has_plan_id, |el| {
                el.child(
                    Label::new(format!(
                        "Target {}/{}",
                        seq.current_target_index, seq.total_targets
                    ))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
                )
            })
            .when_some(seq.waiting_until_unix_secs, |el, ts| {
                el.child(
                    Label::new(format!("Waiting until {}", format_unix_hm(ts, utc_offset)))
                        .size(LabelSize::XSmall)
                        .color(Color::Warning),
                )
            })
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_sm()
                    .bg(cx.theme().colors().surface_background)
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(frac))
                            .rounded_sm()
                            .bg(cx.theme().colors().border_focused),
                    ),
            )
            .when(seq.total_steps > 0, |el| {
                let ft = seq
                    .current_frame_type
                    .map(|f| format!(" {}", f.label()))
                    .unwrap_or_default();
                el.child(
                    Label::new(format!(
                        "Step {}/{}{}: {}/{}",
                        seq.current_step,
                        seq.total_steps,
                        ft,
                        seq.current_step_completed,
                        seq.current_step_total
                    ))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
                )
            })
            .children(seq.message.clone().map(|m| {
                Label::new(m)
                    .size(LabelSize::XSmall)
                    .color(if seq.phase == SequencePhase::Failed {
                        Color::Error
                    } else {
                        Color::Muted
                    })
            }))
            .when(is_active, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(
                                "pp_pause_resume_btn",
                                if is_paused { "Resume" } else { "Pause" },
                            )
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if is_paused {
                                        if has_plan_id
                                            && let Some(plan) = cx
                                                .try_global::<GlobalNightPlan>()
                                                .map(|g| g.0.read(cx).plan().clone())
                                        {
                                            this.send_viewer_message(
                                                ViewerMessage::UpdateNightPlan(plan),
                                                cx,
                                            );
                                        }
                                        this.send_viewer_message(ViewerMessage::ResumeSequence, cx);
                                    } else {
                                        this.send_viewer_message(ViewerMessage::PauseSequence, cx);
                                    }
                                },
                            )),
                        )
                        .child(
                            Button::new("pp_abort_btn", "Abort")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send_viewer_message(ViewerMessage::Abort, cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_run_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (hw_ready, is_capturing, seq_status) = cx
            .try_global::<GlobalDaemonState>()
            .map(|g| {
                let m = g.0.read(cx);
                let ready = m.state.connection == ConnectionState::Connected
                    && m.state.camera == CameraStatus::Ready;
                let capturing = m.state.daemon == DaemonStatus::Capturing;
                (ready, capturing, m.state.sequence.clone())
            })
            .unwrap_or((false, false, None));

        let plan_frames: u32 = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                g.0.read(cx)
                    .plan()
                    .targets
                    .iter()
                    .filter(|t| t.enabled)
                    .map(|t| t.total_frames())
                    .sum()
            })
            .unwrap_or(0);

        let can_run_plan = hw_ready && !is_capturing && plan_frames > 0;
        let panel_entity = cx.entity();

        v_flex()
            .gap_2()
            .child(
                Label::new("Session Execution")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(
                Button::new(
                    "pp_run_tonight_btn",
                    format!("Run Tonight ({plan_frames}f)"),
                )
                .style(if can_run_plan {
                    ButtonStyle::Filled
                } else {
                    ButtonStyle::Subtle
                })
                .disabled(!can_run_plan)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.run_night_plan(cx);
                })),
            )
            .child(crate::render_switch_row(
                "pp_start_now_btn",
                "Start Immediately",
                self.start_immediately,
                false,
                move |state, _, cx| {
                    let next = *state == ToggleState::Selected;
                    panel_entity.update(cx, |this, cx| {
                        this.start_immediately = next;
                        cx.notify();
                    });
                },
                cx,
            ))
            .children(
                seq_status
                    .as_ref()
                    .map(|seq| self.render_sequence_status_card(seq, cx)),
            )
            .into_any_element()
    }

    fn render_night_policy_section(&self, locked: bool, cx: &App) -> AnyElement {
        let policy = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| g.0.read(cx).plan().policy)
            .unwrap_or_default();

        v_flex()
            .gap_1()
            .child(
                Label::new("Night Automation")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(crate::render_switch_row(
                "pp_park_end_btn",
                "Park Mount at End",
                policy.park_at_end,
                locked,
                |state, _, cx| {
                    let next = *state == ToggleState::Selected;
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(|p| p.park_at_end = next, cx);
                        });
                    }
                },
                cx,
            ))
            .child(crate::render_switch_row(
                "pp_def_slew_btn",
                "Default Slew to Target",
                policy.default_slew_to_target,
                locked,
                |state, _, cx| {
                    let next = *state == ToggleState::Selected;
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(|p| p.default_slew_to_target = next, cx);
                        });
                    }
                },
                cx,
            ))
            .child(crate::render_switch_row(
                "pp_def_flip_btn",
                "Default Auto Flip",
                policy.default_auto_meridian_flip,
                locked,
                |state, _, cx| {
                    let next = *state == ToggleState::Selected;
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(|p| p.default_auto_meridian_flip = next, cx);
                        });
                    }
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_add_target_section(&self, locked: bool, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1p5()
            .child(
                Label::new("Add Target")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(self.name_input.clone())
            .child(self.ra_input.clone())
            .child(self.dec_input.clone())
            .children(
                self.add_error
                    .clone()
                    .map(|err| Label::new(err).size(LabelSize::XSmall).color(Color::Error)),
            )
            .child(
                Button::new("pl_submit_add_target_btn", "+ Add Target")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .disabled(locked)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.submit_manual_target(cx);
                    })),
            )
            .into_any_element()
    }
}

impl Render for PlanLibraryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = is_session_locked(cx);
        let night_date = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| g.0.read(cx).plan().night_date.clone())
            .unwrap_or_default();

        v_flex()
            .id("plan_library_panel")
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().colors().panel_background)
            .overflow_y_scroll()
            .child(self.render_date_header(&night_date, locked, cx))
            .child(Divider::horizontal())
            .child(self.render_run_section(cx))
            .child(Divider::horizontal())
            .child(self.render_night_policy_section(locked, cx))
            .child(Divider::horizontal())
            .child(self.render_add_target_section(locked, cx))
    }
}

impl Focusable for PlanLibraryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PlanLibraryPanel {
    fn persistent_name() -> &'static str {
        "plan_library_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Session".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Plan)
    }

    fn default_size(&self, _cx: &App) -> Pixels {
        px(250.)
    }

    fn min_size(&self, _cx: &App) -> Pixels {
        px(200.)
    }
}
