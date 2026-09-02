use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    color::palettes::css::RED,
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
struct PlanetariumCameraMarker;

pub struct PlanetariumRenderer;

pub enum ViewportCommand {
    Resize(u32, u32),
}

#[derive(Default)]
pub struct BevyBridgeState {
    pub is_playing: bool,
    pub is_dirty: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub pending_commands: Vec<ViewportCommand>,
}

impl BevyBridgeState {
    pub fn texture(&self) -> Option<&Arc<wgpu::TextureView>> {
        self.latest_texture.as_ref()
    }
}

struct BridgeInner {
    state: Mutex<BevyBridgeState>,
    cvar: Condvar,
    shutdown: AtomicBool,
    notify_ui_fn: Box<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub struct BevyBridge {
    inner: Arc<BridgeInner>,
}

impl BevyBridge {
    pub fn new(notify_ui: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(BridgeInner {
                state: Mutex::new(BevyBridgeState {
                    is_dirty: true,
                    is_playing: false,
                    latest_texture: None,
                    last_size: None,
                    pending_commands: Vec::new(),
                }),
                cvar: Condvar::new(),
                shutdown: AtomicBool::new(false),
                notify_ui_fn: Box::new(notify_ui),
            }),
        }
    }

    pub fn texture_view(&self) -> Option<Arc<wgpu::TextureView>> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.latest_texture.clone()
    }

    pub fn last_size(&self) -> Option<(u32, u32)> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.last_size
    }

    pub fn send_command(&self, cmd: ViewportCommand) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.pending_commands.push(cmd);
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn request_redraw(&self) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn notify_frame_ready(&self) {
        (self.inner.notify_ui_fn)();
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.inner.cvar.notify_all();
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
                            synchronous_pipeline_compilation: true,
                            ..default()
                        })
                        .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(), // TODO: try to make pipelined rendering work
                );

                let size = Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                };
                let mut render_image = Image::new_fill(
                    size,
                    TextureDimension::D2,
                    &[0, 0, 0, 255],
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
                app.add_systems(
                    Startup,
                    move |mut commands: Commands,
                          mut meshes: ResMut<Assets<Mesh>>,
                          mut materials: ResMut<Assets<StandardMaterial>>| {
                        commands.spawn((
                            Camera3d::default(),
                            RenderTarget::Image(target_clone.clone().into()),
                            Transform::from_xyz(4., 5., 3.).looking_at(Vec3::ZERO, Dir3::Y),
                            PlanetariumCameraMarker,
                        ));

                        commands.spawn((
                            DirectionalLight {
                                illuminance: 10_000.0,
                                ..Default::default()
                            },
                            Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
                        ));

                        commands.spawn((
                            Mesh3d(meshes.add(Capsule3d::default())),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: RED.into(),
                                ..Default::default()
                            })),
                            Transform::from_xyz(0., 0., 0.),
                        ));
                    },
                );

                app.finish();
                app.cleanup();

                while !bridge.inner.shutdown.load(Ordering::Acquire) {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if !guard.is_dirty
                        && !guard.is_playing
                        && !bridge.inner.shutdown.load(Ordering::Acquire)
                    {
                        guard = bridge
                            .inner
                            .cvar
                            .wait_while(guard, |s| {
                                !s.is_dirty
                                    && !s.is_playing
                                    && !bridge.inner.shutdown.load(Ordering::Acquire)
                            })
                            .unwrap_or_else(PoisonError::into_inner);
                    } else if guard.is_playing {
                        guard = bridge
                            .inner
                            .cvar
                            .wait_timeout(guard, Duration::from_millis(16))
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    if bridge.inner.shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    guard.is_dirty = false;
                    let commands = std::mem::take(&mut guard.pending_commands);
                    drop(guard);

                    let mut texture_resized = false;
                    for cmd in commands {
                        match cmd {
                            ViewportCommand::Resize(new_width, new_height) => {
                                let new_size = Extent3d {
                                    width: new_width.max(1),
                                    height: new_height.max(1),
                                    depth_or_array_layers: 1,
                                };
                                if let Some(mut images) =
                                    app.world_mut().get_resource_mut::<Assets<Image>>()
                                {
                                    if let Some(mut image) = images.get_mut(&render_target_handle) {
                                        if image.texture_descriptor.size != new_size {
                                            image.resize(new_size);
                                            texture_resized = true;
                                            let mut guard = bridge
                                                .inner
                                                .state
                                                .lock()
                                                .unwrap_or_else(PoisonError::into_inner);
                                            guard.last_size =
                                                Some((new_size.width, new_size.height));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    app.update();

                    if let Some(render_sub_app) = app.get_sub_app(bevy::render::RenderApp) {
                        let render_world = render_sub_app.world();
                        if let Some(render_assets) = render_world
                            .get_resource::<bevy::render::render_asset::RenderAssets<
                            bevy::render::texture::GpuImage,
                        >>() {
                            if let Some(gpu_image) = render_assets.get(&render_target_handle) {
                                let mut guard = bridge
                                    .inner
                                    .state
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner);
                                if guard.latest_texture.is_none() || texture_resized {
                                    let wgpu_texture: &wgpu::Texture = &gpu_image.texture;
                                    let wgpu_view = wgpu_texture
                                        .create_view(&wgpu::TextureViewDescriptor::default());
                                    guard.latest_texture = Some(Arc::new(wgpu_view));
                                }
                            }
                        }
                    }

                    bridge.notify_frame_ready();
                }

                let mut guard = bridge
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.latest_texture = None;
            })
    }
}
