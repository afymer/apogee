use std::{cell::Cell, rc::Rc, sync::Arc};

use apogee_planetarium::{BevyBridge, PlanetariumRenderer, ViewportCommand};
use futures::StreamExt;
use gpui::{App, Context, Entity, Window, WindowOptions, div, prelude::*, px, rgb};
use gpui_platform::application;
use tracing::info;

struct ApogeeApp {
    viewport: Entity<BevyViewportView>,
}

pub struct BevyViewportView {
    bridge: BevyBridge,
    cached_texture: Option<Arc<wgpu::TextureView>>,
    last_size: Rc<Cell<Option<(u32, u32)>>>,
}

impl BevyViewportView {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        initial_width: u32,
        initial_height: u32,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();

            let notify_ui = {
                let tx = tx.clone();
                move || {
                    let _ = tx.unbounded_send(());
                }
            };

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
                last_size: Rc::new(Cell::new(None)),
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
        self.cached_texture = self.bridge.texture_view();
        let bridge = self.bridge.clone();
        let last_size = self.last_size.clone();
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
                        if width > 0 && height > 0 && last_size.get() != Some((width, height)) {
                            // info!("Resizing to {},{}", width, height);
                            last_size.set(Some((width, height)));
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

impl Render for ApogeeApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_row().children([
            div()
                .w(px(200.))
                .h_full()
                .bg(rgb(0xaa2222))
                .into_any_element(),
            div()
                .h_full()
                .flex_1()
                .bg(rgb(0x111111))
                .child(self.viewport.clone())
                .into_any_element(),
            div()
                .w(px(200.))
                .h_full()
                .bg(rgb(0xaa2222))
                .into_any_element(),
        ])
        // .child(self.viewport.clone())
    }
}

impl ApogeeApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            viewport: BevyViewportView::build(window, cx, 64, 64),
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
