use client::{DaemonCommandSender, GlobalDaemonState};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, Window, div, prelude::*, px,
};
use observer::GlobalObserver;
use protocol::{
    CameraStatus, ConnectionState, DaemonStatus, NightPlanRunRequest, SequencePhase,
    SequenceStatus, ViewerMessage,
};
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, Icon, IconName, IconSize, Label, LabelSize,
    prelude::*,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::state::{GlobalNightPlan, format_unix_hm, is_session_locked};

pub struct PlanPolicyPanel {
    focus_handle: FocusHandle,
    start_immediately: bool,
}

impl EventEmitter<PanelEvent> for PlanPolicyPanel {}

impl PlanPolicyPanel {
    pub fn build(cx: &mut App) -> Entity<Self> {
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
            Self {
                focus_handle: cx.focus_handle(),
                start_immediately: false,
            }
        })
    }

    fn send_viewer_message(&self, msg: ViewerMessage, cx: &App) {
        if let Some(sender) = cx.try_global::<DaemonCommandSender>() {
            if sender.0.try_send(msg).is_err() {
                tracing::error!("Failed to send command to daemon");
            }
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

    fn render_toggle_row(
        id: &'static str,
        label: &'static str,
        active: bool,
        disabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let indicator = div()
            .size(px(14.0))
            .rounded_xs()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(if active {
                colors.text_accent
            } else {
                colors.border
            })
            .when(active, |el| {
                el.bg(colors.text_accent.opacity(0.16))
                    .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
            });

        h_flex()
            .id(id)
            .w_full()
            .justify_between()
            .items_center()
            .px_2()
            .py_1()
            .rounded_sm()
            .when(!disabled, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            })
            .when(disabled, |el| el.opacity(0.55))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(if active { colors.text } else { colors.text_muted })
                    .child(label),
            )
            .child(indicator)
            .into_any_element()
    }

    fn render_sequence_status_card(&self, seq: &SequenceStatus, cx: &mut Context<Self>) -> AnyElement {
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
                            .on_click(cx.listener(move |this, _, _, cx| {
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
                            })),
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

        v_flex()
            .gap_2()
            .child(
                Label::new("Session Execution")
                    .size(LabelSize::Default)
                    .color(Color::Default),
            )
            .child(
                Button::new("pp_run_tonight_btn", format!("Run Tonight ({plan_frames}f)"))
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
            .child(Self::render_toggle_row(
                "pp_start_now_btn",
                "Start Immediately",
                self.start_immediately,
                false,
                |this, cx| {
                    this.start_immediately = !this.start_immediately;
                    cx.notify();
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

    fn render_target_policy_section(&self, locked: bool, cx: &mut Context<Self>) -> AnyElement {
        let selected = cx
            .try_global::<GlobalNightPlan>()
            .and_then(|g| g.0.read(cx).selected_target().cloned());

        v_flex()
            .gap_1()
            .child(
                Label::new("Selected Target Policy")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .when_none(&selected, |el| {
                el.child(
                    Label::new("No target selected")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .when_some(selected, |el, t| {
                let tid1 = t.id.clone();
                let tid2 = t.id.clone();
                let slew = t.slew_to_target;
                let flip = t.auto_meridian_flip;
                el.child(Self::render_toggle_row(
                    "pp_target_slew_btn",
                    "Slew to Target",
                    slew,
                    locked,
                    move |_, cx| {
                        if let Some(plan) =
                            cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid1.clone();
                            plan.update(cx, |m, cx| {
                                m.set_target_policy(&tid, !slew, flip, cx);
                            });
                        }
                    },
                    cx,
                ))
                .child(Self::render_toggle_row(
                    "pp_target_flip_btn",
                    "Auto Meridian Flip",
                    flip,
                    locked,
                    move |_, cx| {
                        if let Some(plan) =
                            cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid2.clone();
                            plan.update(cx, |m, cx| {
                                m.set_target_policy(&tid, slew, !flip, cx);
                            });
                        }
                    },
                    cx,
                ))
            })
            .into_any_element()
    }

    fn render_night_policy_section(&self, locked: bool, cx: &mut Context<Self>) -> AnyElement {
        let policy = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| g.0.read(cx).plan().policy)
            .unwrap_or_default();

        v_flex()
            .gap_1()
            .child(
                Label::new("Night Automation Policy")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(Self::render_toggle_row(
                "pp_park_end_btn",
                "Park Mount at End",
                policy.park_at_end,
                locked,
                |_, cx| {
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(|p| p.park_at_end = !p.park_at_end, cx);
                        });
                    }
                },
                cx,
            ))
            .child(Self::render_toggle_row(
                "pp_def_slew_btn",
                "Default Slew to Target",
                policy.default_slew_to_target,
                locked,
                |_, cx| {
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(
                                |p| p.default_slew_to_target = !p.default_slew_to_target,
                                cx,
                            );
                        });
                    }
                },
                cx,
            ))
            .child(Self::render_toggle_row(
                "pp_def_flip_btn",
                "Default Auto Flip",
                policy.default_auto_meridian_flip,
                locked,
                |_, cx| {
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            m.update_policy(
                                |p| p.default_auto_meridian_flip = !p.default_auto_meridian_flip,
                                cx,
                            );
                        });
                    }
                },
                cx,
            ))
            .into_any_element()
    }
}

impl Render for PlanPolicyPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = is_session_locked(cx);

        v_flex()
            .id("plan_policy_panel")
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().colors().panel_background)
            .overflow_y_scroll()
            .child(self.render_run_section(cx))
            .child(Divider::horizontal())
            .child(self.render_target_policy_section(locked, cx))
            .child(Divider::horizontal())
            .child(self.render_night_policy_section(locked, cx))
    }
}

impl Focusable for PlanPolicyPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PlanPolicyPanel {
    fn persistent_name() -> &'static str {
        "plan_policy_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Policy".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Plan)
    }

    fn default_size(&self, _cx: &App) -> Pixels {
        px(260.)
    }

    fn min_size(&self, _cx: &App) -> Pixels {
        px(220.)
    }
}
