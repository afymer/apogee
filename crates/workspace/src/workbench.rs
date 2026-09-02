use std::collections::HashMap;

use gpui::{Entity, Window, div, prelude::*};

use crate::{Workspace, WorkspaceKind};

pub struct Workbench {
    workspaces: HashMap<WorkspaceKind, Entity<Workspace>>,
    active_workspace_kind: WorkspaceKind,
}

impl Workbench {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut workspaces = HashMap::new();

        for kind in WorkspaceKind::all() {
            let ws = cx.new(|_cx| Workspace::new(*kind));
            workspaces.insert(*kind, ws);
        }
        Self {
            workspaces,
            active_workspace_kind: WorkspaceKind::all()[0],
        }
    }

    pub fn switch_workspace(&mut self, kind: WorkspaceKind, cx: &mut Context<Self>) {
        if self.active_workspace_kind != kind {
            self.active_workspace_kind = kind;
            cx.notify();
        }
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let active_ws = self.workspaces.get(&self.active_workspace_kind).cloned();
        div().size_full().flex().flex_col().child(
            div()
                .flex_1()
                .relative()
                .children(active_ws.map(|ws| ws.into_any_element())),
        )
    }
}
