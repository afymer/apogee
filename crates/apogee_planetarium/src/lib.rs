use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    prelude::*,
    render::{
        RenderPlugin,
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
            WgpuWrapper,
        },
        settings::RenderCreation,
    },
};

#[derive(Default, Component, Clone, Copy)]
struct PlanetariumCameraMarker {}

pub struct PlanetariumRenderer {}

pub enum ViewportCommand {}

#[derive(Default)]
pub struct BevyBridgeState {
    pub is_playing: bool,
    pub is_dirty: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub pending_commands: Vec<ViewportCommand>,
}

impl BevyBridgeState {
    pub fn texture(&self) -> Option<&Arc<wgpu::TextureView>> {
        self.latest_texture.as_ref()
    }
}

#[derive(Clone)]
pub struct BevyBridge {
    state: Arc<(Mutex<BevyBridgeState>, Condvar)>,
    notify_ui_fn: Arc<dyn Fn() + Send + Sync>,
    shutdown: Arc<AtomicBool>,
}

impl BevyBridge {
    pub fn new(
        notify_ui: impl Fn() + Send + Sync + 'static,
    ) -> (Self, Arc<(Mutex<BevyBridgeState>, Condvar)>) {
        let state = Arc::new((
            Mutex::new(BevyBridgeState {
                is_dirty: true,
                is_playing: false,
                latest_texture: None,
                pending_commands: Vec::new(),
            }),
            Condvar::new(),
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let bridge = Self {
            state: state.clone(),
            notify_ui_fn: Arc::new(notify_ui),
            shutdown,
        };
        (bridge, state)
    }

    pub fn texture_view(&self) -> Option<Arc<wgpu::TextureView>> {
        let (lock, _) = &*self.state;
        lock.lock().ok()?.latest_texture.clone()
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn state(&self) -> Arc<(Mutex<BevyBridgeState>, Condvar)> {
        self.state.clone()
    }

    pub fn send_command(&self, cmd: ViewportCommand) {
        let (lock, cvar) = &*self.state;
        if let Ok(mut guard) = lock.lock() {
            guard.pending_commands.push(cmd);
            guard.is_dirty = true;
            cvar.notify_one();
        }
    }

    pub fn notify_frame_ready(&self) {
        (self.notify_ui_fn)();
    }
}

impl PlanetariumRenderer {
    pub fn spawn(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter: wgpu::Adapter,
        instance: wgpu::Instance,
        initial_width: u32,
        initial_height: u32,
        bridge: BevyBridge,
        shared_state: Arc<(Mutex<BevyBridgeState>, Condvar)>,
    ) -> Result<JoinHandle<()>, std::io::Error> {
        std::thread::Builder::new()
            .name("Bevy renderer".to_string())
            .spawn(move || {
                let mut app = App::new();

                let width = initial_width.max(1);
                let height = initial_height.max(1);
                let adapter_info = adapter.get_info();

                app.add_plugins(
                    DefaultPlugins
                        .set(WindowPlugin {
                            primary_window: None,
                            exit_condition: bevy::window::ExitCondition::DontExit,
                            ..default()
                        })
                        .set(RenderPlugin {
                            render_creation: RenderCreation::manual(
                                RenderDevice::from(device),
                                RenderQueue(Arc::new(WgpuWrapper::new(queue))),
                                RenderAdapterInfo(WgpuWrapper::new(adapter_info)),
                                RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
                                RenderInstance(Arc::new(WgpuWrapper::new(instance))),
                            ),
                            ..default()
                        })
                        .disable::<bevy::winit::WinitPlugin>(),
                );

                let size = Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                };
                let mut render_image = Image::new_fill(
                    size,
                    TextureDimension::D2,
                    &[255, 0, 0, 255],
                    TextureFormat::Bgra8UnormSrgb,
                    RenderAssetUsages::default(),
                );
                render_image.texture_descriptor.usage =
                    TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
                let render_target_handle = app
                    .world_mut()
                    .resource_mut::<Assets<Image>>()
                    .add(render_image);

                let target_clone = render_target_handle.clone();
                app.add_systems(Startup, move |mut commands: Commands| {
                    commands.spawn((
                        Camera3d::default(),
                        RenderTarget::Image(target_clone.clone().into()),
                        Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::Z, Vec3::Y),
                        PlanetariumCameraMarker::default(),
                    ));
                });

                app.finish();
                app.cleanup();
                let (lock, cvar) = &*shared_state;

                let shutdown_flag = bridge.shutdown_flag();

                while !shutdown_flag.load(Ordering::Relaxed) {
                    let mut guard = lock.lock().unwrap();

                    let timeout: Option<Duration> = if guard.is_playing {
                        Some(Duration::from_millis(16))
                    } else if guard.is_dirty {
                        Some(Duration::ZERO)
                    } else {
                        None
                    };
                    guard = match timeout {
                        Some(duration) if duration > Duration::ZERO => {
                            cvar.wait_timeout(guard, duration).unwrap().0
                        }
                        Some(_) => guard,
                        None => cvar.wait(guard).unwrap(),
                    };
                    if shutdown_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    app.update();
                    if let Some(render_sub_app) = app.get_sub_app(bevy::render::RenderApp) {
                        let render_world = render_sub_app.world();
                        if let Some(render_assets) = render_world
                            .get_resource::<bevy::render::render_asset::RenderAssets<
                            bevy::render::texture::GpuImage,
                        >>() {
                            if let Some(gpu_image) = render_assets.get(&render_target_handle) {
                                let wgpu_texture: &wgpu::Texture = &gpu_image.texture;
                                let wgpu_view = wgpu_texture
                                    .create_view(&wgpu::TextureViewDescriptor::default());
                                let mut g = lock.lock().unwrap();
                                g.latest_texture = Some(Arc::new(wgpu_view));
                            }
                        }
                    }

                    bridge.notify_frame_ready();
                }
            })
    }
}
