#![recursion_limit = "256"]

pub mod atmosphere;
pub mod camera;
pub mod grids;
pub mod ground;
pub mod hips;
pub mod landscape;
pub mod material;
pub mod sky;
pub mod stars;
pub mod viewport;

pub use atmosphere::AtmosphereSettings;
pub use grids::CoordinateGridSettings;
pub use ground::GroundSettings;
pub use hips::{HipsActiveTiles, HipsGridSettings, HipsMeshCache, HipsTile};
pub use landscape::{HorizonImageData, HorizonLayerData, HorizonRenderSettings};
pub use stars::{PickedStar, StarRenderParams, StarSettings, StarSpatialIndex};

use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    thread::JoinHandle,
    time::Duration,
};

use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    core_pipeline::tonemapping::Tonemapping,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        RenderPlugin,
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
            WgpuWrapper,
        },
        settings::RenderCreation,
        storage::ShaderBuffer,
    },
};

pub use camera::{PlanetariumCamera, PlanetariumViewInfo, SkyCoordinates, ViewMode};
use celestial::HorizonProfile;
use grids::{create_altaz_grid_mesh, create_equatorial_grid_mesh, create_horizon_mesh};
pub use material::{CelestialLineMaterial, CelestialLineUniforms, init_celestial_materials};

pub use crate::viewport::{ObserverClockSpec, ObserverUpdate, ViewportCommand};

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
    pub shutdown: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub view_info: PlanetariumViewInfo,
    pub star_index: Option<Arc<StarSpatialIndex>>,
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
                    shutdown: false,
                    latest_texture: None,
                    last_size: None,
                    view_info: PlanetariumViewInfo::default(),
                    star_index: None,
                    pending_commands: Vec::new(),
                }),
                cvar: Condvar::new(),
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

    pub fn star_index(&self) -> Option<Arc<StarSpatialIndex>> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.star_index.clone()
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
        self.send_command(ViewportCommand::Zoom {
            delta,
            cursor_ndc: None,
        });
    }

    pub fn zoom_at(&self, delta: f64, cursor_ndc: (f64, f64)) {
        self.send_command(ViewportCommand::Zoom {
            delta,
            cursor_ndc: Some(cursor_ndc),
        });
    }

    pub fn set_view_mode(&self, mode: ViewMode) {
        self.send_command(ViewportCommand::SetViewMode(mode));
    }

    pub fn set_hips_grid_visible(&self, visible: bool) {
        self.send_command(ViewportCommand::ToggleHipsGrid(visible));
    }

    pub fn set_hips_survey_visible(&self, visible: bool) {
        self.send_command(ViewportCommand::ToggleHipsSurvey(visible));
    }

    pub fn set_hips_survey(&self, survey: hips::HipsSurvey) {
        self.send_command(ViewportCommand::SetHipsSurvey(survey));
    }

    pub fn set_hips_survey_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetHipsSurveyOpacity(clamped));
    }

    pub fn set_star_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetStarOpacity(clamped));
    }

    pub fn set_altaz_grid_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetAltAzGridOpacity(clamped));
    }

    pub fn set_equatorial_grid_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetEquatorialGridOpacity(clamped));
    }

    pub fn set_observer(
        &self,
        location: celestial::coords::geodetic::GeodeticPosition,
        clock: ObserverClockSpec,
    ) {
        self.send_command(ViewportCommand::SetObserver(ObserverUpdate {
            location,
            clock,
        }));
    }

    pub fn set_atmosphere_strength(&self, strength: f32) {
        let clamped = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetAtmosphereStrength(clamped));
    }

    pub fn set_ground_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetGroundOpacity(clamped));
    }

    pub fn set_horizon(&self, data: Option<HorizonLayerData>) {
        self.send_command(ViewportCommand::SetHorizon(data));
    }

    pub fn set_horizon_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetHorizonOpacity(clamped));
    }

    pub fn set_horizon_line_visible(&self, visible: bool) {
        self.send_command(ViewportCommand::SetHorizonLineVisible(visible));
    }

    pub fn resize(&self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        let changed = {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if guard.last_size == Some((width, height)) {
                false
            } else {
                guard.last_size = Some((width, height));
                guard
                    .pending_commands
                    .push(ViewportCommand::Resize(width, height));
                guard.is_dirty = true;
                true
            }
        };
        if changed {
            self.inner.cvar.notify_one();
        }
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

    pub fn request_frame(&self) {
        self.request_redraw();
    }

    pub fn notify_frame_ready(&self) {
        (self.inner.notify_ui_fn)();
    }

    pub fn shutdown(&self) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.shutdown = true;
        }
        self.inner.cvar.notify_all();
    }
}

