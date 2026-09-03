pub mod dock;
pub mod panel;
pub mod workbench;

use std::sync::Arc;

use gpui::{Entity, SharedString, div, prelude::*};

use crate::{
    dock::{Dock, DockPosition},
    panel::PanelHandle,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkspaceKind {
    #[default]
    Explore,
}

impl WorkspaceKind {
    pub fn all() -> &'static [Self] {
        &[Self::Explore]
    }
}

pub struct Workspace {
    kind: WorkspaceKind,
    left_dock: Entity<Dock>,
    center_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
}

impl Workspace {
    pub fn new(kind: WorkspaceKind, cx: &mut Context<Self>) -> Self {
        let left_dock = cx.new(|_cx| Dock::new(SharedString::new("left_dock"), DockPosition::Left));
        let center_dock =
            cx.new(|_cx| Dock::new(SharedString::new("center_dock"), DockPosition::Center));
        let right_dock =
            cx.new(|_cx| Dock::new(SharedString::new("right_dock"), DockPosition::Right));
        let bottom_dock =
            cx.new(|_cx| Dock::new(SharedString::new("bottom_dock"), DockPosition::Bottom));
        Workspace {
            kind,
            left_dock,
            center_dock,
            right_dock,
            bottom_dock,
        }
    }

    pub fn kind(&self) -> WorkspaceKind {
        self.kind
    }

    fn dock(&self, position: DockPosition) -> &Entity<Dock> {
        match position {
            DockPosition::Left => &self.left_dock,
            DockPosition::Bottom => &self.bottom_dock,
            DockPosition::Center => &self.center_dock,
            DockPosition::Right => &self.right_dock,
        }
    }

    /// Automatically routes a panel to the correct dock if enabled in this workspace
    pub fn add_panel_if_enabled(&mut self, panel: Arc<dyn PanelHandle>, cx: &mut Context<Self>) {
        if !panel.is_enabled_in_workspace(self.kind, cx) {
            return;
        }
        let dock = self.dock(panel.position(cx));
        dock.update(cx, |dock, _cx| dock.add_panel(panel));
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::prelude::Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(self.left_dock.clone())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().relative().child(self.center_dock.clone()))
                    .child(self.bottom_dock.clone()),
            )
            .child(self.right_dock.clone())
    }
}
