use std::sync::Arc;

use gpui::{
    Entity, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, SharedString, Subscription,
    deferred, div, px,
};
use ui::prelude::*;

use crate::panel::{Panel, PanelEvent, PanelHandle};

pub(crate) const RESIZE_HANDLE_SIZE: Pixels = px(6.);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockPosition {
    Left,
    Right,
    Bottom,
    Center,
}

#[derive(Clone)]
pub(crate) struct DraggedDock(pub DockPosition);

impl Render for DraggedDock {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

struct PanelEntry {
    panel: Arc<dyn PanelHandle>,
    _subscriptions: [Subscription; 2],
}

pub(crate) struct Dock {
    title: SharedString,
    position: DockPosition,
    size: Option<Pixels>,
    entries: Vec<PanelEntry>,
    active_index: Option<usize>,
    is_open: bool,
}

impl Dock {
    pub(crate) fn new(title: SharedString, position: DockPosition) -> Self {
        Self {
            title,
            position,
            size: None,
            entries: vec![],
            active_index: None,
            is_open: position == DockPosition::Center,
        }
    }

    fn active_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        self.active_index
            .and_then(|i| self.entries.get(i))
            .map(|entry| &entry.panel)
    }

    #[allow(dead_code)]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.position == DockPosition::Center {
            self.is_open = true;
            return;
        }
        if open == self.is_open {
            return;
        }
        self.is_open = open;
        if let Some(active) = self.active_panel().cloned() {
            cx.defer(move |cx| active.set_active(open, cx));
        }
        cx.notify();
    }

    pub fn activate_panel(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.entries.len() || self.active_index == Some(index) {
            return;
        }
        if self.is_open {
            if let Some(prev) = self.active_panel().cloned() {
                cx.defer(move |cx| prev.set_active(false, cx));
            }
            self.active_index = Some(index);
            if let Some(next) = self.active_panel().cloned() {
                cx.defer(move |cx| next.set_active(true, cx));
            }
        } else {
            self.active_index = Some(index);
        }
        cx.notify();
    }

    pub fn resize(&mut self, size: Pixels, max_size: Pixels, cx: &mut Context<Self>) {
        let min_size = self
            .active_panel()
            .map_or_else(|| px(200.), |panel| panel.min_size(cx));
        let min_f = f32::from(min_size);
        let upper_f = f32::from(max_size).max(min_f);
        let clamped = px(f32::from(size).clamp(min_f, upper_f));
        if self.size != Some(clamped) {
            self.size = Some(clamped);
            cx.notify();
        }
    }

    pub fn reset_size(&mut self, cx: &mut Context<Self>) {
        if self.size.take().is_some() {
            cx.notify();
        }
    }

    pub fn add_panel<T: Panel>(&mut self, panel: Entity<T>, cx: &mut Context<Self>) {
        let panel_id = panel.entity_id();
        if self
            .entries
            .iter()
            .any(|entry| entry.panel.panel_id() == panel_id)
        {
            return;
        }

        let subscriptions = [
            cx.observe(&panel, |_, _, cx| cx.notify()),
            cx.subscribe(&panel, |this, panel, event: &PanelEvent, cx| match event {
                PanelEvent::Activate => {
                    if let Some(ix) = this
                        .entries
                        .iter()
                        .position(|entry| entry.panel.panel_id() == panel.entity_id())
                    {
                        this.activate_panel(ix, cx);
                        this.set_open(true, cx);
                    }
                }
                PanelEvent::Close => {
                    if this.is_open
                        && this
                            .active_panel()
                            .is_some_and(|active| active.panel_id() == panel.entity_id())
                    {
                        this.set_open(false, cx);
                    }
                }
                PanelEvent::Toggle => {
                    if let Some(ix) = this
                        .entries
                        .iter()
                        .position(|entry| entry.panel.panel_id() == panel.entity_id())
                    {
                        if this.is_open && this.active_index == Some(ix) {
                            this.set_open(false, cx);
                        } else {
                            this.activate_panel(ix, cx);
                            this.set_open(true, cx);
                        }
                    }
                }
            }),
        ];

        let starts_open = panel.read(cx).starts_open(cx);
        let idx = self.entries.len();
        let handle: Arc<dyn PanelHandle> = Arc::new(panel);
        self.entries.push(PanelEntry {
            panel: handle.clone(),
            _subscriptions: subscriptions,
        });

        if self.active_index.is_none() {
            self.active_index = Some(idx);
        }

        if self.position == DockPosition::Center || starts_open {
            self.active_index = Some(idx);
            self.is_open = true;
            cx.defer(move |cx| handle.set_active(true, cx));
        }

        cx.notify();
    }
}

impl Render for Dock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.entries.is_empty() || (self.position != DockPosition::Center && !self.is_open) {
            return div().into_any_element();
        }

        let Some(active_handle) = self.active_panel().cloned() else {
            return div().into_any_element();
        };

        if self.position == DockPosition::Center {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().flex_1().min_h_0().overflow_hidden().child(active_handle.to_any()))
                .into_any_element();
        }

        let dock_size = self.size.unwrap_or_else(|| active_handle.default_size(cx));
        let colors = cx.theme().colors();

        let mut root = div()
            .id(self.title.clone())
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .bg(colors.panel_background)
            .border_color(colors.border);

        root = match self.position {
            DockPosition::Left => root.h_full().w(dock_size).border_r_1(),
            DockPosition::Right => root.h_full().w(dock_size).border_l_1(),
            DockPosition::Bottom => root.w_full().h(dock_size).border_t_1(),
            DockPosition::Center => unreachable!(),
        };

        if self.entries.len() > 1 {
            let tabs = self.entries.iter().enumerate().map(|(idx, entry)| {
                let is_active = self.active_index == Some(idx);
                let title = entry.panel.title(cx);
                Button::new(format!("{}_tab_{idx}", self.title), title)
                    .style(if is_active {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.activate_panel(idx, cx);
                    }))
            });

            root = root.child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .bg(colors.panel_background)
                    .children(tabs),
            );
        }

        let position = self.position;
        let handle_base = div()
            .id(format!("{}_resize_handle", self.title))
            .on_drag(DraggedDock(position), |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dragged.clone())
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|dock, e: &MouseUpEvent, _window, cx| {
                    if e.click_count == 2 {
                        dock.reset_size(cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .occlude();

        let resize_handle = match position {
            DockPosition::Left => deferred(
                handle_base
                    .absolute()
                    .right(-RESIZE_HANDLE_SIZE / 2.)
                    .top(px(0.))
                    .h_full()
                    .w(RESIZE_HANDLE_SIZE)
                    .cursor_col_resize(),
            ),
            DockPosition::Right => deferred(
                handle_base
                    .absolute()
                    .top(px(0.))
                    .left(-RESIZE_HANDLE_SIZE / 2.)
                    .h_full()
                    .w(RESIZE_HANDLE_SIZE)
                    .cursor_col_resize(),
            ),
            DockPosition::Bottom => deferred(
                handle_base
                    .absolute()
                    .top(-RESIZE_HANDLE_SIZE / 2.)
                    .left(px(0.))
                    .w_full()
                    .h(RESIZE_HANDLE_SIZE)
                    .cursor_row_resize(),
            ),
            DockPosition::Center => unreachable!(),
        };

        root.child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(active_handle.to_any()),
        )
        .child(resize_handle)
        .into_any_element()
    }
}
