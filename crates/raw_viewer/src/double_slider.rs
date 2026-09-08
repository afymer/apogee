use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Entity, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, Window, div, prelude::*, px, relative,
};
use ui::ActiveTheme;
use workspace::theme::SliderThemeExt;

use crate::bridge::RawViewerBridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveKnob {
    None,
    Min,
    Max,
}

pub struct DoubleSlider {
    bridge: RawViewerBridge,
    min_val: f32,
    max_val: f32,
    active_knob: ActiveKnob,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl DoubleSlider {
    pub fn build(bridge: RawViewerBridge, cx: &mut App) -> Entity<Self> {
        let (min_val, max_val) = bridge.display_range();
        cx.new(|_cx| Self {
            bridge,
            min_val,
            max_val,
            active_knob: ActiveKnob::None,
            track_bounds: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_range(&mut self, min: f32, max: f32, cx: &mut Context<Self>) {
        let min = min.clamp(0.0, 0.999);
        let max = max.clamp(min + 0.001, 1.0);
        self.min_val = min;
        self.max_val = max;
        self.bridge.set_display_range(min, max);
        cx.notify();
    }

    pub fn min_val(&self) -> f32 {
        self.min_val
    }

    pub fn max_val(&self) -> f32 {
        self.max_val
    }

    pub fn active_knob(&self) -> ActiveKnob {
        self.active_knob
    }

    pub fn bridge(&self) -> &RawViewerBridge {
        &self.bridge
    }

    pub fn step_min(&mut self, delta: f32, cx: &mut Context<Self>) {
        let new_min = (self.min_val + delta).clamp(0.0, self.max_val - 0.001);
        self.set_range(new_min, self.max_val, cx);
    }

    pub fn step_max(&mut self, delta: f32, cx: &mut Context<Self>) {
        let new_max = (self.max_val + delta).clamp(self.min_val + 0.001, 1.0);
        self.set_range(self.min_val, new_max, cx);
    }

    fn update_active_knob_to(&mut self, t: f32, cx: &mut Context<Self>) {
        match self.active_knob {
            ActiveKnob::Min => {
                let new_min = t.min(self.max_val - 0.001);
                self.set_range(new_min, self.max_val, cx);
            }
            ActiveKnob::Max => {
                let new_max = t.max(self.min_val + 0.001);
                self.set_range(self.min_val, new_max, cx);
            }
            ActiveKnob::None => {}
        }
    }

    fn handle_mouse_down(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.track_bounds.lock().ok().and_then(|g| *g);
        let Some(bounds) = bounds else {
            return;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return;
        }

        let mouse_x = f32::from(position.x);
        let origin_x = f32::from(bounds.origin.x);
        let t = ((mouse_x - origin_x) / width).clamp(0.0, 1.0);

        let dist_to_min = (t - self.min_val).abs();
        let dist_to_max = (t - self.max_val).abs();

        if dist_to_min <= dist_to_max {
            self.active_knob = ActiveKnob::Min;
        } else {
            self.active_knob = ActiveKnob::Max;
        }
        self.update_active_knob_to(t, cx);
    }

    fn handle_mouse_move_at(
        &mut self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.active_knob == ActiveKnob::None {
            return;
        }
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return;
        }

        let mouse_x = f32::from(position.x);
        let origin_x = f32::from(bounds.origin.x);
        let t = ((mouse_x - origin_x) / width).clamp(0.0, 1.0);
        self.update_active_knob_to(t, cx);
    }
}

impl Render for DoubleSlider {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.active_knob == ActiveKnob::None {
            let (b_min, b_max) = self.bridge.display_range();
            self.min_val = b_min;
            self.max_val = b_max;
        }

        let colors = cx.theme().slider_colors();

        let bounds_store = self.track_bounds.clone();
        let slider_entity = cx.entity().clone();

        let track_canvas = gpui::canvas(
            move |bounds, _window, _cx| {
                if let Ok(mut lock) = bounds_store.lock() {
                    *lock = Some(bounds);
                }
            },
            move |bounds, (), window, _cx| {
                let entity = slider_entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if this.active_knob != ActiveKnob::None {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    this.handle_mouse_move_at(event.position, bounds, cx);
                                } else {
                                    this.active_knob = ActiveKnob::None;
                                    cx.notify();
                                }
                            }
                        });
                    }
                });

                let entity = slider_entity.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if this.active_knob != ActiveKnob::None {
                                this.active_knob = ActiveKnob::None;
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        let span_width = (self.max_val - self.min_val).max(0.001);

        div()
            .id("double_slider_root")
            .relative()
            .w_full()
            .h(px(20.0))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.handle_mouse_down(event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.active_knob != ActiveKnob::None {
                        this.active_knob = ActiveKnob::None;
                        cx.notify();
                    }
                }),
            )
            .child(track_canvas)
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left_0()
                    .right_0()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(colors.track_background),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(relative(self.min_val))
                    .w(relative(span_width))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(colors.active_track),
            )
            .child(
                div()
                    .id("double_slider_min_knob")
                    .absolute()
                    .top(px(2.0))
                    .left(relative(self.min_val))
                    .ml(px(-8.0))
                    .size(px(16.0))
                    .rounded_full()
                    .cursor_pointer()
                    .border_2()
                    .border_color(colors.knob_border(self.active_knob == ActiveKnob::Min))
                    .bg(colors.knob_background(self.active_knob == ActiveKnob::Min))
                    .hover(|s| {
                        s.border_color(colors.knob_border_hover)
                            .bg(colors.knob_background_hover)
                            .shadow_lg()
                    })
                    .shadow_md(),
            )
            .child(
                div()
                    .id("double_slider_max_knob")
                    .absolute()
                    .top(px(2.0))
                    .left(relative(self.max_val))
                    .ml(px(-8.0))
                    .size(px(16.0))
                    .rounded_full()
                    .cursor_pointer()
                    .border_2()
                    .border_color(colors.knob_border(self.active_knob == ActiveKnob::Max))
                    .bg(colors.knob_background(self.active_knob == ActiveKnob::Max))
                    .hover(|s| {
                        s.border_color(colors.knob_border_hover)
                            .bg(colors.knob_background_hover)
                            .shadow_lg()
                    })
                    .shadow_md(),
            )
    }
}
