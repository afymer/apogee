#![recursion_limit = "256"]

pub mod atmosphere;
pub mod camera;
pub mod dso;
pub mod grids;
pub mod ground;
pub mod hips;
pub mod landscape;
pub mod material;
pub mod metrics;
pub mod sky;
pub mod solar_system;
pub mod stars;
pub mod viewport;

pub use atmosphere::AtmosphereSettings;
pub use dso::{
    DsoGpuRecord, DsoLabel, DsoRenderParams, DsoSettings, DsoSpatialIndex, PickedDso,
};
pub use grids::CoordinateGridSettings;
pub use ground::GroundSettings;
pub use hips::{HipsActiveTiles, HipsGridSettings, HipsMeshCache, HipsTile};
pub use landscape::{HorizonImageData, HorizonLayerData, HorizonRenderSettings};
pub use metrics::BevyFrameSample;
pub use solar_system::{
    PickedSolarSystemBody, SolarSystemRenderSettings, pick_solar_system_body,
    rendered_solar_system_disc_radius_px,
};
pub use stars::{
    PickedStar, StarRenderParams, StarSettings, StarSpatialIndex, bp_rp_to_srgb,
    estimate_teff_kelvin, spectral_class_from_bp_rp,
};

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

pub use camera::{
    PlanetariumCamera, PlanetariumViewInfo, SkyCoordinates, TrackingTarget, ViewMode,
};
use celestial::HorizonProfile;
use grids::{create_altaz_grid_mesh, create_equatorial_grid_mesh, create_horizon_mesh};
pub use material::{CelestialLineMaterial, CelestialLineUniforms, init_celestial_materials};

pub use crate::viewport::{
    MountCameraOverride, ObserverClockSpec, ObserverUpdate, TrajectoryRenderData, ViewportCommand,
};

/// Callback executed inside the same `wgpu::CommandEncoder` right after Bevy renders the sky.
/// Returns `true` if another animation frame is needed (e.g. smooth mount telemetry filtering).
pub type OverlayRenderCallback =
    Box<dyn FnMut(&mut wgpu::CommandEncoder, &wgpu::TextureView, u32, u32) -> bool + Send>;

#[derive(Default, Component, Clone, Copy)]
struct PlanetariumCameraMarker;

pub struct PlanetariumRenderer;

#[derive(Clone, Debug)]
pub struct BridgeSnapshot {
    pub texture_view: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub texture_size: Option<(u32, u32)>,
    pub view_info: PlanetariumViewInfo,
    pub is_mount_frame: bool,
}

#[derive(Default)]
pub struct BevyBridgeState {
    pub is_playing: bool,
    pub is_dirty: bool,
    pub shutdown: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub texture_size: Option<(u32, u32)>,
    pub view_info: PlanetariumViewInfo,
    pub star_index: Option<Arc<StarSpatialIndex>>,
    pub dso_index: Option<Arc<DsoSpatialIndex>>,
    pub pending_commands: Vec<ViewportCommand>,
    pub overlay_active: bool,
    pub texture_is_mount_mode: bool,
}

impl BevyBridgeState {
    pub fn texture(&self) -> Option<&Arc<wgpu::TextureView>> {
        self.latest_texture.as_ref()
    }
}

/// Handle returned by [`BevyBridge::add_frame_listener`] so callers can unsubscribe on drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameListenerId(u64);

