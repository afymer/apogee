use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use futures::StreamExt;
use glam::{Vec2, vec2};
use gpui::{
    App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollDelta,
    ScrollWheelEvent, Window, div, prelude::*, px, rgb,
};
use tracing::error;
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use components::{ActiveKnob, DoubleSlider, DoubleSliderEvent};

use crate::{
    bridge::RawViewerBridge,
    histogram::HistogramWidget,
    raw_image::{RawError, RawImage},
    types::ViewTransform,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragMode {
    None,
    Panning {
        last_pos: Point<Pixels>,
    },
    Zooming {
        start_pos: Point<Pixels>,
        anchor_pan: Vec2,
        start_zoom: f32,
        start_pan: Vec2,
    },
}

pub struct RawViewerPanel {
    bridge: RawViewerBridge,
    slider: Entity<DoubleSlider>,
    histogram: Entity<HistogramWidget>,
    drag_mode: DragMode,
    viewport_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    cached_texture: Option<Arc<wgpu::TextureView>>,
    renderer_handle: Option<JoinHandle<()>>,
    focus_handle: FocusHandle,
}

impl EventEmitter<PanelEvent> for RawViewerPanel {}

impl RawViewerPanel {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        initial_width: u32,
        initial_height: u32,
    ) -> Entity<Self> {
        let (tx, mut rx) = futures::channel::mpsc::channel::<()>(1);
        let tx = std::sync::Mutex::new(tx);

        let notify_ui = {
            move || {
                if let Ok(mut tx) = tx.lock() {
                    let _ = tx.try_send(());
                }
            }
        };

        let bridge = RawViewerBridge::new(notify_ui);

        let device = (*window.wgpu_device().expect("wgpu device not available")).clone();
        let queue = (*window.wgpu_queue().expect("wgpu queue not available")).clone();

        let renderer_handle = bridge
            .spawn_renderer(device, queue, initial_width, initial_height)
            .expect("Failed to spawn RawViewerRenderer thread");

        let (min_range, max_range) = bridge.display_range();
        let slider = DoubleSlider::build(min_range, max_range, cx);
        let histogram = HistogramWidget::build(bridge.clone(), cx);

        cx.new(|cx| {
            cx.spawn(async move |this, cx| {
                while let Some(()) = rx.next().await {
                    if this
                        .update(cx, |this: &mut Self, cx| {
                            if this.slider.read(cx).active_knob() == ActiveKnob::None {
                                let (range_min, range_max) = this.bridge.display_range();
                                this.slider.update(cx, |slider, cx| {
                                    slider.set_range(range_min, range_max, cx);
                                });
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            cx.subscribe(
                &slider,
                |this, _slider, event: &DoubleSliderEvent, _cx| match *event {
                    DoubleSliderEvent::Change { min, max } => {
                        this.bridge.set_display_range(min, max);
                    }
                },
            )
            .detach();

            cx.observe(&slider, |_this, _slider, cx| {
                cx.notify();
            })
            .detach();

            cx.observe(&histogram, |_this, _histogram, cx| {
                cx.notify();
            })
            .detach();

            Self {
                bridge,
                slider,
                histogram,
                drag_mode: DragMode::None,
                viewport_bounds: Arc::new(Mutex::new(None)),
                cached_texture: None,
                renderer_handle: Some(renderer_handle),
                focus_handle: cx.focus_handle(),
            }
        })
    }

    pub fn bridge(&self) -> &RawViewerBridge {
        &self.bridge
    }

    pub fn set_image(&self, image: RawImage) {
        self.bridge.set_image(image);
    }

    pub fn load_from_memory(&self, buffer: &[u8]) -> Result<(), RawError> {
        self.bridge.load_from_memory(buffer)
    }

    pub fn toggle_demosaic(&self) {
        self.bridge.toggle_demosaic();
    }

    fn screen_to_pan_space(&self, screen_pos: Point<Pixels>, bounds: Bounds<Pixels>) -> Vec2 {
        let viewer_w = f32::from(bounds.size.width).max(1.0);
        let viewer_h = f32::from(bounds.size.height).max(1.0);
        let screen_aspect = viewer_w / viewer_h;

        let (raw_w, raw_h) = self
            .bridge
            .image_size()
            .unwrap_or((viewer_w as u32, viewer_h as u32));
        let img_aspect = (raw_w as f32) / (raw_h as f32).max(1.0);

        let mouse_x = f32::from(screen_pos.x - bounds.origin.x);
        let mouse_y = f32::from(screen_pos.y - bounds.origin.y);

        let uv_x = (mouse_x / viewer_w).clamp(0.0, 1.0);
        let uv_y = (mouse_y / viewer_h).clamp(0.0, 1.0);

        let mut pan_coord = vec2(uv_x - 0.5, uv_y - 0.5);

        if screen_aspect > img_aspect {
            pan_coord.x *= screen_aspect / img_aspect;
        } else {
            pan_coord.y *= img_aspect / screen_aspect;
        }

        pan_coord
    }

    fn screen_delta_to_pan_delta(&self, delta: Vec2, bounds: Bounds<Pixels>) -> Vec2 {
        let viewer_w = f32::from(bounds.size.width).max(1.0);
        let viewer_h = f32::from(bounds.size.height).max(1.0);
        let screen_aspect = viewer_w / viewer_h;

        let (raw_w, raw_h) = self
            .bridge
            .image_size()
            .unwrap_or((viewer_w as u32, viewer_h as u32));
        let img_aspect = (raw_w as f32) / (raw_h as f32).max(1.0);

        let mut delta_pan = vec2(delta.x / viewer_w, delta.y / viewer_h);

        if screen_aspect > img_aspect {
            delta_pan.x *= screen_aspect / img_aspect;
        } else {
            delta_pan.y *= img_aspect / screen_aspect;
        }

        delta_pan
    }

    pub fn set_one_to_one(&mut self) {
        self.bridge.set_one_to_one();
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if event.click_count == 2 {
            self.bridge.reset_view();
            cx.notify();
            return;
        }

        if event.button != MouseButton::Left && event.button != MouseButton::Middle {
            return;
        }

        let bounds = self.viewport_bounds.lock().ok().and_then(|g| *g);
        let Some(bounds) = bounds else {
            return;
        };

        let is_zoom_modifier = event.modifiers.control;

        if is_zoom_modifier && event.button == MouseButton::Left {
            let anchor_pan = self.screen_to_pan_space(event.position, bounds);
            let (pan, zoom) = self.bridge.pan_zoom();
            self.drag_mode = DragMode::Zooming {
                start_pos: event.position,
                anchor_pan,
                start_zoom: zoom,
                start_pan: pan,
            };
        } else {
            self.drag_mode = DragMode::Panning {
                last_pos: event.position,
            };
        }
        cx.notify();
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            if self.drag_mode != DragMode::None {
                self.drag_mode = DragMode::None;
                cx.notify();
            }
            return;
        }

        let bounds = self.viewport_bounds.lock().ok().and_then(|g| *g);
        let Some(bounds) = bounds else {
            return;
        };

        match self.drag_mode {
            DragMode::Panning { last_pos } => {
                let dx = f32::from(event.position.x - last_pos.x);
                let dy = f32::from(event.position.y - last_pos.y);
                let delta_pan = self.screen_delta_to_pan_delta(vec2(dx, dy), bounds);
                let (pan, zoom) = self.bridge.pan_zoom();
                self.bridge.set_pan_zoom(pan + delta_pan, zoom);
                self.drag_mode = DragMode::Panning {
                    last_pos: event.position,
                };
                cx.notify();
            }
            DragMode::Zooming {
                start_pos,
                anchor_pan,
                start_zoom,
                start_pan,
            } => {
                let dx = f32::from(event.position.x - start_pos.x);
                let dy = f32::from(event.position.y - start_pos.y);

                let drag_delta = (dy + dx) * 0.008;
                let zoom_factor = (2.0f32).powf(drag_delta);
                let new_zoom = (start_zoom * zoom_factor)
                    .clamp(ViewTransform::MIN_ZOOM, ViewTransform::MAX_ZOOM);
                let ratio = new_zoom / start_zoom;
                let new_pan = anchor_pan - (anchor_pan - start_pan) * ratio;
                self.bridge.set_pan_zoom(new_pan, new_zoom);
                cx.notify();
            }
            DragMode::None => {}
        }
    }

    fn handle_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.drag_mode != DragMode::None {
            self.drag_mode = DragMode::None;
            cx.notify();
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let bounds = self.viewport_bounds.lock().ok().and_then(|g| *g);
        let Some(bounds) = bounds else {
            return;
        };

        let delta_lines = match event.delta {
            ScrollDelta::Lines(pt) => pt.y,
            ScrollDelta::Pixels(pt) => f32::from(pt.y) / 20.0,
        };

        if delta_lines.abs() < 1e-4 {
            return;
        }

        let factor = (1.15f32).powf(delta_lines.clamp(-5.0, 5.0));
        let anchor_pan = self.screen_to_pan_space(event.position, bounds);
        let (pan, zoom) = self.bridge.pan_zoom();
        let new_zoom = (zoom * factor).clamp(ViewTransform::MIN_ZOOM, ViewTransform::MAX_ZOOM);
        let ratio = new_zoom / zoom;
        let new_pan = anchor_pan - (anchor_pan - pan) * ratio;
        self.bridge.set_pan_zoom(new_pan, new_zoom);
        cx.notify();
    }
}

impl Render for RawViewerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cached_texture = self.bridge.texture_view();
        let bridge = self.bridge.clone();
        let last_size = self.bridge.last_size();
        let (_, current_zoom) = self.bridge.pan_zoom();
        let display_zoom_pct = if let (Some((raw_w, raw_h)), Some((view_w, view_h))) =
            (self.bridge.image_size(), last_size)
        {
            let view_w = (view_w as f32).max(1.0);
            let view_h = (view_h as f32).max(1.0);
            let raw_w = (raw_w as f32).max(1.0);
            let raw_h = (raw_h as f32).max(1.0);
            let screen_aspect = view_w / view_h;
            let img_aspect = raw_w / raw_h;
            let fitted_w = if screen_aspect > img_aspect {
                view_h * img_aspect
            } else {
                view_w
            };
            current_zoom * (fitted_w / raw_w) * 100.0
        } else {
            current_zoom * 100.0
        };

        let bounds_store = self.viewport_bounds.clone();
        let resize_canvas = gpui::canvas(
            move |bounds, window, _cx| {
                if let Ok(mut lock) = bounds_store.lock() {
                    *lock = Some(bounds);
                }
                let scale = window.scale_factor();
                let width = (f32::from(bounds.size.width) * scale).round() as u32;
                let height = (f32::from(bounds.size.height) * scale).round() as u32;
                if width > 0 && height > 0 && last_size != Some((width, height)) {
                    bridge.resize(width, height);
                }
            },
            |_bounds, (), _window, _cx| {},
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        let cursor = match self.drag_mode {
            DragMode::Panning { .. } => CursorStyle::ClosedHand,
            DragMode::Zooming { .. } => CursorStyle::Crosshair,
            DragMode::None => CursorStyle::OpenHand,
        };

        let viewport = div()
            .id("raw_viewer_viewport")
            .size_full()
            .relative()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.handle_mouse_down(event, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.handle_mouse_down(event, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.handle_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.handle_scroll_wheel(event, cx);
            }))
            .child(resize_canvas)
            .children(
                self.cached_texture
                    .as_ref()
                    .map(|view| gpui::surface(view.clone()).size_full()),
            );

        let mut root = div()
            .size_full()
            .bg(rgb(0x0a0a0c))
            .relative()
            .child(viewport)
            .child(
                h_flex()
                    .absolute()
                    .top_3()
                    .right_3()
                    .gap_1p5()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(cx.theme().colors().panel_background)
                            .border_1()
                            .border_color(cx.theme().colors().border_variant)
                            .child(
                                Label::new(format!("{display_zoom_pct:.0}%"))
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            ),
                    )
                    .child(
                        Button::new("fit_view_btn", "Fit")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.bridge.reset_view();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("one_to_one_btn", "1:1")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_one_to_one();
                                cx.notify();
                            })),
                    ),
            );

        let (range_min, range_max) = self.bridge.display_range();
        let overlay = div()
            .absolute()
            .bottom_3()
            .left_3()
            .w(px(280.0))
            .p_2()
            .gap_1p5()
            .rounded_md()
            .bg(cx.theme().colors().elevated_surface_background)
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Label::new("Stretch")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_sm()
                                    .bg(cx.theme().colors().element_background)
                                    .child(
                                        Label::new(format!("{:.3} - {:.3}", range_min, range_max))
                                            .size(LabelSize::Small)
                                            .color(Color::Default),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("reset_stretch_btn", "Reset")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.bridge.reset_display_range();
                                this.slider.update(cx, |slider, cx| {
                                    slider.set_range(0.0, 1.0, cx);
                                });
                                cx.notify();
                            })),
                    ),
            )
            .child(self.slider.clone());

        root = root.child(overlay);

        let histogram_overlay = div()
            .absolute()
            .bottom_3()
            .right_3()
            .child(self.histogram.clone());

        root = root.child(histogram_overlay);

        root
    }
}

impl Focusable for RawViewerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for RawViewerPanel {
    fn persistent_name() -> &'static str {
        "raw_viewer_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "RAW Viewer".into()
    }

    fn position(&self, _cx: &App) -> workspace::dock::DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Camera)
    }
}

impl Drop for RawViewerPanel {
    fn drop(&mut self) {
        self.cached_texture = None;
        self.bridge.shutdown();
        if let Some(handle) = self.renderer_handle.take() {
            if let Err(e) = handle.join() {
                error!("Failed to join RawViewerRenderer thread: {e:?}");
            }
        }
    }
}
