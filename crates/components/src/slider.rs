use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Window, div, hsla,
    prelude::*, px, relative,
};
use ui::ActiveTheme;

use crate::slider_utils::{lerp_hsla, normalized_x, smoothstep_proximity};
use crate::theme::SliderThemeExt;

/// Events emitted by [`Slider`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliderEvent {
    /// Emitted continuously while dragging or clicking to a new normalized value in `[0.0, 1.0]`.
    Change(f32),
    /// Emitted once when the pointer interaction finishes (mouse up or double-click reset).
    Commit(f32),
}

/// Reusable single-handle proximity-reactive slider supporting optional center-origin bipolar fill.
pub struct Slider {
    value: f32,
    is_dragging: bool,
    is_hovered: bool,
    proximity: f32,
    center_mark: Option<f32>,
    fill_from_center: bool,
    accent_color: Option<Hsla>,
    default_value: Option<f32>,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl EventEmitter<SliderEvent> for Slider {}

impl Slider {
    /// Creates a slider initialized at normalized `value` in `[0.0, 1.0]`.
    pub fn new(value: f32) -> Self {
        let safe = if value.is_finite() { value } else { 0.0 }.clamp(0.0, 1.0);
        Self {
            value: safe,
            is_dragging: false,
            is_hovered: false,
            proximity: 0.0,
            center_mark: None,
            fill_from_center: false,
            accent_color: None,
            default_value: None,
            track_bounds: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates a [`Slider`] entity in the GPUI context.
    pub fn build(value: f32, cx: &mut App) -> Entity<Self> {
        cx.new(|_cx| Self::new(value))
    }

    /// Places a subtle tick mark at normalized position `mark` in `[0.0, 1.0]`.
    #[must_use]
    pub fn center_mark(mut self, mark: f32) -> Self {
        if mark.is_finite() {
            self.center_mark = Some(mark.clamp(0.0, 1.0));
        }
        self
    }

    /// When `true` and [`Self::center_mark`] is set, fills the active bar from the mark to `value`.
    #[must_use]
    pub fn fill_from_center(mut self, enabled: bool) -> Self {
        self.fill_from_center = enabled;
        self
    }

    /// Overrides the active track fill tint.
    #[must_use]
    pub fn accent_color(mut self, color: Hsla) -> Self {
        self.accent_color = Some(color);
        self
    }

    /// Sets the reset target used when double-clicking the slider track.
    #[must_use]
    pub fn default_value(mut self, value: f32) -> Self {
        if value.is_finite() {
            self.default_value = Some(value.clamp(0.0, 1.0));
        }
        self
    }

    /// Returns the current normalized value in `[0.0, 1.0]`.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Returns `true` while the user is actively dragging the thumb.
    pub fn is_dragging(&self) -> bool {
        self.is_dragging
    }

    /// Updates the displayed value silently (without emitting events).
    /// Ignored while [`Self::is_dragging`] is `true` to prevent external sync fighting pointer input.
    pub fn set_value(&mut self, value: f32, cx: &mut Context<Self>) {
        if self.is_dragging {
            return;
        }
        let next = if value.is_finite() { value } else { 0.0 }.clamp(0.0, 1.0);
        if (self.value - next).abs() > 1e-6 {
            self.value = next;
            cx.notify();
        }
    }

    fn set_value_emit(&mut self, value: f32, cx: &mut Context<Self>) {
        let next = if value.is_finite() { value } else { 0.0 }.clamp(0.0, 1.0);
        if (self.value - next).abs() > 1e-6 {
            self.value = next;
            cx.emit(SliderEvent::Change(next));
            cx.notify();
        }
    }

    fn update_proximity(
        &mut self,
        mouse_pos: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.is_dragging {
            return;
        }
        let target_p = smoothstep_proximity(mouse_pos, bounds);
        if (self.proximity - target_p).abs() > 0.005 {
            self.proximity = target_p;
            cx.notify();
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            if let Some(default_val) = self.default_value {
                self.is_dragging = false;
                self.value = default_val;
                cx.emit(SliderEvent::Change(default_val));
                cx.emit(SliderEvent::Commit(default_val));
                cx.notify();
                return;
            }
        }

        let bounds = self.track_bounds.lock().ok().and_then(|g| *g);
        let Some(bounds) = bounds else {
            return;
        };
        if f32::from(bounds.size.width) <= 0.0 {
            return;
        }

        self.is_dragging = true;
        self.proximity = 1.0;
        let t = normalized_x(event.position, bounds, 5.5);
        self.set_value_emit(t, cx);
        cx.notify();
    }

    fn handle_mouse_move_at(
        &mut self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_dragging || f32::from(bounds.size.width) <= 0.0 {
            return;
        }
        let t = normalized_x(position, bounds, 5.5);
        self.set_value_emit(t, cx);
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if self.is_dragging {
            self.is_dragging = false;
            cx.emit(SliderEvent::Commit(self.value));
            cx.notify();
        }
    }
}

impl Render for Slider {
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
                            if !this.is_dragging {
                                this.update_proximity(event.position, bounds, cx);
                            } else if event.pressed_button == Some(MouseButton::Left) {
                                this.handle_mouse_move_at(event.position, bounds, cx);
                            } else {
                                this.finish_drag(cx);
                                this.update_proximity(event.position, bounds, cx);
                            }
                        });
                    }
                });

                let entity = slider_entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if this.is_dragging {
                                this.finish_drag(cx);
                                this.update_proximity(event.position, bounds, cx);
                            }
                        });
                    }
                });

                let entity = slider_entity.clone();
                window.on_mouse_event(move |_event: &MouseExitEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        entity.update(cx, |this, cx| {
                            if !this.is_dragging && this.proximity > 0.0 {
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
        let p = if self.is_dragging { 1.0 } else { self.proximity };

        let track_h = 4.0 + 7.0 * p;
        let track_top = (22.0 - track_h) / 2.0;
        let r = track_h / 2.0;

        let track_bg = lerp_hsla(colors.track_background, colors.track_background_hover, p);
        let active_idle = self
            .accent_color
            .map(|c| c.opacity(0.75))
            .unwrap_or(colors.active_track_idle);
        let active_hover = self.accent_color.unwrap_or(colors.active_track);
        let active_bg = lerp_hsla(active_idle, active_hover, p);

        let (fill_start, fill_end) = match (self.fill_from_center, self.center_mark) {
            (true, Some(center)) => (self.value.min(center), self.value.max(center)),
            _ => (0.0, self.value),
        };

        let active_bar = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(fill_start))
            .ml(px(-r))
            .right(relative(1.0 - fill_end))
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

        if let Some(mark) = self.center_mark {
            inner = inner.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .bottom(px(-2.0))
                    .left(relative(mark))
                    .ml(px(-0.5))
                    .w(px(1.0))
                    .bg(cx.theme().colors().text_muted.opacity(0.45)),
            );
        }

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

            let mut knob = div()
                .id(format!("slider_knob_{entity_id:?}"))
                .absolute()
                .top_0()
                .left(relative(self.value))
                .ml(px(-r))
                .size(px(track_h))
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(colors.knob_border(self.is_dragging))
                .bg(if self.is_dragging {
                    colors.knob_background_active
                } else {
                    knob_bg
                })
                .when(!self.is_dragging, |el| el.border_color(knob_border))
                .hover(|s| {
                    s.border_color(colors.knob_border_hover)
                        .bg(colors.knob_background_hover)
                        .shadow_md()
                });

            if p > 0.5 {
                knob = knob.shadow_sm();
            }

            inner = inner.child(knob);
        }

        div()
            .id(format!("slider_root_{entity_id:?}"))
            .relative()
            .w_full()
            .h(px(22.0))
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
                    this.handle_mouse_down(event, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.finish_drag(cx);
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
