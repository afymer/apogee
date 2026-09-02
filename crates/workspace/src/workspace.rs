pub mod workbench;

use gpui::{div, prelude::*};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceKind {
    Explore,
}

impl WorkspaceKind {
    pub fn all() -> &'static [Self] {
        &[Self::Explore]
    }
}

pub struct Workspace {
    kind: WorkspaceKind,
}

impl Workspace {
    pub fn new(kind: WorkspaceKind) -> Self {
        Workspace { kind }
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
            .child(div().flex_1().flex().flex_col().child(
                div().flex_1().relative(), // .child(self.main_view.clone()),
            ))
    }
}
