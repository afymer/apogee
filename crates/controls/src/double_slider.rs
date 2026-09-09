use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Window, div, prelude::*, px, relative,
};
use ui::ActiveTheme;

use crate::theme::SliderThemeExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveKnob {
    None,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DoubleSliderEvent {
    Change { min: f32, max: f32 },
}

pub struct DoubleSlider {
    min_val: f32,
    max_val: f32,
    active_knob: ActiveKnob,
    is_hovered: bool,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl EventEmitter<DoubleSliderEvent> for DoubleSlider {}

impl DoubleSlider {
    pub fn new(min: f32, max: f32) -> Self {
        let min = min.clamp(0.0, 0.999);
        let max = max.clamp(min + 0.001, 1.0);
        Self {
            min_val: min,
            max_val: max,
            active_knob: ActiveKnob::None,
            is_hovered: false,
            track_bounds: Arc::new(Mutex::new(None)),
        }
    }

    pub fn build(min: f32, max: f32, cx: &mut App) -> Entity<Self> {
        cx.new(|_cx| Self::new(min, max))
    }

    fn set_range_internal(&mut self, min: f32, max: f32, cx: &mut Context<Self>, emit: bool) {
        let min = min.clamp(0.0, 0.999);
        let max = max.clamp(min + 0.001, 1.0);
        self.min_val = min;
        self.max_val = max;
        if emit {
            cx.emit(DoubleSliderEvent::Change { min, max });
        }
        cx.notify();
    }

    pub fn set_range(&mut self, min: f32, max: f32, cx: &mut Context<Self>) {
        self.set_range_internal(min, max, cx, false);
    }

    pub fn min_val(&self) -> f32 {
        self.min_val
    }

    pub fn max_val(&self) -> f32 {
        self.max_val
    }

    pub fn range(&self) -> (f32, f32) {
        (self.min_val, self.max_val)
    }

    pub fn active_knob(&self) -> ActiveKnob {
        self.active_knob
    }

    pub fn is_hovered(&self) -> bool {
        self.is_hovered
    }

    pub fn step_min(&mut self, delta: f32, cx: &mut Context<Self>) {
        let new_min = (self.min_val + delta).clamp(0.0, self.max_val - 0.001);
        self.set_range_internal(new_min, self.max_val, cx, true);
    }

    pub fn step_max(&mut self, delta: f32, cx: &mut Context<Self>) {
        let new_max = (self.max_val + delta).clamp(self.min_val + 0.001, 1.0);
        self.set_range_internal(self.min_val, new_max, cx, true);
    }

    fn update_active_knob_to(&mut self, t: f32, cx: &mut Context<Self>) {
        match self.active_knob {
            ActiveKnob::Min => {
                let new_min = t.min(self.max_val - 0.001);
                self.set_range_internal(new_min, self.max_val, cx, true);
            }
            ActiveKnob::Max => {
                let new_max = t.max(self.min_val + 0.001);
                self.set_range_internal(self.min_val, new_max, cx, true);
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

        let radius = 6.0;
        let usable_width = (width - 2.0 * radius).max(1.0);
        let mouse_x = f32::from(position.x);
        let origin_x = f32::from(bounds.origin.x);
        let t = ((mouse_x - origin_x - radius) / usable_width).clamp(0.0, 1.0);

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

        let radius = 6.0;
        let usable_width = (width - 2.0 * radius).max(1.0);
        let mouse_x = f32::from(position.x);
        let origin_x = f32::from(bounds.origin.x);
        let t = ((mouse_x - origin_x - radius) / usable_width).clamp(0.0, 1.0);
        self.update_active_knob_to(t, cx);
    }
}

impl Render for DoubleSlider {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let is_expanded = self.is_hovered || self.active_knob != ActiveKnob::None;

        let mut root = div()
            .id("double_slider_root")
            .relative()
            .w_full()
            .h(px(24.0))
            .cursor_pointer()
            .on_hover(cx.listener(|this, is_hovered: &bool, _window, cx| {
                if this.is_hovered != *is_hovered {
                    this.is_hovered = *is_hovered;
                    cx.notify();
                }
            }))
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
            .child(track_canvas);

        if !is_expanded {
            // Idle State: minimal 4px track line and active span, tidy without visible knobs
            let span_width = (self.max_val - self.min_val).max(0.001);
            root = root
                .child(
                    div()
                        .absolute()
                        .top(px(10.0))
                        .left_0()
                        .right_0()
                        .h(px(4.0))
                        .rounded_full()
                        .bg(colors.track_background),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(10.0))
                        .left(relative(self.min_val))
                        .w(relative(span_width))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(colors.active_track_idle),
                );
        } else {
            // Expanded State: 12px capsule track with rounded progress bar and knob handles at ends
            let track_margin = px(6.0);
            let knob_size = px(12.0);

            let min_knob = div()
                .id("double_slider_min_knob")
                .absolute()
                .top_0()
                .left(relative(self.min_val))
                .ml(px(-6.0))
                .size(knob_size)
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(colors.knob_border(self.active_knob == ActiveKnob::Min))
                .bg(colors.knob_background(self.active_knob == ActiveKnob::Min))
                .shadow_sm()
                .hover(|s| {
                    s.border_color(colors.knob_border_hover)
                        .bg(colors.knob_background_hover)
                        .shadow_md()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.active_knob = ActiveKnob::Min;
                        cx.stop_propagation();
                    }),
                );

            let max_knob = div()
                .id("double_slider_max_knob")
                .absolute()
                .top_0()
                .left(relative(self.max_val))
                .ml(px(-6.0))
                .size(knob_size)
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(colors.knob_border(self.active_knob == ActiveKnob::Max))
                .bg(colors.knob_background(self.active_knob == ActiveKnob::Max))
                .shadow_sm()
                .hover(|s| {
                    s.border_color(colors.knob_border_hover)
                        .bg(colors.knob_background_hover)
                        .shadow_md()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.active_knob = ActiveKnob::Max;
                        cx.stop_propagation();
                    }),
                );

            let active_bar = div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(self.min_val))
                .ml(px(-6.0))
                .right(relative(1.0 - self.max_val))
                .mr(px(-6.0))
                .rounded_full()
                .bg(colors.active_track);

            let mut inner = div()
                .absolute()
                .top(px(6.0))
                .bottom(px(6.0))
                .left(track_margin)
                .right(track_margin)
                .child(active_bar);

            if self.active_knob == ActiveKnob::Min {
                inner = inner.child(max_knob).child(min_knob);
            } else {
                inner = inner.child(min_knob).child(max_knob);
            }

            root = root
                .child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .left_0()
                        .right_0()
                        .h(px(12.0))
                        .rounded_full()
                        .bg(colors.track_background_hover),
                )
                .child(inner);
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_slider_initialization_clamping() {
        let slider = DoubleSlider::new(0.2, 0.8);
        assert_eq!(slider.min_val(), 0.2);
        assert_eq!(slider.max_val(), 0.8);
        assert_eq!(slider.range(), (0.2, 0.8));
        assert_eq!(slider.active_knob(), ActiveKnob::None);

        // Out-of-bounds clamps
        let slider = DoubleSlider::new(-0.5, 1.5);
        assert_eq!(slider.min_val(), 0.0);
        assert_eq!(slider.max_val(), 1.0);

        // Inverted or too close ranges
        let slider = DoubleSlider::new(0.8, 0.2);
        assert_eq!(slider.min_val(), 0.8);
        assert!((slider.max_val() - 0.801).abs() < 1e-5);
    }
}

