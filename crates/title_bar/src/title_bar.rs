use gpui::{App, Context, IntoElement, Render, WeakEntity, Window, prelude::*};
use ui::{Button, ButtonStyle, ContextMenu, Icon, IconName, LabelSize, PopoverMenu, prelude::*};
use workspace::{WorkspaceKind, workbench::Workbench};

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        let Some(window) = window else { return };
        let workbench_handle = cx.entity().downgrade();
        let title_bar = cx.new(|cx| TitleBar::new(workbench_handle, window, cx));
        workbench.set_title_bar(title_bar.into(), cx);
    })
    .detach();
}

pub struct TitleBar {
    workbench: WeakEntity<Workbench>,
}

impl TitleBar {
    pub fn new(
        workbench: WeakEntity<Workbench>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(workbench) = workbench.upgrade() {
            cx.observe(&workbench, |_this, _workbench, cx| {
                cx.notify();
            })
            .detach();
        }
        Self { workbench }
    }

    fn render_app_menus(&self) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_0p5()
            .child(self.render_file_menu())
            .child(self.render_run_menu())
    }

    fn render_file_menu(&self) -> impl IntoElement {
        PopoverMenu::new("menu_file")
            .trigger(
                Button::new("file_trigger", "File")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small),
            )
            .menu(|window, cx| {
                Some(ContextMenu::build(window, cx, |menu, _window, _cx| {
                    menu.entry("Exit", None, |_window, cx| cx.quit())
                }))
            })
    }

    fn render_run_menu(&self) -> impl IntoElement {
        PopoverMenu::new("menu_run")
            .trigger(
                Button::new("run_trigger", "Run")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small),
            )
            .menu(|window, cx| {
                Some(ContextMenu::build(window, cx, |menu, _window, _cx| {
                    menu.entry("Capture Frame", None, |_window, _cx| {})
                }))
            })
    }

    fn render_workspace_switcher(&self, cx: &App) -> impl IntoElement {
        let active_kind = self
            .workbench
            .upgrade()
            .map(|wb| wb.read(cx).active_workspace_kind())
            .unwrap_or_default();

        let mut switcher = h_flex()
            .items_center()
            .gap_1()
            .p_0p5()
            .rounded_md()
            .bg(cx.theme().colors().panel_background);

        for kind in WorkspaceKind::all() {
            let is_active = *kind == active_kind;
            let kind = *kind;
            let workbench = self.workbench.clone();

            let button = Button::new(format!("ws_tab_{:?}", kind), kind.name())
                .style(if is_active {
                    ButtonStyle::Filled
                } else {
                    ButtonStyle::Subtle
                })
                .label_size(LabelSize::Small)
                .on_click(move |_event, _window, cx| {
                    if let Some(workbench) = workbench.upgrade() {
                        workbench.update(cx, |wb, cx| {
                            wb.switch_workspace(kind, cx);
                        });
                    }
                });

            switcher = switcher.child(button);
        }

        switcher
    }

    fn render_user_info_dropdown(&self) -> impl IntoElement {
        PopoverMenu::new("user_info_menu")
            .trigger(
                Button::new("user_info_trigger", "Observer")
                    .start_icon(Icon::new(IconName::UserGroup))
                    .end_icon(Icon::new(IconName::ChevronDown))
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small),
            )
            .menu(|window, cx| {
                Some(ContextMenu::build(window, cx, |menu, _window, _cx| {
                    menu.header("Profile")
                }))
            })
    }
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h_8()
            .items_center()
            .justify_between()
            .px_2()
            .bg(cx.theme().colors().title_bar_background)
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(self.render_app_menus())
            .child(self.render_workspace_switcher(cx))
            .child(self.render_user_info_dropdown())
    }
}
