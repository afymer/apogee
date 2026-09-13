use std::{sync::Arc, thread::JoinHandle};

use futures::StreamExt;
use gpui::{
    App, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, Window, div, prelude::*, rgb,
};
use planetarium::{BevyBridge, PlanetariumRenderer, ViewMode, ViewportCommand};
use theme::ActiveTheme;
use ui::{
    Button, ButtonCommon, ButtonStyle, Clickable, Color, Label, LabelCommon, LabelSize, h_flex,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    workbench::Workbench,
};

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let viewport = PlanetariumPanel::build(window, cx, 1024, 1024);
            cx.observe(&viewport, |_this, _viewport, cx| {
                cx.notify();
            })
            .detach();
            workbench.add_panel(viewport, window, cx);
        }
    })
    .detach();
}

pub struct PlanetariumPanel {
    bridge: BevyBridge,
    cached_texture: Option<Arc<wgpu::TextureView>>,
    renderer_handle: Option<JoinHandle<()>>,
    focus_handle: FocusHandle,
    is_dragging: bool,
    last_mouse_pos: Option<Point<Pixels>>,
}

impl EventEmitter<PanelEvent> for PlanetariumPanel {}

impl PlanetariumPanel {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        initial_width: u32,
        initial_height: u32,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let (tx, mut rx) = futures::channel::mpsc::channel::<()>(1);
            let tx = std::sync::Mutex::new(tx);

            let notify_ui = {
                move || {
                    if let Ok(mut tx) = tx.try_lock() {
                        let _ = tx.try_send(());
                    }
                }
            };

            let bridge = BevyBridge::new(notify_ui);

            let device = (*window.wgpu_device().expect("wgpu device not available")).clone();
            let queue = (*window.wgpu_queue().expect("wgpu queue not available")).clone();
            let adapter = (*window.wgpu_adapter().expect("wgpu adapter not available")).clone();
            let instance = (*window.wgpu_instance().expect("wgpu instance not available")).clone();

            let renderer_handle = PlanetariumRenderer::spawn(
                device,
                queue,
                adapter,
                instance,
                initial_width,
                initial_height,
                bridge.clone(),
            )
            .expect("Failed to spawn PlanetariumRenderer thread");

            cx.spawn(async move |this, cx| {
                while let Some(()) = rx.next().await {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();

            Self {
                bridge,
                cached_texture: None,
                renderer_handle: Some(renderer_handle),
                focus_handle: cx.focus_handle(),
                is_dragging: false,
                last_mouse_pos: None,
            }
        })
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, _cx: &mut Context<Self>) {
        self.is_dragging = true;
        self.last_mouse_pos = Some(event.position);
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.is_dragging {
            return;
        }

        let last_pos = self.last_mouse_pos.unwrap_or(event.position);
        let dx = f32::from(event.position.x - last_pos.x) as f64;
        let dy = f32::from(event.position.y - last_pos.y) as f64;

        // dx > 0 when dragging right
        // dy > 0 when dragging down
        self.bridge.pan(dx, dy);
        self.last_mouse_pos = Some(event.position);
        cx.notify();
    }

    fn handle_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.is_dragging {
            self.is_dragging = false;
            self.last_mouse_pos = None;
            cx.notify();
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta_lines = match event.delta {
            ScrollDelta::Lines(pt) => pt.y as f64,
            ScrollDelta::Pixels(pt) => (f32::from(pt.y) / 20.0) as f64,
        };

        if delta_lines.abs() > 1e-4 {
            self.bridge.zoom(delta_lines);
            cx.notify();
        }
    }

    fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        let current = self.bridge.view_info().view_mode;
        let next = match current {
            ViewMode::Horizontal => ViewMode::Equatorial,
            ViewMode::Equatorial => ViewMode::Horizontal,
        };
        self.bridge.set_view_mode(next);
        cx.notify();
    }
}