#[derive(Resource, Clone)]
struct CelestialHandles {
    horizon_entity: Entity,
    altaz_entity: Entity,
    eq_entity: Entity,
    horizon_mesh: Handle<Mesh>,
    altaz_mesh: Handle<Mesh>,
    eq_mesh: Handle<Mesh>,
    star_entity: Option<Entity>,
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
    location: celestial::coords::geodetic::GeodeticPosition,
    topo_to_cam: glam::DMat3,
    eq_to_cam: glam::DMat3,
    epoch_secs: f64,
    last_uniform_epoch_secs: f64,
}

impl Default for GridMeshState {
    fn default() -> Self {
        let cam = PlanetariumCamera::default();
        let epoch_secs = cam.observer.epoch_unix_seconds();
        Self {
            level: None,
            fov_y: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            is_zoomed_in: false,
            view_mode: ViewMode::Horizontal,
            location: cam.observer.location,
            topo_to_cam: cam.topocentric_to_camera_matrix(),
            eq_to_cam: cam.icrs_to_camera_matrix(),
            epoch_secs,
            last_uniform_epoch_secs: epoch_secs,
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
struct ObserverClockResource(ObserverClockSpec);

impl Default for ObserverClockResource {
    fn default() -> Self {
        Self(ObserverClockSpec::Live)
    }
}

#[derive(Resource, Default)]
struct PendingCommands(Vec<ViewportCommand>);

#[derive(Resource)]
struct CameraSyncState {
    view_info: PlanetariumViewInfo,
    is_interpolating: bool,
    idle_update_interval: Duration,
}

impl Default for CameraSyncState {
    fn default() -> Self {
        Self {
            view_info: PlanetariumViewInfo::default(),
            is_interpolating: false,
            idle_update_interval: Duration::from_secs(5),
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
/// Alpha of the flat 0° horizon circle; hidden unless the horizon line setting is enabled.
fn horizon_line_alpha(settings: &landscape::HorizonRenderSettings) -> f32 {
    if settings.show_line { 0.85 } else { 0.0 }
}

fn camera_movement_and_interpolation_system(
    time: Res<Time>,
    mut pending_commands: ResMut<PendingCommands>,
    mut observer_clock: ResMut<ObserverClockResource>,
    mut camera_query: Query<&mut PlanetariumCamera>,
    mut camera_sync: ResMut<CameraSyncState>,
    dims: Res<ViewportDimensions>,
    handles: Res<CelestialHandles>,
    star_settings: Res<stars::StarSettings>,
    coord_grid_settings: Res<grids::CoordinateGridSettings>,
    horizon_settings: Res<landscape::HorizonRenderSettings>,
    mut line_materials: Query<&mut CelestialLineMaterial>,
    mut star_materials: Query<&mut stars::CelestialStarMaterial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut grid_state: Local<GridMeshState>,
    mut is_initialized: Local<bool>,
) {
    let Ok(mut camera) = camera_query.single_mut() else {
        return;
    };

    let aspect = dims.width.max(1) as f32 / dims.height.max(1) as f32;
    let mut moved = false;
    for cmd in pending_commands.0.drain(..) {
        match cmd {
            ViewportCommand::Pan { delta_x, delta_y } => {
                camera.pan(delta_x, delta_y, dims.height as f64);
                moved = true;
            }
            ViewportCommand::Zoom { delta, cursor_ndc } => {
                if let Some(ndc) = cursor_ndc {
                    camera.zoom_at(delta, ndc, aspect);
                } else {
                    camera.zoom(delta);
                }
                moved = true;
            }
            ViewportCommand::SetViewMode(mode) => {
                camera.set_view_mode(mode);
                moved = true;
            }
            ViewportCommand::SetObserver(update) => {
                camera.observer.location = update.location;
                observer_clock.0 = update.clock;
                moved = true;
            }
            ViewportCommand::Resize(_, _) => {
                moved = true;
            }
            _ => {}
        }
    }

    match observer_clock.0 {
        ObserverClockSpec::Live => camera.observer.update_epoch_now(),
        ObserverClockSpec::FixedUnixSeconds(secs) => {
            camera.observer.set_epoch_unix_seconds(secs);
        }
    }
    let epoch_secs = camera.observer.epoch_unix_seconds();
    let epoch_changed = (epoch_secs - grid_state.last_uniform_epoch_secs).abs() > 1e-6;

    let prev_yaw = camera.yaw;
    let prev_pitch = camera.pitch;
    let prev_fov = camera.fov_y;

    camera.update(time.delta().as_secs_f64());

    let yaw_diff = (camera.yaw - prev_yaw).abs();
    let pitch_diff = (camera.pitch - prev_pitch).abs();
    let fov_diff = (camera.fov_y - prev_fov).abs();
    let mut target_yaw_diff = (camera.target_yaw - camera.yaw)
        .abs()
        .rem_euclid(std::f64::consts::TAU);
    if target_yaw_diff > std::f64::consts::PI {
        target_yaw_diff = std::f64::consts::TAU - target_yaw_diff;
    }
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

    if moved || is_interpolating || is_first_run || epoch_changed {
        let proj_params = camera.projection_params(aspect);

        let subdiv_count = 6;
        let (grid_level, minor_alpha) = camera.grid_zoom_factor(subdiv_count);

        let is_zoomed_in = camera.fov_y < std::f64::consts::FRAC_PI_3;
        let zoom_ratio_diff = (camera.fov_y - grid_state.fov_y).abs() / grid_state.fov_y.max(1e-8);
        let mut diff_yaw = (camera.yaw - grid_state.yaw)
            .abs()
            .rem_euclid(std::f64::consts::TAU);
        if diff_yaw > std::f64::consts::PI {
            diff_yaw = std::f64::consts::TAU - diff_yaw;
        }
        let effective_yaw_diff = diff_yaw * camera.pitch.cos().abs();
        let sidereal_drift = (epoch_secs - grid_state.epoch_secs).abs() * 7.292_115e-5;
        let moved_significantly = is_zoomed_in
            && (zoom_ratio_diff > 0.15
                || effective_yaw_diff > camera.fov_y * 0.1
                || (camera.pitch - grid_state.pitch).abs() > camera.fov_y * 0.1
                || sidereal_drift > camera.fov_y * 0.05);

        let needs_rebuild = grid_state.level != Some(grid_level)
            || grid_state.is_zoomed_in != is_zoomed_in
            || grid_state.view_mode != camera.view_mode
            || grid_state.location != camera.observer.location
            || moved_significantly;

        if needs_rebuild {
            let view_info = camera.view_info(aspect);
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
            grid_state.location = camera.observer.location;
            grid_state.topo_to_cam = topo_to_cam;
            grid_state.eq_to_cam = eq_to_cam;
            grid_state.epoch_secs = epoch_secs;
        }

        let delta_topo = glam::DMat4::from_mat3(
            camera.topocentric_to_camera_matrix() * grid_state.topo_to_cam.transpose(),
        )
        .as_mat4();
        let delta_eq = glam::DMat4::from_mat3(
            camera.icrs_to_camera_matrix() * grid_state.eq_to_cam.transpose(),
        )
        .as_mat4();

        if let Ok(mut mat) = line_materials.get_mut(handles.horizon_entity) {
            mat.uniforms.transform = delta_topo;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = 1.0;
            mat.uniforms.color.w = horizon_line_alpha(&horizon_settings);
        }
        if let Ok(mut mat) = line_materials.get_mut(handles.altaz_entity) {
            mat.uniforms.transform = delta_topo;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = minor_alpha;
            mat.uniforms.color.w = grids::ALTAZ_GRID_BASE_ALPHA * coord_grid_settings.altaz_opacity;
        }
        if let Ok(mut mat) = line_materials.get_mut(handles.eq_entity) {
            mat.uniforms.transform = delta_eq;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.minor_alpha = minor_alpha;
            mat.uniforms.color.w =
                grids::EQUATORIAL_GRID_BASE_ALPHA * coord_grid_settings.equatorial_opacity;
        }

        if let Some(star_entity) = handles.star_entity {
            if let Ok(mut mat) = star_materials.get_mut(star_entity) {
                let render_params = stars::StarRenderParams::default();
                mat.uniforms.transform =
                    glam::DMat4::from_mat3(camera.icrs_to_camera_matrix()).as_mat4();
                mat.uniforms.projection = proj_params.into();
                mat.uniforms.viewport_size = Vec2::new(dims.width as f32, dims.height as f32);
                mat.uniforms.limiting_mag = render_params.limiting_mag;
                mat.uniforms.fade_range = render_params.fade_range;
                mat.uniforms.base_radius = render_params.base_radius;
                mat.uniforms.min_radius = render_params.min_radius;
                mat.uniforms.max_radius = render_params.max_radius;
                mat.uniforms.opacity = star_settings.opacity;
            }
        }

        grid_state.last_uniform_epoch_secs = epoch_secs;
        camera_sync.view_info = camera.view_info(aspect);
        camera_sync.is_interpolating = is_interpolating;
        camera_sync.idle_update_interval = match observer_clock.0 {
            ObserverClockSpec::Live => camera.sky_rotation_update_interval(dims.height),
            ObserverClockSpec::FixedUnixSeconds(_) => Duration::from_secs(3600),
        };
    }

    if let Ok(mut mat) = line_materials.get_mut(handles.horizon_entity) {
        mat.uniforms.color.w = horizon_line_alpha(&horizon_settings);
    }
    if let Ok(mut mat) = line_materials.get_mut(handles.altaz_entity) {
        mat.uniforms.color.w = grids::ALTAZ_GRID_BASE_ALPHA * coord_grid_settings.altaz_opacity;
    }
    if let Ok(mut mat) = line_materials.get_mut(handles.eq_entity) {
        mat.uniforms.color.w =
            grids::EQUATORIAL_GRID_BASE_ALPHA * coord_grid_settings.equatorial_opacity;
    }
    if let Some(star_entity) = handles.star_entity {
        if let Ok(mut mat) = star_materials.get_mut(star_entity) {
            mat.uniforms.opacity = star_settings.opacity;
        }
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
            ViewportCommand::ToggleHipsGrid(visible) => {
                if let Some(mut settings) =
                    app.world_mut().get_resource_mut::<hips::HipsGridSettings>()
                {
                    settings.visible = visible;
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::ToggleHipsSurvey(visible) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<hips::HipsSurveySettings>()
                {
                    settings.enabled = visible;
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetHipsSurvey(survey) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<hips::HipsSurveySettings>()
                {
                    settings.survey = survey;
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetHipsSurveyOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<hips::HipsSurveySettings>()
                {
                    settings.alpha = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetStarOpacity(opacity) => {
                if let Some(mut settings) = app.world_mut().get_resource_mut::<stars::StarSettings>()
                {
                    settings.opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetAltAzGridOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<grids::CoordinateGridSettings>()
                {
                    settings.altaz_opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetEquatorialGridOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<grids::CoordinateGridSettings>()
                {
                    settings.equatorial_opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetAtmosphereStrength(strength) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<atmosphere::AtmosphereSettings>()
                {
                    settings.strength = if strength.is_finite() {
                        strength.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetGroundOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<ground::GroundSettings>()
                {
                    settings.opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetHorizonOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<landscape::HorizonRenderSettings>()
                {
                    settings.opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetHorizonLineVisible(visible) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<landscape::HorizonRenderSettings>()
                {
                    settings.show_line = visible;
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetHorizon(maybe_data) => {
                let Some((horizon_buffer_handle, fallback_image_handle, prev_tag)) = app
                    .world()
                    .get_resource::<landscape::HorizonRenderSettings>()
                    .map(|s| {
                        (
                            s.horizon_buffer.clone(),
                            s.fallback_image.clone(),
                            s.current_image_tag.clone(),
                        )
                    })
                else {
                    continue;
                };

                if let Some(data) = maybe_data {
                    let profile =
                        HorizonProfile::from_samples(data.samples_deg, data.azimuth_offset_deg);
                    let mut samples_rad = profile.to_radians_vec();
                    if samples_rad.is_empty() {
                        samples_rad.push(0.0);
                    }
                    let sample_count = samples_rad.len() as u32;
                    let azimuth_offset_rad = profile.azimuth_offset_deg.to_radians() as f32;

                    if let Some(mut buf) = app
                        .world_mut()
                        .resource_mut::<Assets<ShaderBuffer>>()
                        .get_mut(&horizon_buffer_handle)
                    {
                        buf.data = Some(bytemuck::cast_slice(&samples_rad).to_vec());
                    }

                    let new_image_handle = match (&data.image_tag, data.image) {
                        (Some(_tag), Some(img_data)) => {
                            let mut image = Image::new(
                                Extent3d {
                                    width: img_data.width.max(1),
                                    height: img_data.height.max(1),
                                    depth_or_array_layers: 1,
                                },
                                TextureDimension::D2,
                                img_data.rgba.clone(),
                                TextureFormat::Rgba8UnormSrgb,
                                RenderAssetUsages::RENDER_WORLD,
                            );
                            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                                address_mode_u: ImageAddressMode::Repeat,
                                address_mode_v: ImageAddressMode::ClampToEdge,
                                address_mode_w: ImageAddressMode::ClampToEdge,
                                ..ImageSamplerDescriptor::linear()
                            });
                            Some(app.world_mut().resource_mut::<Assets<Image>>().add(image))
                        }
                        _ => None,
                    };

                    if let Some(mut settings) = app
                        .world_mut()
                        .get_resource_mut::<landscape::HorizonRenderSettings>()
                    {
                        settings.active = true;
                        settings.azimuth_offset_rad = azimuth_offset_rad;
                        settings.sample_count = sample_count;
                        match (data.image_tag, new_image_handle) {
                            (None, _) => {
                                settings.panorama_image = fallback_image_handle;
                                settings.has_texture = false;
                                settings.current_image_tag = None;
                            }
                            (Some(tag), Some(handle)) => {
                                settings.panorama_image = handle;
                                settings.has_texture = true;
                                settings.current_image_tag = Some(tag);
                            }
                            (Some(tag), None) => {
                                if prev_tag.as_ref() != Some(&tag) {
                                    settings.panorama_image = fallback_image_handle;
                                    settings.has_texture = false;
                                    settings.current_image_tag = None;
                                }
                            }
                        }
                    }
                } else if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<landscape::HorizonRenderSettings>()
                {
                    settings.active = false;
                    settings.has_texture = false;
                    settings.sample_count = 0;
                    settings.azimuth_offset_rad = 0.0;
                    settings.panorama_image = fallback_image_handle;
                    settings.current_image_tag = None;
                }
                *frames_remaining = 3;
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
        let spawn_instant = std::time::Instant::now();
        std::thread::Builder::new()
            .name("Bevy renderer".to_string())
            .spawn(move || {
                let thread_start = std::time::Instant::now();
                #[cfg(all(debug_assertions, feature = "tracy"))]
                if let Some(client) = tracy_client::Client::running() {
                    client.set_thread_name("Bevy renderer");
                }

                let init_span = tracing::info_span!("planetarium_init").entered();
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
                    if guard.last_size.is_none() {
                        guard.last_size = Some((width, height));
                    }
                }

                let sync_device = device.clone();
                let sync_queue = queue.clone();

                {
                    let _span = tracing::info_span!("bevy_add_plugins").entered();
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
                            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(
                            ), // TODO: try to make pipelined rendering work
                    );
                }

                {
                    let _span = tracing::info_span!("init_celestial_materials").entered();
                    init_celestial_materials(&mut app);
                }

                let render_target_handle = {
                    let _span = tracing::info_span!("create_render_target_image").entered();
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
                    app.world_mut()
                        .resource_mut::<Assets<Image>>()
                        .add(render_image)
                };

                let target_clone = render_target_handle.clone();

                let initial_camera = PlanetariumCamera::default();
                let initial_topo_to_cam = initial_camera.topocentric_to_camera_matrix();
                let initial_eq_to_cam = initial_camera.icrs_to_camera_matrix();
                let initial_aspect = width as f32 / height as f32;
                let initial_proj = initial_camera.projection_params(initial_aspect);

                let (horizon_material, altaz_material, eq_material) = {
                    let _span = tracing::info_span!("init_line_materials").entered();
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
                            color: Vec4::new(0.2, 0.55, 0.85, grids::ALTAZ_GRID_BASE_ALPHA),
                            minor_alpha: 1.0,
                        },
                    };

                    let eq_material = CelestialLineMaterial {
                        uniforms: CelestialLineUniforms {
                            transform: Mat4::IDENTITY,
                            projection: initial_proj.into(),
                            color: Vec4::new(0.85, 0.50, 0.20, grids::EQUATORIAL_GRID_BASE_ALPHA),
                            minor_alpha: 1.0,
                        },
                    };

                    (horizon_material, altaz_material, eq_material)
                };

                let horizon_mesh_handle = {
                    let _span = tracing::info_span!("create_horizon_mesh").entered();
                    app.world_mut()
                        .resource_mut::<Assets<Mesh>>()
                        .add(create_horizon_mesh(initial_topo_to_cam))
                };

                let altaz_mesh_handle = {
                    let _span = tracing::info_span!("create_altaz_grid_mesh").entered();
                    app.world_mut()
                        .resource_mut::<Assets<Mesh>>()
                        .add(create_altaz_grid_mesh(initial_topo_to_cam))
                };

                let eq_mesh_handle = {
                    let _span = tracing::info_span!("create_equatorial_grid_mesh").entered();
                    app.world_mut()
                        .resource_mut::<Assets<Mesh>>()
                        .add(create_equatorial_grid_mesh(initial_eq_to_cam))
                };

                let star_catalog = {
                    let _span = tracing::info_span!("load_star_catalog_step").entered();
                    stars::load_star_catalog()
                };
                let star_setup = if let Some(catalog) = star_catalog {
                    let star_count = catalog.records.len();
                    {
                        let _span =
                            tracing::info_span!("build_star_spatial_index", star_count).entered();
                        let index = Arc::new(stars::StarSpatialIndex::build_with_metadata(
                            &catalog.records,
                            catalog.metadata,
                        ));
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        guard.star_index = Some(index);
                    }
                    let buffer_handle = {
                        let _span =
                            tracing::info_span!("create_star_shader_buffer", star_count).entered();
                        let raw_bytes: &[u8] = bytemuck::cast_slice(&catalog.records);
                        let storage_buffer =
                            ShaderBuffer::new(raw_bytes, RenderAssetUsages::RENDER_WORLD);
                        app.world_mut()
                            .resource_mut::<Assets<ShaderBuffer>>()
                            .add(storage_buffer)
                    };

                    let mesh_handle = {
                        let _span =
                            tracing::info_span!("create_star_mesh_step", star_count).entered();
                        let star_mesh = stars::create_star_quad_mesh();
                        app.world_mut()
                            .resource_mut::<Assets<Mesh>>()
                            .add(star_mesh)
                    };

                    let star_material = {
                        let _span = tracing::info_span!("create_star_material_step").entered();
                        let render_params = stars::StarRenderParams::default();
                        stars::CelestialStarMaterial {
                            uniforms: stars::CelestialStarUniforms {
                                transform: glam::DMat4::from_mat3(initial_eq_to_cam).as_mat4(),
                                projection: initial_proj.into(),
                                viewport_size: Vec2::new(width as f32, height as f32),
                                limiting_mag: render_params.limiting_mag,
                                fade_range: render_params.fade_range,
                                base_radius: render_params.base_radius,
                                min_radius: render_params.min_radius,
                                max_radius: render_params.max_radius,
                                opacity: 1.0,
                            },
                            stars: buffer_handle,
                            star_count: star_count as u32,
                        }
                    };

                    Some((star_material, mesh_handle))
                } else {
                    None
                };

                app.init_resource::<PendingCommands>();
                app.init_resource::<ObserverClockResource>();
                app.init_resource::<CameraSyncState>();
                app.init_resource::<atmosphere::AtmosphereSettings>();
                app.init_resource::<ground::GroundSettings>();
                app.init_resource::<stars::StarSettings>();
                app.init_resource::<grids::CoordinateGridSettings>();
                let tile_loader = hips::HipsTileLoader::new(Some(bridge.clone()));
                app.insert_resource(tile_loader);
                app.init_resource::<hips::HipsMeshCache>();
                app.init_resource::<hips::HipsActiveTiles>();
                app.init_resource::<hips::HipsGridSettings>();
                app.init_resource::<hips::HipsSurfaceMeshCache>();
                app.init_resource::<hips::HipsImageCache>();
                app.init_resource::<hips::HipsActiveSurveyTiles>();
                app.init_resource::<hips::HipsSurveySettings>();
                app.insert_resource(bridge.clone());
                app.insert_resource(ViewportDimensions { width, height });

                let initial_horizon_settings = {
                    let initial_samples = vec![0.0_f32; celestial::DEFAULT_HORIZON_SAMPLES];
                    let horizon_buffer = app
                        .world_mut()
                        .resource_mut::<Assets<ShaderBuffer>>()
                        .add(ShaderBuffer::new(
                            bytemuck::cast_slice(&initial_samples),
                            RenderAssetUsages::default(),
                        ));
                    let mut fallback = Image::new(
                        Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        vec![0, 0, 0, 0],
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::RENDER_WORLD,
                    );
                    fallback.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                        address_mode_u: ImageAddressMode::Repeat,
                        address_mode_v: ImageAddressMode::ClampToEdge,
                        address_mode_w: ImageAddressMode::ClampToEdge,
                        ..ImageSamplerDescriptor::linear()
                    });
                    let fallback_image =
                        app.world_mut().resource_mut::<Assets<Image>>().add(fallback);
                    landscape::HorizonRenderSettings {
                        opacity: 1.0,
                        active: false,
                        has_texture: false,
                        show_line: false,
                        azimuth_offset_rad: 0.0,
                        sample_count: 0,
                        horizon_buffer,
                        panorama_image: fallback_image.clone(),
                        fallback_image,
                        current_image_tag: None,
                    }
                };
                app.insert_resource(initial_horizon_settings.clone());

                let (horizon_entity, altaz_entity, eq_entity, star_entity) = {
                    let _span = tracing::info_span!("spawn_celestial_entities").entered();
                    let world = app.world_mut();
                    let horizon = world
                        .spawn((Mesh3d(horizon_mesh_handle.clone()), horizon_material))
                        .id();
                    let altaz = world
                        .spawn((Mesh3d(altaz_mesh_handle.clone()), altaz_material))
                        .id();
                    let eq = world
                        .spawn((Mesh3d(eq_mesh_handle.clone()), eq_material))
                        .id();
                    let star = star_setup
                        .map(|(material, mesh)| world.spawn((Mesh3d(mesh), material)).id());
                    world.spawn(sky::CelestialSkyMaterial::from_camera(
                        &initial_camera,
                        initial_proj,
                        1.0,
                        1.0,
                    ));
                    world.spawn(landscape::CelestialLandscapeMaterial::from_camera_and_settings(
                        &initial_camera,
                        initial_proj,
                        &initial_horizon_settings,
                    ));
                    (horizon, altaz, eq, star)
                };

                app.insert_resource(CelestialHandles {
                    horizon_entity,
                    altaz_entity,
                    eq_entity,
                    horizon_mesh: horizon_mesh_handle,
                    altaz_mesh: altaz_mesh_handle,
                    eq_mesh: eq_mesh_handle,
                    star_entity,
                });

                {
                    let _span = tracing::info_span!("add_startup_systems").entered();
                    app.add_systems(Startup, move |mut commands: Commands| {
                        commands.spawn((
                            Camera3d::default(),
                            Camera::default(),
                            RenderTarget::Image(target_clone.clone().into()),
                            Tonemapping::None,
                            bevy::render::view::Msaa::Off,
                            PlanetariumCameraMarker,
                            PlanetariumCamera::default(),
                        ));
                    });
                }

                app.add_systems(
                    Update,
                    (
                        camera_movement_and_interpolation_system,
                        sky::sky_system,
                        landscape::landscape_system,
                        hips::hips_survey_system,
                        hips::hips_grid_system,
                    )
                        .chain(),
                );

                {
                    let _span = tracing::info_span!("bevy_app_finish").entered();
                    app.finish();
                }
                {
                    let _span = tracing::info_span!("bevy_app_cleanup").entered();
                    app.cleanup();
                }
                drop(init_span);

                tracing::info!(
                    "Bevy renderer initialized in {:.2}ms (thread spawn delay: {:.2}ms)",
                    thread_start.elapsed().as_secs_f64() * 1000.0,
                    (thread_start - spawn_instant).as_secs_f64() * 1000.0,
                );

                let mut last_view_size = None;
                let mut frames_remaining = 0;
                let mut current_width = width;
                let mut current_height = height;
                let mut is_first_frame = true;
                let mut last_frame_start: Option<std::time::Instant> = None;
                let mut last_submission: Option<wgpu::SubmissionIndex> = None;
                let mut idle_update_interval = Duration::from_secs(5);
                const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(15_500);

                loop {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if guard.shutdown {
                        break;
                    }
                    if !guard.is_dirty && !guard.is_playing {
                        let wait_for = last_frame_start.map_or(idle_update_interval, |start| {
                            idle_update_interval.saturating_sub(start.elapsed())
                        });
                        if !wait_for.is_zero() {
                            guard = bridge
                                .inner
                                .cvar
                                .wait_timeout_while(guard, wait_for, |s| {
                                    !s.is_dirty && !s.is_playing && !s.shutdown
                                })
                                .unwrap_or_else(PoisonError::into_inner)
                                .0;
                        }
                    }
                    if guard.shutdown {
                        break;
                    }
                    if let Some(remaining) = last_frame_start
                        .and_then(|start| MIN_FRAME_INTERVAL.checked_sub(start.elapsed()))
                    {
                        guard = bridge
                            .inner
                            .cvar
                            .wait_timeout_while(guard, remaining, |s| !s.shutdown)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    if guard.shutdown {
                        break;
                    }
                    guard.is_dirty = false;
                    let commands = std::mem::take(&mut guard.pending_commands);
                    drop(guard);

                    if let Some(sub_idx) = last_submission.take() {
                        let _span = tracing::info_span!("gpu_frame_sync").entered();
                        let _ = sync_device.poll(wgpu::PollType::Wait {
                            submission_index: Some(sub_idx),
                            timeout: Some(Duration::from_millis(100)),
                        });
                    }

                    last_frame_start = Some(std::time::Instant::now());
                    let first_frame = is_first_frame;
                    #[cfg(all(debug_assertions, feature = "tracy"))]
                    let _frame_span = tracy_client::non_continuous_frame!("Frame");

                    let resized_size = {
                        let _span = tracing::info_span!("process_viewport_commands").entered();
                        process_viewport_commands(
                            commands,
                            &mut app,
                            &mut current_width,
                            &mut current_height,
                            &render_target_handle,
                            &mut frames_remaining,
                        )
                    };

                    {
                        let _span = tracing::info_span!("bevy_app_update").entered();
                        app.update();
                    }
                    last_submission = Some(sync_queue.submit(std::iter::empty()));

                    let mut new_texture_view = None;
                    {
                        let _span = tracing::info_span!("extract_texture_view").entered();
                        if let Some(render_sub_app) = app.get_sub_app(bevy::render::RenderApp) {
                            let render_world = render_sub_app.world();
                            if let Some(render_assets) = render_world
                                .get_resource::<bevy::render::render_asset::RenderAssets<
                                bevy::render::texture::GpuImage,
                            >>() {
                                if let Some(gpu_image) = render_assets.get(&render_target_handle) {
                                    let wgpu_texture: &wgpu::Texture = &gpu_image.texture;
                                    let current_size =
                                        (wgpu_texture.width(), wgpu_texture.height());

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
                    }

                    let mut needs_frame_dirty = false;
                    if frames_remaining > 0 {
                        frames_remaining -= 1;
                        needs_frame_dirty = true;
                    }

                    let (view_info, is_interpolating, next_idle_interval) =
                        if let Some(sync) = app.world().get_resource::<CameraSyncState>() {
                            (
                                sync.view_info,
                                sync.is_interpolating,
                                sync.idle_update_interval,
                            )
                        } else {
                            (
                                PlanetariumViewInfo::default(),
                                false,
                                Duration::from_secs(5),
                            )
                        };
                    idle_update_interval = next_idle_interval;

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

                    if first_frame {
                        is_first_frame = false;
                        tracing::info!(
                            "Bevy first frame rendered in {:.2}ms from thread start",
                            thread_start.elapsed().as_secs_f64() * 1000.0
                        );
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
