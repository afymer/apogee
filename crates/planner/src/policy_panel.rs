use client::GlobalDaemonState;
use components::TextField;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, Window, div, hsla, prelude::*, px,
};
use observer::GlobalObserver;
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, IconButton, IconButtonShape, IconName,
    IconSize, Label, LabelSize, ToggleState, Tooltip, prelude::*,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::state::{GlobalNightPlan, format_radec_compact, is_session_locked};

/// Right dock panel showing details and sequence templates for the currently selected target group.
pub struct PlanPolicyPanel {
    focus_handle: FocusHandle,
    template_name_input: Entity<TextField>,
}

impl EventEmitter<PanelEvent> for PlanPolicyPanel {}

impl PlanPolicyPanel {
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

            let template_name_input =
                cx.new(|cx| TextField::new("", window, cx).placeholder("Template name"));

            Self {
                focus_handle: cx.focus_handle(),
                template_name_input,
            }
        })
    }

    fn render_target_policy_section(&self, locked: bool, cx: &App) -> AnyElement {
        let (selected, selected_count) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (m.selected_target().cloned(), m.selected_target_ids().len())
            })
            .unwrap_or((None, 0));

        let colors = cx.theme().colors();

        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("Selected Target")
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .when(selected_count > 1, |el| {
                        el.child(
                            Label::new(format!("{selected_count} selected"))
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        )
                    }),
            )
            .when_none(&selected, |el| {
                el.child(
                    Label::new("No target selected")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .when_some(selected, |el, t| {
                let hue = t.color_hue_deg.rem_euclid(360.0) / 360.0;
                let dot_col = hsla(hue, 0.85, 0.62, 1.0);
                let tid1 = t.id.clone();
                let tid2 = t.id.clone();
                let slew = t.slew_to_target;
                let flip = t.auto_meridian_flip;
                let radec = format_radec_compact(t.ra_deg, t.dec_deg);

                el.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(div().size(px(8.0)).rounded_full().bg(dot_col))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(colors.text)
                                .child(t.name),
                        ),
                )
                .child(
                    div()
                        .font_buffer(cx)
                        .text_size(px(11.0))
                        .text_color(colors.text_muted)
                        .child(radec),
                )
                .child(crate::render_switch_row(
                    "pp_target_slew_btn",
                    "Slew to Target",
                    slew,
                    locked,
                    move |state, _, cx| {
                        let next = *state == ToggleState::Selected;
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid1.clone();
                            plan.update(cx, |m, cx| {
                                m.set_target_policy(&tid, next, flip, cx);
                            });
                        }
                    },
                    cx,
                ))
                .child(crate::render_switch_row(
                    "pp_target_flip_btn",
                    "Auto Meridian Flip",
                    flip,
                    locked,
                    move |state, _, cx| {
                        let next = *state == ToggleState::Selected;
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            let tid = tid2.clone();
                            plan.update(cx, |m, cx| {
                                m.set_target_policy(&tid, slew, next, cx);
                            });
                        }
                    },
                    cx,
                ))
            })
            .into_any_element()
    }

    fn render_templates_section(&self, locked: bool, cx: &mut Context<Self>) -> AnyElement {
        let (templates, selected_id, selected_count) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.all_templates(),
                    m.selected_target_id().map(str::to_string),
                    m.selected_target_ids().len(),
                )
            })
            .unwrap_or_default();

        let has_selected = selected_id.is_some() || selected_count > 0;

        v_flex()
            .gap_1p5()
            .child(
                Label::new("Sequence Templates")
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .children(templates.into_iter().enumerate().map(|(idx, tpl)| {
                let tpl_id = tpl.id.clone();
                let tpl_del_id = tpl.id.clone();
                let is_builtin = tpl.builtin;
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap_1()
                    .child(
                        Label::new(tpl.name)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("pl_apply_tpl", idx), "Apply")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::XSmall)
                                    .disabled(locked || !has_selected)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if let Some(plan) =
                                            cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                        {
                                            let tid = tpl_id.clone();
                                            plan.update(cx, |m, cx| {
                                                m.apply_template_to_selected(&tid, cx);
                                            });
                                        }
                                    })),
                            )
                            .when(!is_builtin, |el| {
                                el.child(
                                    IconButton::new(("pl_del_tpl", idx), IconName::Trash)
                                        .shape(IconButtonShape::Square)
                                        .icon_size(IconSize::XSmall)
                                        .style(ButtonStyle::Subtle)
                                        .disabled(locked)
                                        .tooltip(Tooltip::text("Delete template"))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            if let Some(plan) = cx
                                                .try_global::<GlobalNightPlan>()
                                                .map(|g| g.0.clone())
                                            {
                                                let tid = tpl_del_id.clone();
                                                plan.update(cx, |m, cx| {
                                                    m.delete_user_template(&tid, cx);
                                                });
                                            }
                                        })),
                                )
                            }),
                    )
            }))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().child(self.template_name_input.clone()))
                    .child(
                        Button::new("pl_save_tpl_btn", "Save")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .disabled(locked || selected_id.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let name = this.template_name_input.read(cx).text().to_string();
                                if let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                {
                                    let sid =
                                        plan.read(cx).selected_target_id().map(str::to_string);
                                    if let Some(target_id) = sid {
                                        plan.update(cx, |m, cx| {
                                            m.save_template_from_target(&target_id, name, cx);
                                        });
                                        this.template_name_input
                                            .update(cx, |f, cx| f.set_text("", cx));
                                    }
                                }
                            })),
                    ),
            )
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
            .child(self.render_target_policy_section(locked, cx))
            .child(Divider::horizontal())
            .child(self.render_templates_section(locked, cx))
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
        "Target".into()
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
