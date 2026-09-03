pub mod dock;
pub mod workbench;

use gpui::{Entity, SharedString, div, prelude::*, rgb};

use crate::dock::{Dock, DockPosition};

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
        let left_dock = cx.new(|_cx| {
            Dock::new(
                SharedString::new("left_dock"),
                rgb(0xff0000),
                DockPosition::Left,
            )
        });
        let center_dock = cx.new(|_cx| {
            Dock::new(
                SharedString::new("center_dock"),
                rgb(0x00ff00),
                DockPosition::Center,
            )
        });
        let right_dock = cx.new(|_cx| {
            Dock::new(
                SharedString::new("right_dock"),
                rgb(0x0000ff),
                DockPosition::Right,
            )
        });
        let bottom_dock = cx.new(|_cx| {
            Dock::new(
                SharedString::new("bottom_dock"),
                rgb(0xff00ff),
                DockPosition::Bottom,
            )
        });
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
