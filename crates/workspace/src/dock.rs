use gpui::{Fill, Pixels, SharedString, div, prelude::*, px};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DockPosition {
    Left,
    Right,
    Bottom,
    Center,
}

pub(crate) struct Dock {
    title: SharedString,
    fill: Fill,
    position: DockPosition,
    size: Pixels,
}

impl Dock {
    pub(crate) fn new(title: SharedString, fill: impl Into<Fill>, position: DockPosition) -> Self {
        let size = match position {
            DockPosition::Left | DockPosition::Right => px(100.),
            DockPosition::Bottom => px(50.),
            DockPosition::Center => px(50.),
        };
        Self {
            title,
            fill: fill.into(),
            position,
            size,
        }
    }
}

impl Render for Dock {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::prelude::Context<Self>,
    ) -> impl IntoElement {
        let mut root = div().flex();
        root = match self.position {
            DockPosition::Left | DockPosition::Right => root.flex_col().h_full().w(self.size),
            DockPosition::Bottom => root.flex_col().w_full().h(self.size),
            DockPosition::Center => root.flex_col().size_full(),
        };
        root.bg(self.fill.clone()).child(self.title.to_string())
    }
}