impl Render for PlanetariumPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.bridge.snapshot();
        self.cached_texture = snapshot.texture_view;
        let bridge = self.bridge.clone();
        let last_size = snapshot.last_size;
        let view_info = snapshot.view_info;

        let cursor = if self.is_dragging {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };

        let fov_text = if view_info.fov_deg >= 1.0 {
            format!("FOV: {:.1}°", view_info.fov_deg)
        } else if view_info.fov_deg * 60.0 >= 1.0 {
            format!("FOV: {:.1}'", view_info.fov_deg * 60.0)
        } else {
            format!("FOV: {:.1}\"", view_info.fov_deg * 3600.0)
        };

        let altaz_text = format!(
            "Az: {:.1}°  Alt: {:+.1}°",
            view_info.center_az_deg, view_info.center_alt_deg
        );

        let ra_hours = view_info.center_ra_deg / 15.0;
        let h = ra_hours.floor() as u32;
        let m = ((ra_hours - f64::from(h)) * 60.0).floor() as u32;
        let dec = view_info.center_dec_deg;
        let sign = if dec >= 0.0 { '+' } else { '-' };
        let abs_dec = dec.abs();
        let d = abs_dec.floor() as u32;
        let dm = ((abs_dec - f64::from(d)) * 60.0).floor() as u32;
        let eq_text = format!("RA: {:02}h {:02}m  Dec: {}{:02}° {:02}'", h, m, sign, d, dm);

        let mode_label = match view_info.view_mode {
            ViewMode::Horizontal => "Alt/Az",
            ViewMode::Equatorial => "RA/Dec",
        };

        div()
            .size_full()
            .bg(rgb(0x000000))
            .relative()
            .cursor(cursor)
            .on_mouse_down(
                MouseButton::Left,
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
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.handle_scroll_wheel(event, cx);
            }))
            .child(
                gpui::canvas(
                    move |bounds, window, _cx| {
                        let scale = window.scale_factor();
                        let width = (f32::from(bounds.size.width) * scale).round() as u32;
                        let height = (f32::from(bounds.size.height) * scale).round() as u32;
                        if width > 0 && height > 0 && last_size != Some((width, height)) {
                            bridge.send_command(ViewportCommand::Resize(width, height));
                        }
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .size_full()
                .absolute()
                .top_0()
                .left_0(),
            )
            .children(
                self.cached_texture
                    .as_ref()
                    .map(|view| gpui::surface(view.clone()).size_full()),
            )
            // Temporary HUD. TODO: move it to the toolbar
            .child(
                h_flex()
                    .absolute()
                    .top_3()
                    .right_3()
                    .gap_2()
                    .items_center()
                    .p_1p5()
                    .rounded_md()
                    .bg(cx
                        .theme()
                        .colors()
                        .elevated_surface_background
                        .opacity(0.85))
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .shadow_md()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .child(
                                Label::new(fov_text)
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .child(Label::new(altaz_text).size(LabelSize::Small).color(
                                if view_info.view_mode == ViewMode::Horizontal {
                                    Color::Default
                                } else {
                                    Color::Muted
                                },
                            )),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .child(Label::new(eq_text).size(LabelSize::Small).color(
                                if view_info.view_mode == ViewMode::Equatorial {
                                    Color::Default
                                } else {
                                    Color::Muted
                                },
                            )),
                    )
                    .child(
                        Button::new("view_mode_toggle_btn", mode_label)
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_view_mode(cx);
                            })),
                    ),
            )
    }
}

impl Focusable for PlanetariumPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PlanetariumPanel {
    fn persistent_name() -> &'static str {
        "planetarium_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Planetarium".into()
    }

    fn position(&self, _cx: &App) -> workspace::dock::DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Explore)
    }
}

impl Drop for PlanetariumPanel {
    fn drop(&mut self) {
        self.cached_texture = None;
        self.bridge.shutdown();
        if let Some(handle) = self.renderer_handle.take() {
            if let Err(e) = handle.join() {
                tracing::error!("Failed to join PlanetariumRenderer thread: {e:?}");
            }
        }
    }
}
