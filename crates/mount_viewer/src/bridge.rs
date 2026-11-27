//! Thread-safe bridge between GPUI mount panels and the shared Planetarium Bevy renderer.
//!
//! Registers a [`MountOverlayRenderer`] callback on [`planetarium::BevyBridge`] so the 3D mount
//! and at-infinity limit widgets render directly onto the Planetarium's sky frame in the same
//! `wgpu::CommandEncoder` with zero frame lag.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use mount_renderer::{
    MountOverlayRenderer, MountSceneState, OrbitCamera, WidgetVisibility, step_angular_lowpass,
};
use planetarium::{BevyBridge, FrameListenerId, MountCameraOverride};
use protocol::MountLimits;

fn to_planetarium_override(camera: OrbitCamera) -> MountCameraOverride {
    let (az_rad, alt_rad, fov_y_rad) = camera.to_topocentric_angles();
    MountCameraOverride {
        az_rad,
        alt_rad,
        fov_y_rad,
    }
}

fn step_filtered_scene(displayed: &mut MountSceneState, target: &MountSceneState, dt: f64) -> bool {
    let moving_eq = step_angular_lowpass(
        &mut displayed.ra_deg,
        &mut displayed.dec_deg,
        target.ra_deg,
        target.dec_deg,
        dt,
    );
    let moving_hz = step_angular_lowpass(
        &mut displayed.az_deg,
        &mut displayed.alt_deg,
        target.az_deg,
        target.alt_deg,
        dt,
    );

    displayed.pier_side = target.pier_side;
    displayed.is_slewing = target.is_slewing;
    displayed.is_parked = target.is_parked;
    displayed.is_connected = target.is_connected;
    displayed.target_ra_deg = target.target_ra_deg;
    displayed.target_dec_deg = target.target_dec_deg;
    displayed.limits = target.limits;
    displayed.observer_lat_deg = target.observer_lat_deg;
    displayed.observer_lst_deg = target.observer_lst_deg;
    displayed.widgets = target.widgets;
    displayed.sky_limits_enabled = target.sky_limits_enabled;
    displayed.sky_limits_opacity = target.sky_limits_opacity;

    moving_eq || moving_hz
}

struct BridgeState {
    bevy_bridge: Option<BevyBridge>,
    frame_listener_id: Option<FrameListenerId>,
    camera: OrbitCamera,
    scene: MountSceneState,
    /// Active preview limits while a UI slider is being dragged (prevents daemon telemetry flicker).
    limits_preview: Option<MountLimits>,
    /// Snaps `displayed_scene` directly to `target_scene` on the next rendered frame.
    snap_next_frame: bool,
    has_received_attitude: bool,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            bevy_bridge: None,
            frame_listener_id: None,
            camera: OrbitCamera::default(),
            scene: MountSceneState::default(),
            limits_preview: None,
            snap_next_frame: true,
            has_received_attitude: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct MountViewerBridge {
    state: Arc<Mutex<BridgeState>>,
}

impl MountViewerBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches a `MountOverlayRenderer` to the shared Planetarium `BevyBridge`.
    pub fn attach_to_planetarium(
        &self,
        bevy_bridge: &BevyBridge,
        device: wgpu::Device,
        queue: wgpu::Queue,
        frame_listener_id: FrameListenerId,
    ) {
        let initial_scene = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.bevy_bridge = Some(bevy_bridge.clone());
            guard.frame_listener_id = Some(frame_listener_id);
            guard.scene.clone()
        };

        let state = self.state.clone();
        let bevy_for_cb = bevy_bridge.clone();
        let mut renderer = MountOverlayRenderer::new(device, queue);
        let mut displayed_scene = initial_scene;
        let mut last_frame_time = Instant::now();

