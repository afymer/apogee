pub mod camera;
pub mod grids;
pub mod material;
pub mod viewport;

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
    camera::{RenderTarget, visibility::NoFrustumCulling},
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

pub use camera::{PlanetariumCamera, PlanetariumViewInfo, ViewMode};
use grids::{create_altaz_grid_mesh, create_equatorial_grid_mesh, create_horizon_mesh};
pub use material::{CelestialLineMaterial, CelestialLineUniforms, init_celestial_materials};

pub use crate::viewport::ViewportCommand;

#[derive(Default, Component, Clone, Copy)]
struct PlanetariumCameraMarker;

pub struct PlanetariumRenderer;

#[derive(Clone, Debug)]
pub struct BridgeSnapshot {
    pub texture_view: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub view_info: PlanetariumViewInfo,
}

#[derive(Default)]
pub struct BevyBridgeState {
    pub is_playing: bool,
    pub is_dirty: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub view_info: PlanetariumViewInfo,
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

#[derive(Clone, Resource)]
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
                    view_info: PlanetariumViewInfo::default(),
                    pending_commands: Vec::new(),
                }),
                cvar: Condvar::new(),
                shutdown: AtomicBool::new(false),
                notify_ui_fn: Box::new(notify_ui),
            }),
        }
    }

    pub fn snapshot(&self) -> BridgeSnapshot {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        BridgeSnapshot {
            texture_view: guard.latest_texture.clone(),
            last_size: guard.last_size,
            view_info: guard.view_info,
        }
    }

    pub fn texture_view(&self) -> Option<Arc<wgpu::TextureView>> {
        self.snapshot().texture_view
    }

    pub fn last_size(&self) -> Option<(u32, u32)> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.last_size
    }

    pub fn view_info(&self) -> PlanetariumViewInfo {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.view_info
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

    pub fn pan(&self, delta_x: f64, delta_y: f64) {
        self.send_command(ViewportCommand::Pan { delta_x, delta_y });
    }

    pub fn zoom(&self, delta: f64) {
        self.send_command(ViewportCommand::Zoom { delta });
    }

    pub fn set_view_mode(&self, mode: ViewMode) {
        self.send_command(ViewportCommand::SetViewMode(mode));
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

#[derive(Resource, Clone)]
struct CelestialHandles {
    horizon_material: Handle<CelestialLineMaterial>,
    altaz_material: Handle<CelestialLineMaterial>,
    eq_material: Handle<CelestialLineMaterial>,
    horizon_mesh: Handle<Mesh>,
    altaz_mesh: Handle<Mesh>,
    eq_mesh: Handle<Mesh>,
}

#[derive(Resource, Clone, Copy)]
struct ViewportDimensions {
    width: u32,
    height: u32,
}

struct GridMeshState {
    level: Option<u32>,
    fov_y: f64,
    yaw: f64,
    pitch: f64,
    is_zoomed_in: bool,
    view_mode: ViewMode,
    mesh_rotation: glam::DMat3,
}

impl Default for GridMeshState {
    fn default() -> Self {
        Self {
            level: None,
            fov_y: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            is_zoomed_in: false,
            view_mode: ViewMode::Horizontal,
            mesh_rotation: PlanetariumCamera::default().camera_rotation_matrix(),
        }
    }
}

#[derive(Resource, Default)]
struct PendingCommands(Vec<ViewportCommand>);

#[derive(Resource, Default)]
struct CameraSyncState {
    view_info: PlanetariumViewInfo,
    is_interpolating: bool,
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn camera_movement_and_interpolation_system(
    time: Res<Time>,
    mut pending_commands: ResMut<PendingCommands>,
    mut camera_query: Query<&mut PlanetariumCamera>,
    mut camera_sync: ResMut<CameraSyncState>,
    dims: Res<ViewportDimensions>,
    handles: Res<CelestialHandles>,
    mut materials: ResMut<Assets<CelestialLineMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut grid_state: Local<GridMeshState>,
    mut is_initialized: Local<bool>,
) {
    let Ok(mut camera) = camera_query.single_mut() else {
        return;
    };

    let mut moved = false;
    for cmd in pending_commands.0.drain(..) {
        match cmd {
            ViewportCommand::Pan { delta_x, delta_y } => {
                camera.pan(delta_x, delta_y);
                moved = true;
            }
            ViewportCommand::Zoom { delta } => {
                camera.zoom(delta);
                moved = true;
            }
            ViewportCommand::SetViewMode(mode) => {
                camera.set_view_mode(mode);
                moved = true;
            }
            ViewportCommand::Resize(_, _) => {
                moved = true;
            }
        }
    }

    let prev_yaw = camera.yaw;
    let prev_pitch = camera.pitch;
    let prev_fov = camera.fov_y;

    camera.update(time.delta().as_secs_f64());

    let yaw_diff = (camera.yaw - prev_yaw).abs();
    let pitch_diff = (camera.pitch - prev_pitch).abs();
    let fov_diff = (camera.fov_y - prev_fov).abs();
    let target_yaw_diff = (camera.target_yaw - camera.yaw).abs();
    let target_pitch_diff = (camera.target_pitch - camera.pitch).abs();
    let target_fov_diff = (camera.target_fov_y - camera.fov_y).abs();

    let is_interpolating = yaw_diff > 1e-8
        || pitch_diff > 1e-8
        || fov_diff > 1e-8
        || target_yaw_diff > 1e-7
        || target_pitch_diff > 1e-7
        || target_fov_diff > 1e-7;

    let is_first_run = !*is_initialized;
    if is_first_run {
        *is_initialized = true;
    }

    if moved || is_interpolating || is_first_run {
        let aspect = dims.width as f32 / dims.height as f32;
        let proj_params = camera.projection_params(aspect);

        let subdiv_count = 6;
        let (grid_level, minor_alpha) = camera.grid_zoom_factor(subdiv_count);

        let is_zoomed_in = camera.fov_y < std::f64::consts::FRAC_PI_3;
        let zoom_ratio_diff = (camera.fov_y - grid_state.fov_y).abs() / grid_state.fov_y.max(1e-8);
        let moved_significantly = is_zoomed_in
            && (zoom_ratio_diff > 0.15
                || (camera.yaw - grid_state.yaw).abs() > camera.fov_y * 0.1
                || (camera.pitch - grid_state.pitch).abs() > camera.fov_y * 0.1);

        let needs_rebuild = grid_state.level != Some(grid_level)
            || grid_state.is_zoomed_in != is_zoomed_in
            || grid_state.view_mode != camera.view_mode
            || moved_significantly;

        if needs_rebuild {
            let view_info = camera.view_info();
            let (bounds_altaz, bounds_eq) = if is_zoomed_in {
                let aspect_ratio = aspect as f64;
                let b_altaz = camera::compute_spherical_bounds(
                    view_info.center_az_deg.to_radians(),
                    view_info.center_alt_deg.to_radians(),
                    camera.fov_y,
                    aspect_ratio,
                );
                let b_eq = camera::compute_spherical_bounds(
                    view_info.center_ra_deg.to_radians(),
                    view_info.center_dec_deg.to_radians(),
                    camera.fov_y,
                    aspect_ratio,
                );
                (b_altaz, b_eq)
            } else {
                (
                    celestial::coords::SphericalRect::full_sphere(),
                    celestial::coords::SphericalRect::full_sphere(),
                )
            };

            let topo_to_cam = camera.topocentric_to_camera_matrix();
            let eq_to_cam = camera.icrs_to_camera_matrix();

            let new_altaz = grids::create_grid_mesh_mapped(
                &bounds_altaz,
                grid_level,
                subdiv_count,
                move |az, alt| {
                    let v_topo = grids::topocentric_spherical_to_enu(az, alt);
                    let v_cam = topo_to_cam.mul_vec3(v_topo);
                    Vec3::new(v_cam.x as f32, v_cam.y as f32, v_cam.z as f32)
                },
            );
            if let Some(mut mesh) = meshes.get_mut(&handles.altaz_mesh) {
                *mesh = new_altaz;
            }

            let new_horizon = grids::create_horizon_mesh(topo_to_cam);
            if let Some(mut mesh) = meshes.get_mut(&handles.horizon_mesh) {
                *mesh = new_horizon;
            }

            let new_eq = grids::create_grid_mesh_mapped(
                &bounds_eq,
                grid_level,
                subdiv_count,
                move |lon, lat| {
                    let v_eq = grids::canonical_spherical_to_cartesian(lon, lat);
                    let v_cam = eq_to_cam.mul_vec3(v_eq);
                    Vec3::new(v_cam.x as f32, v_cam.y as f32, v_cam.z as f32)
                },
            );
            if let Some(mut mesh) = meshes.get_mut(&handles.eq_mesh) {
                *mesh = new_eq;
            }

            grid_state.level = Some(grid_level);
            grid_state.fov_y = camera.fov_y;
            grid_state.yaw = camera.yaw;
            grid_state.pitch = camera.pitch;
            grid_state.is_zoomed_in = is_zoomed_in;
            grid_state.view_mode = camera.view_mode;
            grid_state.mesh_rotation = camera.camera_rotation_matrix();
        }

        let delta_rot = camera.camera_rotation_matrix() * grid_state.mesh_rotation.transpose();
        let delta_transform = glam::DMat4::from_mat3(delta_rot).as_mat4();

        if let Some(mut mat) = materials.get_mut(&handles.horizon_material) {
            mat.uniforms.transform = delta_transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = 1.0;
        }
        if let Some(mut mat) = materials.get_mut(&handles.altaz_material) {
            mat.uniforms.transform = delta_transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = minor_alpha;
        }
        if let Some(mut mat) = materials.get_mut(&handles.eq_material) {
            mat.uniforms.transform = delta_transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = minor_alpha;
        }

        camera_sync.view_info = camera.view_info();
        camera_sync.is_interpolating = is_interpolating;
    }
}

fn process_viewport_commands(
    commands: Vec<ViewportCommand>,
    app: &mut App,
    current_width: &mut u32,
    current_height: &mut u32,
    render_target_handle: &Handle<Image>,
    frames_remaining: &mut u32,
) -> Option<(u32, u32)> {
    let mut resized_size = None;
    let mut pending = Vec::new();
    for cmd in commands {
        match cmd {
            ViewportCommand::Resize(w, h) => {
                let (new_w, new_h) = (w.max(1), h.max(1));
                if *current_width != new_w || *current_height != new_h {
                    *current_width = new_w;
                    *current_height = new_h;
                    let new_size = Extent3d {
                        width: new_w,
                        height: new_h,
                        depth_or_array_layers: 1,
                    };
                    if let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() {
                        if let Some(mut image) = images.get_mut(render_target_handle) {
                            image.resize(new_size);
                            *frames_remaining = 3;
                        }
                        resized_size = Some((new_w, new_h));
                    }
                    if let Some(mut dims) = app.world_mut().get_resource_mut::<ViewportDimensions>()
                    {
                        dims.width = new_w;
                        dims.height = new_h;
                    }
                    pending.push(ViewportCommand::Resize(new_w, new_h));
                }
            }
            movement => {
                pending.push(movement);
            }
        }
    }
    if !pending.is_empty() {
        if let Some(mut queue) = app.world_mut().get_resource_mut::<PendingCommands>() {
            queue.0.extend(pending);
        }
    }
    resized_size
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

                {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    guard.last_size = Some((width, height));
                }

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

                init_celestial_materials(&mut app);

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

                let initial_camera = PlanetariumCamera::default();
                let initial_topo_to_cam = initial_camera.topocentric_to_camera_matrix();
                let initial_eq_to_cam = initial_camera.icrs_to_camera_matrix();
                let initial_aspect = width as f32 / height as f32;
                let initial_proj = initial_camera.projection_params(initial_aspect);

                let horizon_material = CelestialLineMaterial {
                    uniforms: CelestialLineUniforms {
                        transform: Mat4::IDENTITY,
                        projection: initial_proj.into(),
                        color: Vec4::new(0.0, 0.85, 1.0, 0.85),
                        minor_alpha: 1.0,
                    },
                };

                let altaz_material = CelestialLineMaterial {
                    uniforms: CelestialLineUniforms {
                        transform: Mat4::IDENTITY,
                        projection: initial_proj.into(),
                        color: Vec4::new(0.2, 0.55, 0.85, 0.40),
                        minor_alpha: 1.0,
                    },
                };

                let eq_material = CelestialLineMaterial {
                    uniforms: CelestialLineUniforms {
                        transform: Mat4::IDENTITY,
                        projection: initial_proj.into(),
                        color: Vec4::new(0.85, 0.50, 0.20, 0.30),
                        minor_alpha: 1.0,
                    },
                };

                let horizon_mat_handle = app
                    .world_mut()
                    .resource_mut::<Assets<CelestialLineMaterial>>()
                    .add(horizon_material);

                let altaz_mat_handle = app
                    .world_mut()
                    .resource_mut::<Assets<CelestialLineMaterial>>()
                    .add(altaz_material);

                let eq_mat_handle = app
                    .world_mut()
                    .resource_mut::<Assets<CelestialLineMaterial>>()
                    .add(eq_material);

                let horizon_mesh_handle = app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(create_horizon_mesh(initial_topo_to_cam));

                let altaz_mesh_handle = app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(create_altaz_grid_mesh(initial_topo_to_cam));

                let eq_mesh_handle = app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(create_equatorial_grid_mesh(initial_eq_to_cam));

                app.init_resource::<PendingCommands>();
                app.init_resource::<CameraSyncState>();
                app.insert_resource(bridge.clone());
                app.insert_resource(ViewportDimensions { width, height });
                app.insert_resource(CelestialHandles {
                    horizon_material: horizon_mat_handle.clone(),
                    altaz_material: altaz_mat_handle.clone(),
                    eq_material: eq_mat_handle.clone(),
                    horizon_mesh: horizon_mesh_handle.clone(),
                    altaz_mesh: altaz_mesh_handle.clone(),
                    eq_mesh: eq_mesh_handle.clone(),
                });

                let h_mat = horizon_mat_handle.clone();
                let a_mat = altaz_mat_handle.clone();
                let e_mat = eq_mat_handle.clone();

                let h_mesh = horizon_mesh_handle.clone();
                let a_mesh = altaz_mesh_handle.clone();
                let e_mesh = eq_mesh_handle.clone();

                app.add_systems(Startup, move |mut commands: Commands| {
                    commands.spawn((
                        Camera3d::default(),
                        Camera::default(),
                        RenderTarget::Image(target_clone.clone().into()),
                        PlanetariumCameraMarker,
                        PlanetariumCamera::default(),
                    ));

                    commands.spawn((
                        Mesh3d(h_mesh.clone()),
                        MeshMaterial3d(h_mat.clone()),
                        Transform::default(),
                        NoFrustumCulling,
                    ));

                    commands.spawn((
                        Mesh3d(a_mesh.clone()),
                        MeshMaterial3d(a_mat.clone()),
                        Transform::default(),
                        NoFrustumCulling,
                    ));

                    commands.spawn((
                        Mesh3d(e_mesh.clone()),
                        MeshMaterial3d(e_mat.clone()),
                        Transform::default(),
                        NoFrustumCulling,
                    ));
                });

                app.add_systems(Update, camera_movement_and_interpolation_system);

                app.finish();
                app.cleanup();

                let mut last_view_size = None;
                let mut frames_remaining = 0;
                let mut current_width = width;
                let mut current_height = height;

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
                    } else if guard.is_playing || guard.is_dirty {
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

                    let resized_size = process_viewport_commands(
                        commands,
                        &mut app,
                        &mut current_width,
                        &mut current_height,
                        &render_target_handle,
                        &mut frames_remaining,
                    );

                    app.update();

                    let mut new_texture_view = None;
                    if let Some(render_sub_app) = app.get_sub_app(bevy::render::RenderApp) {
                        let render_world = render_sub_app.world();
                        if let Some(render_assets) = render_world
                            .get_resource::<bevy::render::render_asset::RenderAssets<
                            bevy::render::texture::GpuImage,
                        >>() {
                            if let Some(gpu_image) = render_assets.get(&render_target_handle) {
                                let wgpu_texture: &wgpu::Texture = &gpu_image.texture;
                                let current_size = (wgpu_texture.width(), wgpu_texture.height());

                                if current_size == (current_width, current_height)
                                    && last_view_size != Some(current_size)
                                {
                                    let wgpu_view = wgpu_texture
                                        .create_view(&wgpu::TextureViewDescriptor::default());
                                    new_texture_view = Some(Arc::new(wgpu_view));
                                    last_view_size = Some(current_size);
                                }
                            }
                        }
                    }

                    let mut needs_frame_dirty = false;
                    if frames_remaining > 0 {
                        frames_remaining -= 1;
                        needs_frame_dirty = true;
                    }

                    let (view_info, is_interpolating) =
                        if let Some(sync) = app.world().get_resource::<CameraSyncState>() {
                            (sync.view_info, sync.is_interpolating)
                        } else {
                            (PlanetariumViewInfo::default(), false)
                        };

                    {
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        if let Some(size) = resized_size {
                            guard.last_size = Some(size);
                        }
                        if let Some(texture) = new_texture_view {
                            guard.latest_texture = Some(texture);
                        }
                        guard.view_info = view_info;
                        if is_interpolating || needs_frame_dirty {
                            guard.is_dirty = true;
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
