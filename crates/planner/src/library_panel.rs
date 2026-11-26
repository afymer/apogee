use celestial::coords::{parse_dec_degrees, parse_ra_degrees};
use client::GlobalDaemonState;
use components::TextField;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Pixels,
    Render, Window, div, prelude::*, px,
};
use jiff::Span;
use jiff::civil::Date;
use observer::GlobalObserver;
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, IconButton, IconButtonShape, IconName,
    IconSize, Label, LabelSize, Tooltip, prelude::*,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::state::{GlobalNightPlan, is_session_locked, observer_evening_date_and_start};

pub struct PlanLibraryPanel {
    focus_handle: FocusHandle,
    name_input: Entity<TextField>,
    ra_input: Entity<TextField>,
    dec_input: Entity<TextField>,
    template_name_input: Entity<TextField>,
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
            let template_name_input =
                cx.new(|cx| TextField::new("", window, cx).placeholder("Template name"));

            Self {
                focus_handle: cx.focus_handle(),
                name_input,
                ra_input,
                dec_input,
                template_name_input,
                add_error: None,
            }
        })
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

    fn render_date_header(&self, night_date: &str, locked: bool, cx: &mut Context<Self>) -> AnyElement {
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
            .children(self.add_error.clone().map(|err| {
                Label::new(err)
                    .size(LabelSize::XSmall)
                    .color(Color::Error)
            }))
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
                                    let sid = plan
                                        .read(cx)
                                        .selected_target_id()
                                        .map(str::to_string);
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
            .child(self.render_add_target_section(locked, cx))
            .child(Divider::horizontal())
            .child(self.render_templates_section(locked, cx))
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
        "Plan".into()
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
