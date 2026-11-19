use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Window, div, hsla,
    prelude::*, px, relative,
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
    proximity: f32,
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
            proximity: 0.0,
            track_bounds: Arc::new(Mutex::new(None)),
        }
    }

    pub fn build(min: f32, max: f32, cx: &mut App) -> Entity<Self> {
        cx.new(|_cx| Self::new(min, max))
    }

    fn set_range_internal(&mut self, min: f32, max: f32, cx: &mut Context<Self>, emit: bool) {
        let min = if min.is_finite() { min } else { 0.0 }.clamp(0.0, 0.999);
        let max = if max.is_finite() { max } else { 1.0 }.clamp(min + 0.001, 1.0);
        if (self.min_val - min).abs() < 1e-6 && (self.max_val - max).abs() < 1e-6 {
            return;
        }
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

    pub fn proximity(&self) -> f32 {
        self.proximity
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

    fn update_proximity(
        &mut self,
        mouse_pos: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.active_knob != ActiveKnob::None {
            return;
        }

        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let mouse_x = f32::from(mouse_pos.x);
        let mouse_y = f32::from(mouse_pos.y);
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);

        let bar_y = origin_y + height / 2.0;
        let dy = (mouse_y - bar_y).abs();

        let dx = if mouse_x < origin_x {
            origin_x - mouse_x
        } else if mouse_x > origin_x + width {
            mouse_x - (origin_x + width)
        } else {
            0.0
        };

        let dist = (dx * dx + dy * dy).sqrt();

        let d_inner = 3.0;
        let d_outer = 20.0;

        let target_p = if dist <= d_inner {
            1.0
        } else if dist >= d_outer {
            0.0
        } else {
            let u = (d_outer - dist) / (d_outer - d_inner);
            u * u * (3.0 - 2.0 * u)
        };

        if (self.proximity - target_p).abs() > 0.005 {
            self.proximity = target_p;
            cx.notify();
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

        self.proximity = 1.0;

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
                            if this.active_knob == ActiveKnob::None {
                                this.update_proximity(event.position, bounds, cx);
                            } else if event.pressed_button == Some(MouseButton::Left) {
                                this.handle_mouse_move_at(event.position, bounds, cx);
                            } else {
                                this.active_knob = ActiveKnob::None;
                                this.update_proximity(event.position, bounds, cx);
                                cx.notify();
                            }
                        });
                    }
                });

                let entity = slider_entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if this.active_knob != ActiveKnob::None {
                                this.active_knob = ActiveKnob::None;
                                this.update_proximity(event.position, bounds, cx);
                                cx.notify();
                            }
                        });
                    }
                });

                let entity = slider_entity.clone();
                window.on_mouse_event(move |_event: &MouseExitEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if this.active_knob == ActiveKnob::None && this.proximity > 0.0 {
                                this.proximity = 0.0;
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

        let entity_id = cx.entity_id();
        let p = if self.active_knob == ActiveKnob::None {
            self.proximity
        } else {
            1.0
        };

        let track_h = 4.0 + 8.0 * p;
        let track_top = (24.0 - track_h) / 2.0;
        let r = track_h / 2.0;

        let lerp_hsla = |a: gpui::Hsla, b: gpui::Hsla, t: f32| -> gpui::Hsla {
            let t = t.clamp(0.0, 1.0);
            hsla(
                a.h + (b.h - a.h) * t,
                a.s + (b.s - a.s) * t,
                a.l + (b.l - a.l) * t,
                a.a + (b.a - a.a) * t,
            )
        };

        let track_bg = lerp_hsla(colors.track_background, colors.track_background_hover, p);
        let active_bg = lerp_hsla(colors.active_track_idle, colors.active_track, p);

        let active_bar = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(self.min_val))
            .ml(px(-r))
            .right(relative(1.0 - self.max_val))
            .mr(px(-r))
            .rounded_full()
            .bg(active_bg);

        let mut inner = div()
            .absolute()
            .top(px(track_top))
            .h(px(track_h))
            .left(px(r))
            .right(px(r))
            .child(active_bar);

        if p > 0.02 {
            let knob_opacity = p.powf(1.5);
            let knob_bg = hsla(
                colors.knob_background.h,
                colors.knob_background.s,
                colors.knob_background.l,
                colors.knob_background.a * knob_opacity,
            );
            let knob_border = hsla(
                colors.knob_border.h,
                colors.knob_border.s,
                colors.knob_border.l,
                colors.knob_border.a * knob_opacity,
            );

            let mut min_knob = div()
                .id(format!("double_slider_min_knob_{entity_id:?}"))
                .absolute()
                .top_0()
                .left(relative(self.min_val))
                .ml(px(-r))
                .size(px(track_h))
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(if self.active_knob == ActiveKnob::Min {
                    colors.knob_border_active
                } else {
                    knob_border
                })
                .bg(if self.active_knob == ActiveKnob::Min {
                    colors.knob_background_active
                } else {
                    knob_bg
                })
                .hover(|s| {
                    s.border_color(colors.knob_border_hover)
                        .bg(colors.knob_background_hover)
                        .shadow_md()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.active_knob = ActiveKnob::Min;
                        this.proximity = 1.0;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                );

            let mut max_knob = div()
                .id(format!("double_slider_max_knob_{entity_id:?}"))
                .absolute()
                .top_0()
                .left(relative(self.max_val))
                .ml(px(-r))
                .size(px(track_h))
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(if self.active_knob == ActiveKnob::Max {
                    colors.knob_border_active
                } else {
                    knob_border
                })
                .bg(if self.active_knob == ActiveKnob::Max {
                    colors.knob_background_active
                } else {
                    knob_bg
                })
                .hover(|s| {
                    s.border_color(colors.knob_border_hover)
                        .bg(colors.knob_background_hover)
                        .shadow_md()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.active_knob = ActiveKnob::Max;
                        this.proximity = 1.0;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                );

            if p > 0.5 {
                min_knob = min_knob.shadow_sm();
                max_knob = max_knob.shadow_sm();
            }

            if self.active_knob == ActiveKnob::Min {
                inner = inner.child(max_knob).child(min_knob);
            } else {
                inner = inner.child(min_knob).child(max_knob);
            }
        }

        div()
            .id(format!("double_slider_root_{entity_id:?}"))
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
            .child(track_canvas)
            .child(
                div()
                    .absolute()
                    .top(px(track_top))
                    .left_0()
                    .right_0()
                    .h(px(track_h))
                    .rounded_full()
                    .bg(track_bg),
            )
            .child(inner)
    }
}
