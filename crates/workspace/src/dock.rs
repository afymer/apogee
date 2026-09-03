use std::sync::Arc;

use gpui::{Fill, Pixels, SharedString, div, prelude::*, px};

use crate::panel::PanelHandle;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockPosition {
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
    panels: Vec<Arc<dyn PanelHandle>>,
    active_index: Option<usize>,
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
            panels: vec![],
            active_index: None,
        }
    }

    pub fn add_panel(&mut self, panel: Arc<dyn PanelHandle>, _cx: &mut Context<Self>) {
        // Prevent duplicate panels
        if !self.panels.iter().any(|p| p.panel_id() == panel.panel_id()) {
            self.panels.push(panel);
            if self.active_index.is_none() {
                self.active_index = Some(0);
            }
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
        let active_panel = self
            .active_index
            .and_then(|i| self.panels.get(i))
            .map(|panel| div().flex_1().overflow_hidden().child(panel.to_any()));
        root.bg(self.fill.clone())
            .child(self.title.to_string())
            .children(active_panel)
    }
}
