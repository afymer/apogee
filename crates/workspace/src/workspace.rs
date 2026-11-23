pub mod dock;
pub mod panel;
pub mod workbench;

use std::sync::{Arc, Mutex};

use gpui::{Bounds, DragMoveEvent, Entity, Pixels, SharedString, div, prelude::*};

use crate::{
    dock::{Dock, DockPosition, DraggedDock},
    panel::Panel,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum WorkspaceKind {
    Connect,
    #[default]
    Explore,
    Mount,
    Capture,
}

impl WorkspaceKind {
    pub fn all() -> &'static [Self] {
        &[Self::Connect, Self::Explore, Self::Mount, Self::Capture]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Connect => "Connect",
            Self::Explore => "Explore",
            Self::Mount => "Mount",
            Self::Capture => "Capture",
        }
    }
}

pub struct Workspace {
    kind: WorkspaceKind,
    left_dock: Entity<Dock>,
    center_dock: Entity<Dock>,
    right_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
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
            bounds: Arc::new(Mutex::new(None)),
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

    /// Automatically routes a panel to the correct dock if enabled in this workspace.
    pub fn add_panel_if_enabled<T: Panel>(&mut self, panel: Entity<T>, cx: &mut Context<Self>) {
        if !panel.read(cx).is_enabled_in_workspace(self.kind, cx) {
            return;
        }
        let position = panel.read(cx).position(cx);
        let dock = self.dock(position);
        dock.update(cx, |dock, cx| dock.add_panel(panel, cx));
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::prelude::Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        let bounds_store = self.bounds.clone();

        div()
            .id(("workspace", self.kind as usize))
            .size_full()
            .relative()
            .flex()
            .flex_row()
            .on_drag_move(
                cx.listener(|this, e: &DragMoveEvent<DraggedDock>, _window, cx| {
                    let Some(bounds) = this.bounds.lock().ok().and_then(|g| *g) else {
                        return;
                    };
                    let pos = e.event.position;
                    match e.drag(cx).0 {
                        DockPosition::Left => {
                            let width = pos.x - bounds.left();
                            let max_w = bounds.size.width * 0.55;
                            this.left_dock
                                .update(cx, |dock, cx| dock.resize(width, max_w, cx));
                        }
                        DockPosition::Right => {
                            let width = bounds.right() - pos.x;
                            let max_w = bounds.size.width * 0.55;
                            this.right_dock
                                .update(cx, |dock, cx| dock.resize(width, max_w, cx));
                        }
                        DockPosition::Bottom => {
                            let height = bounds.bottom() - pos.y;
                            let max_h = bounds.size.height * 0.5;
                            this.bottom_dock
                                .update(cx, |dock, cx| dock.resize(height, max_h, cx));
                        }
                        DockPosition::Center => {}
                    }
                }),
            )
            .child(
                gpui::canvas(
                    move |bounds, _window, _cx| {
                        if let Ok(mut guard) = bounds_store.lock() {
                            *guard = Some(bounds);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.left_dock.clone())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(self.center_dock.clone()),
                    )
                    .child(self.bottom_dock.clone()),
            )
            .child(self.right_dock.clone())
    }
}