        bevy_bridge.set_overlay_pass(Some(Box::new(move |encoder, color_view, width, height| {
            let (camera, mut target_scene, snap) = {
                let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
                let snap = guard.snap_next_frame;
                guard.snap_next_frame = false;
                (guard.camera, guard.scene.clone(), snap)
            };

            // Keep observer latitude & live LST synchronized with the Planetarium's live clock
            // so the 3D mount pose never drifts 15°/hr relative to the sky behind it.
            let obs = bevy_for_cb.view_info().observer;
            target_scene.observer_lat_deg = obs.location.latitude_deg();
            target_scene.observer_lst_deg = obs.local_sidereal_time().degrees();

            let now = Instant::now();
            let raw_dt = now.duration_since(last_frame_time).as_secs_f64();
            last_frame_time = now;
            // After an idle gap (>100ms), clamp first step to 1/60s so slews ease smoothly.
            let dt = if raw_dt > 0.1 { 1.0 / 60.0 } else { raw_dt };

            let still_moving = if snap {
                displayed_scene = target_scene;
                false
            } else {
                step_filtered_scene(&mut displayed_scene, &target_scene, dt)
            };

            let sun_enu = celestial::solar::solar_topocentric_enu(obs.location, obs.epoch);
            let sun_alt_deg = sun_enu.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
            let sun_world = glam::Vec3::new(sun_enu.x as f32, sun_enu.z as f32, -sun_enu.y as f32)
                .normalize_or_zero();
            let sun_dir_alt = [sun_world.x, sun_world.y, sun_world.z, sun_alt_deg];

            renderer.render_into(
                encoder,
                color_view,
                width,
                height,
                camera,
                &displayed_scene,
                sun_dir_alt,
            );
            still_moving
        })));
    }

    /// Atomically activates mount camera + overlay mode together with the laid-out viewport size,
    /// or resizes if already active.
    pub fn activate_mount_mode_with_size(&self, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        // Drop `self.state` lock before querying `bevy` to maintain strict lock ordering.
        let (bevy_bridge, camera) = {
            let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            (guard.bevy_bridge.clone(), guard.camera)
        };
        if let Some(bevy) = bevy_bridge {
            if !bevy.is_mount_camera_active() {
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .snap_next_frame = true;
                bevy.set_mount_camera_with_size(
                    Some(to_planetarium_override(camera)),
                    Some((w, h)),
                );
            } else if bevy.last_size() != Some((w, h)) {
                bevy.resize(w, h);
            }
        }
    }

    pub fn is_mount_camera_active(&self) -> bool {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone();
        bevy.is_some_and(|b| b.is_mount_camera_active())
    }

    pub fn snapshot(&self) -> Option<planetarium::BridgeSnapshot> {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone()?;
        Some(bevy.snapshot())
    }

    pub fn texture_view(&self) -> Option<Arc<wgpu::TextureView>> {
        self.snapshot()?.texture_view
    }

    pub fn last_size(&self) -> Option<(u32, u32)> {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone()?;
        bevy.last_size()
    }

    pub fn scene_state(&self) -> MountSceneState {
        let (mut scene, bevy) = {
            let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            (guard.scene.clone(), guard.bevy_bridge.clone())
        };
        if let Some(bevy) = bevy {
            let obs = bevy.view_info().observer;
            scene.observer_lat_deg = obs.location.latitude_deg();
            scene.observer_lst_deg = obs.local_sidereal_time().degrees();
        }
        scene
    }

    pub fn camera(&self) -> OrbitCamera {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .camera
    }

    pub fn widgets(&self) -> WidgetVisibility {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .scene
            .widgets
    }

    fn request_redraw_if_active(&self) {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone();
        if let Some(bevy) = bevy {
            if bevy.is_mount_camera_active() {
                bevy.request_redraw();
            }
        }
    }

    fn sync_camera_if_active(&self, camera: OrbitCamera) {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone();
        if let Some(bevy) = bevy {
            if bevy.is_mount_camera_active() {
                bevy.set_mount_camera(Some(to_planetarium_override(camera)));
            }
        }
    }

    pub fn update_scene(&self, f: impl FnOnce(&mut MountSceneState)) {
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut guard.scene);
        }
        self.request_redraw_if_active();
    }

    pub fn set_limits(&self, limits: MountLimits) {
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.limits_preview = None;
            guard.scene.limits = limits;
        }
        self.request_redraw_if_active();
    }

    pub fn set_limits_preview(&self, limits: MountLimits) {
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.limits_preview = Some(limits);
            guard.scene.limits = limits;
        }
        self.request_redraw_if_active();
    }

    pub fn apply_telemetry(&self, telemetry: &protocol::MountTelemetry, is_connected: bool) {
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.scene.apply_telemetry(telemetry, is_connected);
            if !guard.has_received_attitude {
                guard.has_received_attitude = true;
                guard.snap_next_frame = true;
            }
            if let Some(preview) = guard.limits_preview {
                guard.scene.limits = preview;
            }
        }
        self.request_redraw_if_active();
    }

    pub fn orbit(&self, delta_yaw: f32, delta_pitch: f32) {
        let camera = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.camera.yaw_rad += delta_yaw;
            guard.camera.pitch_rad = (guard.camera.pitch_rad + delta_pitch).clamp(-1.15, 1.52);
            guard.camera
        };
        self.sync_camera_if_active(camera);
    }

    pub fn look_along_ota(&self) {
        let dir = self.scene_state().ota_direction();
        let camera = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.camera = OrbitCamera::preset_along_ota(dir);
            guard.camera
        };
        self.sync_camera_if_active(camera);
    }

    pub fn zoom(&self, factor: f32) {
        let camera = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.camera.zoom(factor);
            guard.camera
        };
        self.sync_camera_if_active(camera);
    }

    pub fn set_camera(&self, camera: OrbitCamera) {
        {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.camera = camera;
        }
        self.sync_camera_if_active(camera);
    }

    pub fn resize(&self, width: u32, height: u32) {
        let bevy = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bevy_bridge
            .clone();
        if let Some(bevy) = bevy {
            bevy.resize(width, height);
        }
    }

    pub fn shutdown(&self) {
        let (bevy, listener_id) = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            (guard.bevy_bridge.take(), guard.frame_listener_id.take())
        };
        if let Some(bevy) = bevy {
            if let Some(id) = listener_id {
                bevy.remove_frame_listener(id);
            }
            bevy.set_overlay_pass(None);
            if bevy.is_mount_camera_active() {
                bevy.set_mount_camera(None);
            }
        }
    }
}