struct BridgeInner {
    state: Mutex<BevyBridgeState>,
    cvar: Condvar,
    next_listener_id: std::sync::atomic::AtomicU64,
    notify_listeners: Mutex<Vec<(FrameListenerId, Box<dyn Fn() + Send + Sync>)>>,
    overlay_pass: Mutex<Option<OverlayRenderCallback>>,
    latest_metrics_seq: std::sync::atomic::AtomicU64,
    metrics: Mutex<metrics::BevyMetricsRing>,
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
                    texture_size: None,
                    view_info: PlanetariumViewInfo::default(),
                    star_index: None,
                    dso_index: None,
                    pending_commands: Vec::new(),
                    overlay_active: false,
                    texture_is_mount_mode: false,
                }),
                cvar: Condvar::new(),
                next_listener_id: std::sync::atomic::AtomicU64::new(1),
                notify_listeners: Mutex::new(vec![(FrameListenerId(0), Box::new(notify_ui))]),
                overlay_pass: Mutex::new(None),
                latest_metrics_seq: std::sync::atomic::AtomicU64::new(0),
                metrics: Mutex::new(metrics::BevyMetricsRing::default()),
            }),
        }
    }

    pub fn latest_frame_seq(&self) -> u64 {
        self.inner
            .latest_metrics_seq
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn collect_frame_samples(&self, cursor: &mut u64, out: &mut Vec<BevyFrameSample>) {
        if self.latest_frame_seq() <= *cursor {
            return;
        }
        let guard = self
            .inner
            .metrics
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.collect_since(cursor, out);
    }

    pub(crate) fn record_frame_sample(&self, sample: BevyFrameSample) {
        let seq = {
            let mut guard = self
                .inner
                .metrics
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.push(sample)
        };
        self.inner
            .latest_metrics_seq
            .store(seq, std::sync::atomic::Ordering::Release);
    }

    pub fn add_frame_listener(
        &self,
        listener: impl Fn() + Send + Sync + 'static,
    ) -> FrameListenerId {
        let id = FrameListenerId(
            self.inner
                .next_listener_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let mut guard = self
            .inner
            .notify_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.push((id, Box::new(listener)));
        id
    }

    pub fn remove_frame_listener(&self, id: FrameListenerId) {
        let mut guard = self
            .inner
            .notify_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.retain(|(entry_id, _)| *entry_id != id);
    }

    pub fn set_overlay_pass(&self, pass: Option<OverlayRenderCallback>) {
        let mut guard = self
            .inner
            .overlay_pass
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *guard = pass;
    }

    pub fn is_mount_camera_active(&self) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .overlay_active
    }

    pub fn set_mount_camera(&self, override_cam: Option<MountCameraOverride>) {
        self.set_mount_camera_with_size(override_cam, None);
    }

    /// Atomically updates mount camera mode and (optionally) resizes the render target in the
    /// same command batch so workspace switches never render an intermediate frame at the old size.
    pub fn set_mount_camera_with_size(
        &self,
        override_cam: Option<MountCameraOverride>,
        size: Option<(u32, u32)>,
    ) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some((width, height)) = size {
                let w = width.max(1);
                let h = height.max(1);
                if guard.last_size != Some((w, h)) {
                    guard.last_size = Some((w, h));
                    guard.pending_commands.push(ViewportCommand::Resize(w, h));
                }
            }
            guard.overlay_active = override_cam.is_some();
            guard
                .pending_commands
                .push(ViewportCommand::SetMountCamera(override_cam));
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
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
            texture_size: guard.texture_size,
            view_info: guard.view_info,
            is_mount_frame: guard.texture_is_mount_mode,
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

    pub fn dso_index(&self) -> Option<Arc<DsoSpatialIndex>> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.dso_index.clone()
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

    pub fn set_dso_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetDsoOpacity(clamped));
    }

    pub fn set_dso_catalog_filter(&self, show_messier: bool, show_ngc: bool) {
        self.send_command(ViewportCommand::SetDsoCatalogFilter {
            show_messier,
            show_ngc,
        });
    }

    pub fn set_solar_system_opacity(&self, opacity: f32) {
        let clamped = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.send_command(ViewportCommand::SetSolarSystemOpacity(clamped));
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

    pub fn set_trajectory(&self, data: Option<TrajectoryRenderData>) {
        self.send_command(ViewportCommand::SetTrajectory(data));
    }

    pub fn track_target(&self, target: Option<TrackingTarget>) {
        self.send_command(ViewportCommand::TrackTarget(target));
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
        let guard = self
            .inner
            .notify_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (_, listener) in guard.iter() {
            listener();
        }
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
    trajectory_entity: Entity,
    horizon_mesh: Handle<Mesh>,
    altaz_mesh: Handle<Mesh>,
    eq_mesh: Handle<Mesh>,
    trajectory_mesh: Handle<Mesh>,
    star_entity: Option<Entity>,
}

fn create_trajectory_mesh(points: &[([f32; 3], bool, f32)], minor_alpha: f32) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(points.len().saturating_sub(1) * 2);
    let mut line_tiers: Vec<f32> = Vec::with_capacity(points.len().saturating_sub(1) * 2);
    for pair in points.windows(2) {
        let (p0, a0, f0) = pair[0];
        let (p1, a1, f1) = pair[1];
        let base_alpha = if a0 && a1 { 1.0_f32 } else { minor_alpha };
        let alpha0 = (base_alpha * f0.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let alpha1 = (base_alpha * f1.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        positions.push(p0);
        positions.push(p1);
        line_tiers.push(-1.0 - alpha0);
        line_tiers.push(-1.0 - alpha1);
    }
    let mut mesh = Mesh::new(bevy::mesh::PrimitiveTopology::LineList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(grids::ATTRIBUTE_LINE_TIER, line_tiers);
    mesh
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

#[derive(Resource, Default, Clone, Copy)]
struct BevyStageTimer {
    cursor: Option<std::time::Instant>,
    camera_grid_us: u32,
    celestial_us: u32,
    hips_us: u32,
}

#[allow(clippy::needless_pass_by_value)]
fn mark_update_start(mut timer: ResMut<BevyStageTimer>) {
    timer.cursor = Some(std::time::Instant::now());
}

#[allow(clippy::needless_pass_by_value)]
fn mark_after_camera(mut timer: ResMut<BevyStageTimer>) {
    let now = std::time::Instant::now();
    timer.camera_grid_us = timer
        .cursor
        .map_or(0, |prev| metrics::duration_to_us(now.saturating_duration_since(prev)));
    timer.cursor = Some(now);
}

#[allow(clippy::needless_pass_by_value)]
fn mark_after_celestial(mut timer: ResMut<BevyStageTimer>) {
    let now = std::time::Instant::now();
    timer.celestial_us = timer
        .cursor
        .map_or(0, |prev| metrics::duration_to_us(now.saturating_duration_since(prev)));
    timer.cursor = Some(now);
}

#[allow(clippy::needless_pass_by_value)]
fn mark_after_hips(mut timer: ResMut<BevyStageTimer>) {
    let now = std::time::Instant::now();
    timer.hips_us = timer
        .cursor
        .map_or(0, |prev| metrics::duration_to_us(now.saturating_duration_since(prev)));
    timer.cursor = Some(now);
}

#[derive(Resource)]
struct CameraSyncState {
    view_info: PlanetariumViewInfo,
    is_interpolating: bool,
    is_mount_mode: bool,
    idle_update_interval: Duration,
}

impl Default for CameraSyncState {
    fn default() -> Self {
        Self {
            view_info: PlanetariumViewInfo::default(),
            is_interpolating: false,
            is_mount_mode: false,
            idle_update_interval: Duration::from_secs(5),
        }
    }
}

/// Alpha of the flat 0° horizon circle; hidden unless the horizon line setting is enabled.
fn horizon_line_alpha(settings: &landscape::HorizonRenderSettings) -> f32 {
    if settings.show_line { 0.85 } else { 0.0 }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn camera_movement_and_interpolation_system(
    time: Res<Time>,
    mut pending_commands: ResMut<PendingCommands>,
    mut observer_clock: ResMut<ObserverClockResource>,
    mut camera_query: Query<&mut PlanetariumCamera>,
    mut camera_sync: ResMut<CameraSyncState>,
    dims: Res<ViewportDimensions>,
    handles: Res<CelestialHandles>,
    star_settings: Res<stars::StarSettings>,
    atmo_settings: Res<atmosphere::AtmosphereSettings>,
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
            ViewportCommand::TrackTarget(target) => {
                camera.set_tracking_target(target);
                moved = true;
            }
            ViewportCommand::SetMountCamera(override_cam) => {
                camera.set_mount_override(override_cam);
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

    // When waking from idle sleep, `time.delta()` reflects the sleep duration rather than
    // an animation frame interval; cap the first animation step to ~60 Hz so slews and zooms
    // start smoothly without an initial jump.
    let dt = if camera_sync.is_interpolating {
        time.delta().as_secs_f64()
    } else {
        time.delta().as_secs_f64().min(1.0 / 60.0)
    };
    camera.update(dt);
    camera_sync.is_mount_mode = camera.is_mount_override_active();

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

    let is_interpolating = if camera.is_view_mode_transitioning() {
        true
    } else if camera.is_tracking_locked() {
        fov_diff > 1e-8 || target_fov_diff > 1e-7
    } else if camera.is_tracking_slewing() {
        true
    } else {
        yaw_diff > 1e-8
            || pitch_diff > 1e-8
            || fov_diff > 1e-8
            || target_yaw_diff > 1e-7
            || target_pitch_diff > 1e-7
            || target_fov_diff > 1e-7
    };

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
        if let Ok(mut mat) = line_materials.get_mut(handles.trajectory_entity) {
            mat.uniforms.transform =
                glam::DMat4::from_mat3(camera.topocentric_to_camera_matrix()).as_mat4();
            mat.uniforms.projection = proj_params.into();
        }

        if let Some(star_entity) = handles.star_entity {
            if let Ok(mut mat) = star_materials.get_mut(star_entity) {
                mat.uniforms.transform =
                    glam::DMat4::from_mat3(camera.icrs_to_camera_matrix()).as_mat4();
                mat.uniforms.projection = proj_params.into();
                mat.uniforms.viewport_size = Vec2::new(dims.width as f32, dims.height as f32);
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
            let proj_params = camera.projection_params(aspect);
            let sun_enu = celestial::solar::solar_topocentric_enu(
                camera.observer.location,
                camera.observer.epoch,
            );
            let sun_alt_deg = sun_enu.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
            let render_params = stars::StarRenderParams::for_sky(
                sun_alt_deg,
                proj_params.fov_y,
                atmo_settings.strength,
            );
            let topo_to_cam = camera.topocentric_to_camera_matrix();
            let zenith_cam = topo_to_cam.z_axis.normalize_or_zero();
            let sun_cam = (topo_to_cam * sun_enu).normalize_or_zero();

            mat.uniforms.limiting_mag = render_params.limiting_mag;
            mat.uniforms.fade_range = render_params.fade_range;
            mat.uniforms.base_radius = render_params.base_radius;
            mat.uniforms.min_radius = render_params.min_radius;
            mat.uniforms.max_radius = render_params.max_radius;
            mat.uniforms.opacity = star_settings.opacity;
            mat.uniforms.zenith_and_atmo = Vec4::new(
                zenith_cam.x as f32,
                zenith_cam.y as f32,
                zenith_cam.z as f32,
                atmo_settings.strength,
            );
            mat.uniforms.sun_cam_and_alt = Vec4::new(
                sun_cam.x as f32,
                sun_cam.y as f32,
                sun_cam.z as f32,
                sun_alt_deg,
            );
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
                if let Some(mut settings) =
                    app.world_mut().get_resource_mut::<stars::StarSettings>()
                {
                    settings.opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetDsoOpacity(opacity) => {
                if let Some(mut settings) = app.world_mut().get_resource_mut::<dso::DsoSettings>()
                {
                    settings.opacity = if opacity.is_finite() {
                        opacity.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetDsoCatalogFilter {
                show_messier,
                show_ngc,
            } => {
                if let Some(mut settings) = app.world_mut().get_resource_mut::<dso::DsoSettings>()
                {
                    settings.show_messier = show_messier;
                    settings.show_ngc = show_ngc;
                    *frames_remaining = 3;
                }
            }
            ViewportCommand::SetSolarSystemOpacity(opacity) => {
                if let Some(mut settings) = app
                    .world_mut()
                    .get_resource_mut::<solar_system::SolarSystemRenderSettings>()
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
                if let Some(mut settings) =
                    app.world_mut().get_resource_mut::<ground::GroundSettings>()
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
            ViewportCommand::SetTrajectory(maybe_traj) => {
                let handles = app.world().get_resource::<CelestialHandles>().cloned();
                if let Some(handles) = handles {
                    match maybe_traj {
                        Some(data) => {
                            let minor_alpha = if data.occlude_ground { 0.08 } else { 0.24 };
                            let new_mesh = create_trajectory_mesh(&data.points, minor_alpha);
                            if let Some(mut mesh) = app
                                .world_mut()
                                .resource_mut::<Assets<Mesh>>()
                                .get_mut(&handles.trajectory_mesh)
                            {
                                *mesh = new_mesh;
                            }
                            if let Some(mut mat) = app
                                .world_mut()
                                .get_mut::<CelestialLineMaterial>(handles.trajectory_entity)
                            {
                                let r = data.srgb[0] + (1.0 - data.srgb[0]) * 0.22;
                                let g = data.srgb[1] + (1.0 - data.srgb[1]) * 0.22;
                                let b = data.srgb[2] + (1.0 - data.srgb[2]) * 0.22;
                                mat.uniforms.color = Vec4::new(r, g, b, 0.92);
                                mat.uniforms.minor_alpha =
                                    if data.occlude_ground { 0.22 } else { 0.42 };
                            }
                        }
                        None => {
                            if let Some(mut mat) = app
                                .world_mut()
                                .get_mut::<CelestialLineMaterial>(handles.trajectory_entity)
                            {
                                mat.uniforms.color.w = 0.0;
                            }
                        }
                    }
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
                    render_image.texture_descriptor.usage = TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC;
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

                let trajectory_mesh_handle = app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(create_trajectory_mesh(&[], 0.22));
                let trajectory_material = CelestialLineMaterial {
                    uniforms: CelestialLineUniforms {
                        transform: glam::DMat4::from_mat3(initial_topo_to_cam).as_mat4(),
                        projection: initial_proj.into(),
                        color: Vec4::new(1.0, 1.0, 1.0, 0.0),
                        minor_alpha: 0.22,
                    },
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
                        let sun_enu = celestial::solar::solar_topocentric_enu(
                            initial_camera.observer.location,
                            initial_camera.observer.epoch,
                        );
                        let sun_alt_deg = sun_enu.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
                        let render_params =
                            stars::StarRenderParams::for_sky(sun_alt_deg, initial_proj.fov_y, 1.0);
                        let zenith_cam = initial_topo_to_cam.z_axis.normalize_or_zero();
                        let sun_cam = (initial_topo_to_cam * sun_enu).normalize_or_zero();
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
                                zenith_and_atmo: Vec4::new(
                                    zenith_cam.x as f32,
                                    zenith_cam.y as f32,
                                    zenith_cam.z as f32,
                                    1.0,
                                ),
                                sun_cam_and_alt: Vec4::new(
                                    sun_cam.x as f32,
                                    sun_cam.y as f32,
                                    sun_cam.z as f32,
                                    sun_alt_deg,
                                ),
                            },
                            stars: buffer_handle,
                            star_count: star_count as u32,
                        }
                    };

                    Some((star_material, mesh_handle))
                } else {
                    None
                };

                let dso_catalog = {
                    let _span = tracing::info_span!("load_dso_catalog_step").entered();
                    dso::load_dso_catalog()
                };
                let dso_setup = if let Some(catalog) = dso_catalog {
                    let dso_count = catalog.records.len();
                    let gpu_records: Vec<dso::DsoGpuRecord> = catalog
                        .records
                        .iter()
                        .map(dso::DsoGpuRecord::from_record)
                        .collect();
                    {
                        let _span = tracing::info_span!("build_dso_spatial_index", dso_count).entered();
                        let index = Arc::new(dso::DsoSpatialIndex::build(catalog.records));
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        guard.dso_index = Some(index);
                    }
                    let buffer_handle = {
                        let raw_bytes: &[u8] = bytemuck::cast_slice(&gpu_records);
                        let storage_buffer =
                            ShaderBuffer::new(raw_bytes, RenderAssetUsages::RENDER_WORLD);
                        app.world_mut()
                            .resource_mut::<Assets<ShaderBuffer>>()
                            .add(storage_buffer)
                    };
                    let mesh_handle = app
                        .world_mut()
                        .resource_mut::<Assets<Mesh>>()
                        .add(stars::create_star_quad_mesh());

                    let sun_alt_deg = celestial::solar::solar_altitude_deg(
                        initial_camera.observer.location,
                        initial_camera.observer.epoch,
                    ) as f32;
                    let render_params = dso::DsoRenderParams::for_sky(
                        sun_alt_deg,
                        initial_proj.fov_y,
                        1.0,
                        true,
                        true,
                    );
                    let dso_material = dso::CelestialDsoMaterial {
                        uniforms: dso::CelestialDsoUniforms {
                            transform: glam::DMat4::from_mat3(initial_eq_to_cam).as_mat4(),
                            projection: initial_proj.into(),
                            viewport_size: Vec2::new(width as f32, height as f32),
                            ngc_limiting_mag: render_params.ngc_limiting_mag,
                            messier_boost: render_params.messier_boost,
                            fade_range: render_params.fade_range,
                            opacity: 1.0,
                            catalog_mask: 3,
                            atmo_visibility: render_params.atmo_visibility,
                        },
                        dsos: buffer_handle,
                        dso_count: dso_count as u32,
                    };
                    Some((dso_material, mesh_handle))
                } else {
                    None
                };

                app.init_resource::<PendingCommands>();
                app.init_resource::<ObserverClockResource>();
                app.init_resource::<CameraSyncState>();
                app.init_resource::<BevyStageTimer>();
                app.init_resource::<atmosphere::AtmosphereSettings>();
                app.init_resource::<ground::GroundSettings>();
                app.init_resource::<stars::StarSettings>();
                app.init_resource::<dso::DsoSettings>();
                app.init_resource::<solar_system::SolarSystemRenderSettings>();
                let moon_textures = {
                    let _span = tracing::info_span!("init_fallback_moon_textures").entered();
                    let mut images = app.world_mut().resource_mut::<Assets<Image>>();
                    solar_system::fallback_moon_textures(&mut images)
                };
                app.insert_resource(moon_textures.clone());
                app.insert_resource(solar_system::SolarSystemTextureLoader::spawn(
                    bridge.clone(),
                ));
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
                    let fallback_image = app
                        .world_mut()
                        .resource_mut::<Assets<Image>>()
                        .add(fallback);
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

                let solar_system_quad_mesh = app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .add(stars::create_star_quad_mesh());

                let (horizon_entity, altaz_entity, eq_entity, trajectory_entity, star_entity) = {
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
                    let trajectory = world
                        .spawn((Mesh3d(trajectory_mesh_handle.clone()), trajectory_material))
                        .id();
                    let star = star_setup
                        .map(|(material, mesh)| world.spawn((Mesh3d(mesh), material)).id());
                    if let Some((material, mesh)) = dso_setup {
                        world.spawn((Mesh3d(mesh), material));
                    }
                    world.spawn((
                        Mesh3d(solar_system_quad_mesh),
                        solar_system::CelestialSolarSystemMaterial::from_camera(
                            &initial_camera,
                            initial_proj,
                            Vec2::new(width as f32, height as f32),
                            1.0,
                            1.0,
                            1.0,
                            &moon_textures,
                        ),
                    ));
                    world.spawn(sky::CelestialSkyMaterial::from_camera(
                        &initial_camera,
                        initial_proj,
                        1.0,
                        1.0,
                    ));
                    world.spawn(
                        landscape::CelestialLandscapeMaterial::from_camera_and_settings(
                            &initial_camera,
                            initial_proj,
                            &initial_horizon_settings,
                        ),
                    );
                    (horizon, altaz, eq, trajectory, star)
                };

                app.insert_resource(CelestialHandles {
                    horizon_entity,
                    altaz_entity,
                    eq_entity,
                    trajectory_entity,
                    horizon_mesh: horizon_mesh_handle,
                    altaz_mesh: altaz_mesh_handle,
                    eq_mesh: eq_mesh_handle,
                    trajectory_mesh: trajectory_mesh_handle,
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
                        mark_update_start,
                        camera_movement_and_interpolation_system,
                        mark_after_camera,
                        dso::dso_system,
                        sky::sky_system,
                        solar_system::poll_solar_system_textures_system,
                        solar_system::solar_system_system,
                        landscape::landscape_system,
                        mark_after_celestial,
                        hips::hips_survey_system,
                        hips::hips_grid_system,
                        mark_after_hips,
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

                struct PresentSlot {
                    texture: wgpu::Texture,
                    view: Arc<wgpu::TextureView>,
                }

                let mut present_ring: Vec<PresentSlot> = Vec::new();
                let mut present_size: Option<(u32, u32)> = None;
                let mut present_index: usize = 0;
                let mut frames_remaining = 0;
                let mut current_width = width;
                let mut current_height = height;
                let mut is_first_frame = true;
                let mut last_frame_start: Option<std::time::Instant> = None;
                let mut last_frame_finished_at: Option<std::time::Instant> = None;
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

                    let command_count = commands.len().min(u16::MAX as usize) as u16;
                    let frame_metrics_start = std::time::Instant::now();

                    let gpu_sync_us = if let Some(sub_idx) = last_submission.take() {
                        let _span = tracing::info_span!("gpu_frame_sync").entered();
                        let sync_start = std::time::Instant::now();
                        let _ = sync_device.poll(wgpu::PollType::Wait {
                            submission_index: Some(sub_idx),
                            timeout: Some(Duration::from_millis(100)),
                        });
                        metrics::duration_to_us(sync_start.elapsed())
                    } else {
                        0
                    };

                    last_frame_start = Some(std::time::Instant::now());
                    let first_frame = is_first_frame;
                    #[cfg(all(debug_assertions, feature = "tracy"))]
                    let _frame_span = tracy_client::non_continuous_frame!("Frame");

                    let commands_start = std::time::Instant::now();
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
                    let commands_us = metrics::duration_to_us(commands_start.elapsed());

                    if let Some(mut stage_timer) =
                        app.world_mut().get_resource_mut::<BevyStageTimer>()
                    {
                        *stage_timer = BevyStageTimer::default();
                    }
                    let app_update_start = std::time::Instant::now();
                    {
                        let _span = tracing::info_span!("bevy_app_update").entered();
                        app.update();
                    }
                    let app_update_us = metrics::duration_to_us(app_update_start.elapsed());
                    let stage_timer = app
                        .world()
                        .get_resource::<BevyStageTimer>()
                        .copied()
                        .unwrap_or_default();
                    let render_us = app_update_us.saturating_sub(
                        stage_timer
                            .camera_grid_us
                            .saturating_add(stage_timer.celestial_us)
                            .saturating_add(stage_timer.hips_us),
                    );

                    // Copy the rendered frame into an immutable triple-buffered present slot so
                    // Bevy's next frame cannot overwrite the texture while GPUI is still drawing
                    // overlays projected from this frame's `view_info`.
                    let mut new_texture_view = None;
                    let mut rendered_with_overlay = false;
                    let mut submitted_copy = false;
                    let mut overlay_us = 0_u32;
                    let extract_start = std::time::Instant::now();
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

                                    if current_size == (current_width, current_height) {
                                        let copy_extent = wgpu::Extent3d {
                                            width: current_size.0,
                                            height: current_size.1,
                                            depth_or_array_layers: 1,
                                        };
                                        if present_size != Some(current_size) {
                                            present_ring = (0..3)
                                                .map(|i| {
                                                    let texture = sync_device.create_texture(
                                                        &wgpu::TextureDescriptor {
                                                            label: Some(&format!(
                                                                "planetarium_present_{i}"
                                                            )),
                                                            size: copy_extent,
                                                            mip_level_count: 1,
                                                            sample_count: 1,
                                                            dimension: wgpu::TextureDimension::D2,
                                                            format:
                                                                wgpu::TextureFormat::Bgra8UnormSrgb,
                                                            usage:
                                                                wgpu::TextureUsages::TEXTURE_BINDING
                                                                    | wgpu::TextureUsages::COPY_DST
                                                                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                                                            view_formats: &[],
                                                        },
                                                    );
                                                    let view = Arc::new(texture.create_view(
                                                        &wgpu::TextureViewDescriptor::default(),
                                                    ));
                                                    PresentSlot { texture, view }
                                                })
                                                .collect();
                                            present_size = Some(current_size);
                                            present_index = 0;
                                        }

                                        let slot =
                                            &present_ring[present_index % present_ring.len()];
                                        present_index = present_index.wrapping_add(1);
                                        let mut encoder = sync_device.create_command_encoder(
                                            &wgpu::CommandEncoderDescriptor {
                                                label: Some("planetarium_present_copy"),
                                            },
                                        );
                                        encoder.copy_texture_to_texture(
                                            wgpu_texture.as_image_copy(),
                                            slot.texture.as_image_copy(),
                                            copy_extent,
                                        );
                                        // Determine whether THIS rendered frame used the mount camera
                                        // from Bevy's world state (avoids racing with UI-thread toggles).
                                        let overlay_active = app
                                            .world()
                                            .get_resource::<CameraSyncState>()
                                            .is_some_and(|s| s.is_mount_mode);
                                        let mut overlay_needs_frame = false;
                                        if overlay_active {
                                            if let Ok(mut pass_guard) =
                                                bridge.inner.overlay_pass.lock()
                                            {
                                                if let Some(pass) = pass_guard.as_mut() {
                                                    let overlay_start = std::time::Instant::now();
                                                    overlay_needs_frame = pass(
                                                        &mut encoder,
                                                        &slot.view,
                                                        current_size.0,
                                                        current_size.1,
                                                    );
                                                    overlay_us = metrics::duration_to_us(
                                                        overlay_start.elapsed(),
                                                    );
                                                }
                                            }
                                        }
                                        if overlay_needs_frame {
                                            frames_remaining = frames_remaining.max(1);
                                        }
                                        last_submission = Some(
                                            sync_queue.submit(std::iter::once(encoder.finish())),
                                        );
                                        submitted_copy = true;
                                        rendered_with_overlay = overlay_active;
                                        new_texture_view = Some(slot.view.clone());
                                    }
                                }
                            }
                        }
                    }
                    if !submitted_copy {
                        last_submission = Some(sync_queue.submit(std::iter::empty()));
                    }
                    let present_copy_us =
                        metrics::duration_to_us(extract_start.elapsed()).saturating_sub(overlay_us);

                    let mut needs_frame_dirty = false;
                    if frames_remaining > 0 {
                        frames_remaining -= 1;
                        needs_frame_dirty = true;
                    }

                    let (view_info, is_interpolating, next_idle_interval, frame_is_mount_mode) =
                        if let Some(sync) = app.world().get_resource::<CameraSyncState>() {
                            (
                                sync.view_info,
                                sync.is_interpolating,
                                sync.idle_update_interval,
                                sync.is_mount_mode,
                            )
                        } else {
                            (
                                PlanetariumViewInfo::default(),
                                false,
                                Duration::from_secs(5),
                                false,
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
                            guard.texture_size = present_size;
                            guard.texture_is_mount_mode = rendered_with_overlay;
                            guard.view_info = view_info;
                        }
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

                    let finished_at = std::time::Instant::now();
                    let interval_us = last_frame_finished_at.map_or(0, |prev| {
                        metrics::duration_to_us(finished_at.saturating_duration_since(prev))
                    });
                    last_frame_finished_at = Some(finished_at);
                    let total_us = metrics::duration_to_us(
                        finished_at.saturating_duration_since(frame_metrics_start),
                    );
                    bridge.record_frame_sample(BevyFrameSample {
                        seq: 0,
                        finished_at,
                        interval_us,
                        total_us,
                        gpu_sync_us,
                        commands_us,
                        camera_grid_us: stage_timer.camera_grid_us,
                        celestial_us: stage_timer.celestial_us,
                        hips_us: stage_timer.hips_us,
                        render_us,
                        present_copy_us,
                        overlay_us,
                        width: current_width,
                        height: current_height,
                        command_count,
                        is_mount_mode: frame_is_mount_mode,
                        resized: resized_size.is_some(),
                    });

                    bridge.notify_frame_ready();
                }

                let mut guard = bridge
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.latest_texture = None;
                guard.texture_size = None;
            })
    }
}
