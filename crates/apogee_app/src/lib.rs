use std::sync::Arc;

use apogee_planetarium::{BevyBridge, PlanetariumRenderer, ViewportCommand};
use gpui::{App, Context, Entity, Window, WindowOptions, div, prelude::*, px, rgb};
use gpui_platform::application;

struct ApogeeApp {
    viewport: Entity<BevyViewportView>,
}

pub struct BevyViewportView {
    bridge: BevyBridge,
    cached_texture: Option<Arc<wgpu::TextureView>>,
}

impl BevyViewportView {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        initial_width: u32,
        initial_height: u32,
    ) -> Entity<Self> {
        cx.new(|_cx| {
            let notify_ui = || {};

            let (bridge, shared_state) = BevyBridge::new(notify_ui);

            let device = (*window.wgpu_device().expect("wgpu device not available")).clone();
            let queue = (*window.wgpu_queue().expect("wgpu queue not available")).clone();
            let adapter = (*window.wgpu_adapter().expect("wgpu adapter not available")).clone();
            let instance = (*window.wgpu_instance().expect("wgpu instance not available")).clone();

            PlanetariumRenderer::spawn(
                device,
                queue,
                adapter,
                instance,
                initial_width,
                initial_height,
                bridge.clone(),
                shared_state,
            )
            .expect("Failed to spawn PlanetariumRenderer thread");

            Self {
                bridge,
                cached_texture: None,
            }
        })
    }

    pub fn send_command(&mut self, cmd: ViewportCommand) {
        let (lock, cvar) = &*self.bridge.state();
        let mut state = lock.lock().unwrap();
        state.pending_commands.push(cmd);
        cvar.notify_one();
    }
}

impl Render for BevyViewportView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Release the lock while drawing
        {
            self.cached_texture = self.bridge.texture_view();
        }
        if let Some(ref view) = self.cached_texture {
            gpui::surface(view.clone())
                .size(px(400.0))
                .rounded(px(10.))
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }
}

impl Render for ApogeeApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0x226622))
            .child(self.viewport.clone())
    }
}

impl ApogeeApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            viewport: BevyViewportView::build(window, cx, 400, 400),
        }
    }
}

pub fn run() {
    application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            // let device: wgpu::Device = (*window.wgpu_device().unwrap()).clone();
            // let queue: wgpu::Queue = (*window.wgpu_queue().unwrap()).clone();
            // let adapter: wgpu::Adapter = (*window.wgpu_adapter().unwrap()).clone();
            // let instance: wgpu::Instance = (*window.wgpu_instance().unwrap()).clone();

            cx.new(|cx| {
                let apogee_app = ApogeeApp::new(window, cx);
                // ApogeeApp::new()
                apogee_app
            })
        })
        .unwrap();
    });
}
