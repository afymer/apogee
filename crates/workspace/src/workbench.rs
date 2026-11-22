use std::{collections::HashMap, sync::Arc};

use gpui::{AnyView, Entity};
use status_bar::StatusBar;
use ui::prelude::*;

use crate::{
    Workspace, WorkspaceKind,
    panel::{Panel, PanelHandle},
};

pub struct Workbench {
    workspaces: HashMap<WorkspaceKind, Entity<Workspace>>,
    active_workspace_kind: WorkspaceKind,
    title_bar: Option<AnyView>,
    status_bar: Entity<StatusBar>,
    registered_panels: Vec<Arc<dyn PanelHandle>>,
    overlays: Vec<AnyView>,
}

impl Workbench {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut workspaces = HashMap::new();

        for kind in WorkspaceKind::all() {
            let ws = cx.new(|cx| Workspace::new(*kind, cx));
            workspaces.insert(*kind, ws);
        }

        let active_workspace_kind = WorkspaceKind::default();
        let status_bar = cx.new(StatusBar::new);
        status_bar.update(cx, |bar, cx| {
            bar.set_show_planetarium_telemetry(active_workspace_kind == WorkspaceKind::Explore, cx);
        });

        Self {
            workspaces,
            active_workspace_kind,
            title_bar: None,
            registered_panels: Vec::new(),
            status_bar,
            overlays: Vec::new(),
        }
    }

    pub fn set_title_bar(&mut self, title_bar: AnyView, cx: &mut Context<Self>) {
        self.title_bar = Some(title_bar);
        cx.notify();
    }

    pub fn add_overlay(&mut self, overlay: AnyView, cx: &mut Context<Self>) {
        self.overlays.push(overlay);
        cx.notify();
    }

    pub fn active_workspace_kind(&self) -> WorkspaceKind {
        self.active_workspace_kind
    }

    pub fn switch_workspace(&mut self, kind: WorkspaceKind, cx: &mut Context<Self>) {
        if self.active_workspace_kind != kind {
            self.active_workspace_kind = kind;
            self.status_bar.update(cx, |bar, cx| {
                bar.set_show_planetarium_telemetry(kind == WorkspaceKind::Explore, cx);
            });
            cx.notify();
        }
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for workspace in self.workspaces.values() {
            workspace.update(cx, |ws, cx| {
                ws.add_panel_if_enabled(panel.clone(), cx);
            });
        }
        let handle: Arc<dyn PanelHandle> = Arc::new(panel);
        self.registered_panels.push(handle);
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_ws = self.workspaces.get(&self.active_workspace_kind).cloned();
        div()
            .size_full()
            .relative()
            .child(
                v_flex()
                    .size_full()
                    .bg(cx.theme().colors().background)
                    .children(self.title_bar.clone().map(IntoElement::into_any_element))
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .children(active_ws.map(IntoElement::into_any_element)),
                    )
                    .child(self.status_bar.clone()),
            )
            .children(self.overlays.iter().cloned().map(IntoElement::into_any_element))
    }
}
