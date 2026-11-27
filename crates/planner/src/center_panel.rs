use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    prelude::*,
};
use ui::{ActiveTheme, Divider, v_flex};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::{spreadsheet::SequenceSpreadsheetView, timeline::NightTimelineView};

pub struct PlanCenterPanel {
    timeline: Entity<NightTimelineView>,
    spreadsheet: Entity<SequenceSpreadsheetView>,
    focus_handle: FocusHandle,
}

impl EventEmitter<PanelEvent> for PlanCenterPanel {}

impl PlanCenterPanel {
    pub fn build(cx: &mut App) -> Entity<Self> {
        let timeline = NightTimelineView::build(cx);
        let spreadsheet = SequenceSpreadsheetView::build(cx);
        cx.new(|cx| {
            cx.observe(&timeline, |_this, _tl, cx| cx.notify()).detach();
            cx.observe(&spreadsheet, |_this, _sp, cx| cx.notify())
                .detach();
            Self {
                timeline,
                spreadsheet,
                focus_handle: cx.focus_handle(),
            }
        })
    }
}

impl Render for PlanCenterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(self.timeline.clone())
            .child(Divider::horizontal())
            .child(self.spreadsheet.clone())
    }
}

impl Focusable for PlanCenterPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PlanCenterPanel {
    fn persistent_name() -> &'static str {
        "plan_center_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Night Plan".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Plan)
    }
}
