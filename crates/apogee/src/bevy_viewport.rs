use std::{sync::Arc, thread::JoinHandle};

use futures::StreamExt;
use gpui::{App, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, rgb};
use planetarium::{BevyBridge, PlanetariumRenderer, ViewportCommand};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    workbench::Workbench,
};

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let viewport = PlanetariumPanel::build(window, cx, 64, 64);
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
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();

            let notify_ui = {
                move || {
                    let _ = tx.unbounded_send(());
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
            }
        })
    }
}

impl Render for PlanetariumPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.cached_texture = self.bridge.texture_view();
        let bridge = self.bridge.clone();
        let last_size = self.bridge.last_size();
        div()
            .size_full()
            .bg(rgb(0x765432))
            .relative()
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
