use std::{collections::HashMap, sync::Arc};

use gpui::Entity;
use status_bar::StatusBar;
use ui::prelude::*;

use crate::{
    Workspace, WorkspaceKind,
    panel::{Panel, PanelHandle},
};

pub struct Workbench {
    workspaces: HashMap<WorkspaceKind, Entity<Workspace>>,
    active_workspace_kind: WorkspaceKind,
    status_bar: Entity<StatusBar>,
    registered_panels: Vec<Arc<dyn PanelHandle>>,
}

impl Workbench {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut workspaces = HashMap::new();

        for kind in WorkspaceKind::all() {
            let ws = cx.new(|cx| Workspace::new(*kind, cx));
            workspaces.insert(*kind, ws);
        }

        let status_bar = cx.new(StatusBar::new);

        Self {
            workspaces,
            active_workspace_kind: WorkspaceKind::default(),
            registered_panels: Vec::new(),
            status_bar,
        }
    }

    pub fn switch_workspace(&mut self, kind: WorkspaceKind, cx: &mut Context<Self>) {
        if self.active_workspace_kind != kind {
            self.active_workspace_kind = kind;
            cx.notify();
        }
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle: Arc<dyn PanelHandle> = Arc::new(panel);
        self.registered_panels.push(handle.clone());
        for workspace in self.workspaces.values() {
            workspace.update(cx, |ws, cx| {
                ws.add_panel_if_enabled(handle.clone(), cx);
            });
        }
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_ws = self.workspaces.get(&self.active_workspace_kind).cloned();
        v_flex()
            .size_full()
            .bg(cx.theme().colors().background)
            .child(
                div()
                    .flex_1()
                    .relative()
                    .children(active_ws.map(IntoElement::into_any_element)),
            )
            .child(self.status_bar.clone())
    }
}
