use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, PI, TAU};

use celestial::coords::{
    Angle, AngleInterval, SphericalRect, canonical_spherical_to_cartesian,
    cartesian_to_canonical_spherical, topocentric_enu_to_spherical,
};
use celestial::projection::CameraProjection;
use celestial::{ObservationContext, SolarSystemBody};
use glam::{DMat3, DMat4, DQuat, DVec3, Mat4};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Horizontal,
    Equatorial,
}

impl ViewMode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Horizontal => "Alt/Az",
            Self::Equatorial => "RA/Dec",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProjectionParams {
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub blend_factor: f32,
    pub scale_factor: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyCoordinates {
    pub az_deg: f64,
    pub alt_deg: f64,
    pub ra_deg: f64,
    pub dec_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackingTarget {
    /// Fixed equatorial (ICRS) coordinates in radians.
    Equatorial { ra_rad: f64, dec_rad: f64 },
    /// Solar-system body whose ICRS direction is recomputed from observer location and epoch.
    SolarSystem(SolarSystemBody),
}

impl TrackingTarget {
    pub fn equatorial_deg(ra_deg: f64, dec_deg: f64) -> Self {
        Self::Equatorial {
            ra_rad: ra_deg.to_radians(),
            dec_rad: dec_deg.to_radians(),
        }
    }

    pub fn icrs_dir(&self, observer: &ObservationContext) -> DVec3 {
        match *self {
            Self::Equatorial { ra_rad, dec_rad } => canonical_spherical_to_cartesian(
                Angle::from_radians(ra_rad),
                Angle::from_radians(dec_rad),
            )
            .normalize_or_zero(),
            Self::SolarSystem(body) => body
                .ephemeris(observer.location, observer.epoch)
                .icrs_dir
                .normalize_or_zero(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanetariumViewInfo {
    pub center_ra_deg: f64,
    pub center_dec_deg: f64,
    pub center_az_deg: f64,
    pub center_alt_deg: f64,
    pub fov_deg: f64,
    pub view_mode: ViewMode,
    pub is_tracking: bool,
    pub projection: CameraProjection,
    pub camera_to_topo: DMat3,
    pub camera_to_icrs: DMat3,
    pub observer: ObservationContext,
}

impl Default for PlanetariumViewInfo {
    fn default() -> Self {
        Self {
            center_ra_deg: 0.0,
            center_dec_deg: 0.0,
            center_az_deg: 0.0,
            center_alt_deg: 90.0,
            fov_deg: 180.0,
            view_mode: ViewMode::Horizontal,
            is_tracking: false,
            projection: CameraProjection::default(),
            camera_to_topo: DMat3::IDENTITY,
            camera_to_icrs: DMat3::IDENTITY,
            observer: ObservationContext::default(),
        }
    }
}

impl PlanetariumViewInfo {
    /// Projects a unit topocentric ENU vector (`+X` East, `+Y` North, `+Z` Up) into screen NDC (`[-1, 1]^2`),
    /// returning `None` if culled behind the projection horizon.
    pub fn project_topo_to_ndc(&self, v_topo: DVec3) -> Option<(f64, f64)> {
        let v_cam = self.camera_to_topo.transpose() * v_topo;
        self.projection.project_cam_dir(v_cam)
    }
    /// Computes topocentric (Az/Alt) and equatorial (RA/Dec) sky coordinates at screen NDC (`[-1, 1]^2`).
    pub fn coords_at_ndc(&self, ndc_x: f64, ndc_y: f64) -> SkyCoordinates {
        let v_cam = self.projection.unproject_ndc(ndc_x, ndc_y);
        let v_topo = self.camera_to_topo * v_cam;
        let v_icrs = self.camera_to_icrs * v_cam;

        let (az, alt) = topocentric_enu_to_spherical(v_topo);
        let (ra, dec) = cartesian_to_canonical_spherical(v_icrs);

        SkyCoordinates {
            az_deg: az.degrees(),
            alt_deg: alt.degrees(),
            ra_deg: ra.degrees(),
            dec_deg: dec.degrees(),
        }
    }

    /// Projects a unit ICRS equatorial vector into screen NDC (`[-1, 1]^2`), returning `None` if culled.
    pub fn project_icrs_to_ndc(&self, v_icrs: DVec3) -> Option<(f64, f64)> {
        let v_cam = self.camera_to_icrs.transpose() * v_icrs;
        self.projection.project_cam_dir(v_cam)
    }

    /// Converts an ICRS equatorial direction into topocentric `(az_deg, alt_deg)`.
    pub fn icrs_to_topocentric_deg(&self, v_icrs: DVec3) -> (f64, f64) {
        let v_topo = self.camera_to_topo * (self.camera_to_icrs.transpose() * v_icrs);
        let (az, alt) = topocentric_enu_to_spherical(v_topo);
        (az.degrees(), alt.degrees())
    }
}

use bevy::prelude::Component;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ZoomAnchor {
    pub(crate) ndc: (f64, f64),
    pub(crate) aspect: f64,
    pub(crate) frame_dir: DVec3,
}

/// Duration of the roll animation played when switching between view modes.
const VIEW_MODE_TRANSITION_SECS: f64 = 0.5;

/// In-flight roll animation between the previous view mode's orientation and the live one.
///
/// The view direction is preserved by a mode switch; only the roll about it differs
/// (zenith-up vs. celestial-pole-up), so only that roll is decayed to zero.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewModeTransition {
    initial_roll: f64,
    progress: f64,
}

#[derive(Component, Debug, Clone)]
pub struct PlanetariumCamera {
    pub view_mode: ViewMode,
    pub observer: ObservationContext,
    pub yaw: f64,
    pub pitch: f64,
    pub fov_y: f64,
    pub target_yaw: f64,
    pub target_pitch: f64,
    pub target_fov_y: f64,
    pub yaw_vel: f64,
    pub pitch_vel: f64,
    pub(crate) zoom_anchor: Option<ZoomAnchor>,
    pub(crate) tracking_target: Option<TrackingTarget>,
    pub(crate) tracking_locked: bool,
    pub(crate) tracking_prev_target_dir: Option<DVec3>,
    pub(crate) mode_transition: Option<ViewModeTransition>,
    pub(crate) mount_override: Option<crate::viewport::MountCameraOverride>,
    pub(crate) saved_explore_view: Option<(ViewMode, f64, f64, f64)>,
}

impl Default for PlanetariumCamera {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::Horizontal,
            observer: ObservationContext::default(),
            yaw: 0.0,
            pitch: FRAC_PI_2,
            fov_y: PI,
            target_yaw: 0.0,
            target_pitch: FRAC_PI_2,
            target_fov_y: PI,
            yaw_vel: 0.0,
            pitch_vel: 0.0,
            zoom_anchor: None,
            tracking_target: None,
            tracking_locked: false,
            tracking_prev_target_dir: None,
            mode_transition: None,
            mount_override: None,
            saved_explore_view: None,
        }
    }
}

pub fn compute_spherical_bounds(
    center_lon: f64,
    center_lat: f64,
    fov_y: f64,
    aspect_ratio: f64,
) -> SphericalRect {
    if fov_y >= FRAC_PI_3 {
        return SphericalRect::full_sphere();
    }

    let half_fovy = fov_y * 0.5;
    let half_fovx = half_fovy * aspect_ratio.max(0.1);
    let theta_max = (half_fovy * half_fovy + half_fovx * half_fovx).sqrt() * 1.1;

    let lat_min = (center_lat - theta_max).max(-FRAC_PI_2);
    let lat_max = (center_lat + theta_max).min(FRAC_PI_2);

    let reaches_north_pole = center_lat + theta_max >= FRAC_PI_2;
    let reaches_south_pole = center_lat - theta_max <= -FRAC_PI_2;

    let lon_interval = if reaches_north_pole || reaches_south_pole {
        AngleInterval::full_circle()
    } else {
        let lat_extreme = lat_min.abs().max(lat_max.abs());
        let cos_lat = lat_extreme.cos();
        if cos_lat <= 1e-12 {
            AngleInterval::full_circle()
        } else {
            let delta_lon = theta_max / cos_lat;
            if delta_lon >= PI {
                AngleInterval::full_circle()
            } else {
                let lon_min = (center_lon - delta_lon).rem_euclid(TAU);
                let lon_max = (center_lon + delta_lon).rem_euclid(TAU);
                AngleInterval::from_radians(lon_min, lon_max)
            }
        }
    };

    SphericalRect::new(lon_interval, AngleInterval::from_radians(lat_min, lat_max))
}

fn wrap_pi(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}

fn slerp_unit(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let dot = a.dot(b).clamp(-1.0, 1.0);
    let theta = dot.acos();
    if theta < 1e-7 {
        return a.lerp(b, t).normalize_or_zero();
    }
    if theta > PI - 1e-4 {
        let fallback = if a.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
        let axis = a.cross(fallback).normalize();
        let angle = theta * t;
        return (a * angle.cos() + axis.cross(a) * angle.sin()).normalize();
    }
    let sin_theta = theta.sin();
    let w0 = ((1.0 - t) * theta).sin() / sin_theta;
    let w1 = (t * theta).sin() / sin_theta;
    (a * w0 + b * w1).normalize()
}

fn solve_orientation_for_anchor(
    view_mode: ViewMode,
    frame_dir: DVec3,
    cam_dir: DVec3,
    fallback_yaw: f64,
    fallback_pitch: f64,
) -> (f64, f64) {
    let ax = frame_dir.x;
    let ay = frame_dir.y;
    let az = frame_dir.z;
    let cx = cam_dir.x;
    let cy = cam_dir.y;
    let cz = cam_dir.z;

    let amp = cy.hypot(cz);
    let phi = cy.atan2(cz);
    let cand_pitches = if amp <= 1e-12 {
        [fallback_pitch.clamp(-FRAC_PI_2, FRAC_PI_2); 2]
    } else {
        let s = (az / amp).clamp(-1.0, 1.0).asin();
        [
            wrap_pi(s - phi).clamp(-FRAC_PI_2, FRAC_PI_2),
            wrap_pi(PI - s - phi).clamp(-FRAC_PI_2, FRAC_PI_2),
        ]
    };

    let ref_rot = PlanetariumCamera::rotation_matrix_for(view_mode, fallback_yaw, fallback_pitch);
    let evaluate = |cand_p: f64| -> (f64, f64, f64, f64) {
        let (sin_p, cos_p) = cand_p.sin_cos();
        let wy = cos_p * cz - sin_p * cy;
        let (sin_num, cos_num) = match view_mode {
            ViewMode::Horizontal => (ax * wy - ay * cx, ax * cx + ay * wy),
            ViewMode::Equatorial => (ax * cx + ay * wy, ax * wy - ay * cx),
        };
        let cand_yaw = if sin_num.hypot(cos_num) <= 1e-12 {
            fallback_yaw.rem_euclid(TAU)
        } else {
            sin_num.atan2(cos_num).rem_euclid(TAU)
        };
        let cand_rot = PlanetariumCamera::rotation_matrix_for(view_mode, cand_yaw, cand_p);
        let residual = (cand_rot * frame_dir - cam_dir).length_squared();
        let rot_trace = cand_rot.x_axis.dot(ref_rot.x_axis)
            + cand_rot.y_axis.dot(ref_rot.y_axis)
            + cand_rot.z_axis.dot(ref_rot.z_axis);
        let rot_dist = 3.0 - rot_trace;
        (cand_yaw, cand_p, residual, rot_dist)
    };

    let first = evaluate(cand_pitches[0]);
    let second = evaluate(cand_pitches[1]);
    let chosen = if (first.2 - second.2).abs() <= 1e-10 {
        if first.3 <= second.3 { first } else { second }
    } else if first.2 <= second.2 {
        first
    } else {
        second
    };
    (chosen.0, chosen.1)
}

impl PlanetariumCamera {
    pub const MIN_FOV_RAD: f64 = (1.0 / 3600.0) * (PI / 180.0);
    pub const MAX_FOV_RAD: f64 = PI;

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        if self.view_mode == mode {
            return;
        }

        // Capture the on-screen orientation (including any transition in flight) so the
        // roll difference between the two frames can be animated instead of snapping.
        let from_matrix = self.icrs_to_camera_matrix();

        // Conversion keeps the view direction fixed across the switch.
        match mode {
            ViewMode::Horizontal => {
                let (cur_az, cur_alt) = self.observer.equatorial_to_topocentric_coords(
                    Angle::from_radians(self.yaw),
                    Angle::from_radians(self.pitch),
                );
                let (tgt_az, tgt_alt) = self.observer.equatorial_to_topocentric_coords(
                    Angle::from_radians(self.target_yaw),
                    Angle::from_radians(self.target_pitch),
                );
                self.yaw = cur_az.radians();
                self.pitch = cur_alt.radians();
                self.target_yaw = tgt_az.radians();
                self.target_pitch = tgt_alt.radians();
            }
            ViewMode::Equatorial => {
                let (cur_ra, cur_dec) = self.observer.topocentric_to_equatorial_coords(
                    Angle::from_radians(self.yaw),
                    Angle::from_radians(self.pitch),
                );
                let (tgt_ra, tgt_dec) = self.observer.topocentric_to_equatorial_coords(
                    Angle::from_radians(self.target_yaw),
                    Angle::from_radians(self.target_pitch),
                );
                self.yaw = cur_ra.radians();
                self.pitch = cur_dec.radians();
                self.target_yaw = tgt_ra.radians();
                self.target_pitch = tgt_dec.radians();
            }
        }

        self.view_mode = mode;
        // Camera-space rotation taking the new orientation to the old one: a roll about +z.
        let relative = from_matrix * self.live_icrs_to_camera_matrix().transpose();
        self.mode_transition = Some(ViewModeTransition {
            initial_roll: relative.x_axis.y.atan2(relative.x_axis.x),
            progress: 0.0,
        });
        self.yaw_vel = 0.0;
        self.pitch_vel = 0.0;
        self.zoom_anchor = None;
        self.tracking_prev_target_dir = None;
    }

    pub fn is_view_mode_transitioning(&self) -> bool {
        self.mode_transition.is_some()
    }

    pub fn set_tracking_target(&mut self, target: Option<TrackingTarget>) {
        self.zoom_anchor = None;
        self.tracking_target = target;
        self.tracking_locked = false;
        if let Some(target) = target {
            let (target_dir, target_yaw, target_pitch) = self.resolve_target_in_frame(target);
            self.target_yaw = target_yaw;
            self.target_pitch = target_pitch;
            self.tracking_prev_target_dir = Some(target_dir);
        } else {
            self.target_yaw = self.yaw;
            self.target_pitch = self.pitch;
            self.tracking_prev_target_dir = None;
        }
    }

    pub fn is_tracking(&self) -> bool {
        self.tracking_target.is_some()
    }

    pub fn is_tracking_slewing(&self) -> bool {
        self.tracking_target.is_some() && !self.tracking_locked
    }

    pub fn is_tracking_locked(&self) -> bool {
        self.tracking_target.is_some() && self.tracking_locked
    }

    fn frame_dir_to_yaw_pitch(&self, dir: DVec3) -> (f64, f64) {
        match self.view_mode {
            ViewMode::Horizontal => {
                let (az, alt) = topocentric_enu_to_spherical(dir);
                (az.radians(), alt.radians())
            }
            ViewMode::Equatorial => {
                let (ra, dec) = cartesian_to_canonical_spherical(dir);
                (ra.radians(), dec.radians())
            }
        }
    }

    fn resolve_target_in_frame(&self, target: TrackingTarget) -> (DVec3, f64, f64) {
        let v_icrs = target.icrs_dir(&self.observer);
        let frame_dir = match self.view_mode {
            ViewMode::Horizontal => self
                .observer
                .equatorial_to_topocentric_dir(v_icrs)
                .normalize_or_zero(),
            ViewMode::Equatorial => v_icrs,
        };
        let (yaw, pitch) = self.frame_dir_to_yaw_pitch(frame_dir);
        (frame_dir, yaw, pitch)
    }

    pub fn pan(&mut self, dx: f64, dy: f64, viewport_height: f64) {
        self.zoom_anchor = None;
        self.tracking_target = None;
        self.tracking_locked = false;
        self.tracking_prev_target_dir = None;
        let proj = self.projection(1.0);
        let scale = proj.scale_factor();
        let h = viewport_height.max(1.0);
        let sensitivity = 2.0 * scale / h;

        let delta_pitch = dy * sensitivity;
        self.pitch = (self.pitch + delta_pitch).clamp(-FRAC_PI_2, FRAC_PI_2);
        self.target_pitch = self.pitch;

        let min_cos = (self.fov_y * 0.5).sin().max(1e-4);
        let cos_pitch = self.pitch.cos().abs().max(min_cos);

        let sign = match self.view_mode {
            ViewMode::Horizontal => -1.0,
            ViewMode::Equatorial => 1.0,
        };
        let delta_yaw = sign * dx * (sensitivity / cos_pitch);
        self.yaw = (self.yaw + delta_yaw).rem_euclid(TAU);
        self.target_yaw = self.yaw;
    }

    pub fn zoom(&mut self, delta: f64) {
        self.zoom_anchor = None;
        let zoom_factor = (-delta * 0.15).exp();
        self.target_fov_y =
            (self.target_fov_y * zoom_factor).clamp(Self::MIN_FOV_RAD, Self::MAX_FOV_RAD);
    }

    /// Zooms toward a normalized cursor coordinate (`[-1, 1]^2`), pinning the sky direction under
    /// that cursor position throughout the smooth zoom interpolation. When tracking an object,
    /// zooms toward the tracked center instead of pulling the object off-center.
    pub fn zoom_at(&mut self, delta: f64, cursor_ndc: (f64, f64), aspect_ratio: f32) {
        if self.tracking_target.is_some() {
            self.zoom(delta);
            return;
        }
        let aspect = f64::from(aspect_ratio);
        let zoom_factor = (-delta * 0.15).exp();
        let next_target_fov =
            (self.target_fov_y * zoom_factor).clamp(Self::MIN_FOV_RAD, Self::MAX_FOV_RAD);
        if (next_target_fov - self.target_fov_y).abs() <= 1e-12
            && (self.target_fov_y - self.fov_y).abs() <= 1e-9
        {
            return;
        }

        let reuse_anchor = self.zoom_anchor.filter(|existing| {
            (existing.ndc.0 - cursor_ndc.0).hypot(existing.ndc.1 - cursor_ndc.1) < 1e-3
                && (existing.aspect - aspect).abs() < 1e-4
        });

        let (frame_dir, fallback_yaw, fallback_pitch) = if let Some(existing) = reuse_anchor {
            (existing.frame_dir, self.target_yaw, self.target_pitch)
        } else {
            let v_cam_now = self
                .projection(aspect_ratio)
                .unproject_ndc(cursor_ndc.0, cursor_ndc.1);
            let frame_dir = (self.camera_rotation_matrix().transpose() * v_cam_now).normalize();
            (frame_dir, self.yaw, self.pitch)
        };

        self.target_fov_y = next_target_fov;
        let v_cam_target = Self::projection_for_fov(self.target_fov_y, aspect_ratio)
            .unproject_ndc(cursor_ndc.0, cursor_ndc.1);
        let (target_yaw, target_pitch) = solve_orientation_for_anchor(
            self.view_mode,
            frame_dir,
            v_cam_target,
            fallback_yaw,
            fallback_pitch,
        );
        self.target_yaw = target_yaw;
        self.target_pitch = target_pitch;
        let rho = frame_dir.x.hypot(frame_dir.y);
        self.zoom_anchor = if v_cam_target.x.abs() <= rho * 0.995 {
            Some(ZoomAnchor {
                ndc: cursor_ndc,
                aspect,
                frame_dir,
            })
        } else {
            None
        };
    }

    pub fn is_mount_override_active(&self) -> bool {
        self.mount_override.is_some()
    }

    pub fn set_mount_override(
        &mut self,
        override_cam: Option<crate::viewport::MountCameraOverride>,
    ) {
        match override_cam {
            Some(ov) => {
                if self.saved_explore_view.is_none() {
                    self.saved_explore_view =
                        Some((self.view_mode, self.yaw, self.pitch, self.fov_y));
                }
                self.mount_override = Some(ov);
                self.view_mode = ViewMode::Horizontal;
                self.yaw = ov.az_rad.rem_euclid(TAU);
                self.pitch = ov.alt_rad.clamp(-FRAC_PI_2 + 1e-4, FRAC_PI_2 - 1e-4);
                self.fov_y = ov.fov_y_rad.clamp(Self::MIN_FOV_RAD, Self::MAX_FOV_RAD);
                self.target_yaw = self.yaw;
                self.target_pitch = self.pitch;
                self.target_fov_y = self.fov_y;
                self.mode_transition = None;
                self.zoom_anchor = None;
            }
            None => {
                if self.mount_override.take().is_some() {
                    if let Some((mode, yaw, pitch, fov_y)) = self.saved_explore_view.take() {
                        self.view_mode = mode;
                        self.yaw = yaw;
                        self.pitch = pitch;
                        self.fov_y = fov_y;
                        self.target_yaw = yaw;
                        self.target_pitch = pitch;
                        self.target_fov_y = fov_y;
                    }
                }
            }
        }
    }

    pub fn update(&mut self, dt_seconds: f64) {
        if self.mount_override.is_some() {
            return;
        }
        let dt = dt_seconds.clamp(0.0, 0.1);
        let factor = 1.0 - (-12.0 * dt).exp();

        if let Some(transition) = self.mode_transition.as_mut() {
            transition.progress += dt / VIEW_MODE_TRANSITION_SECS;
            if transition.progress >= 1.0 {
                self.mode_transition = None;
            }
        }

        if let Some(target) = self.tracking_target {
            self.fov_y += (self.target_fov_y - self.fov_y) * factor;
            if (self.target_fov_y - self.fov_y).abs() <= 1e-9 {
                self.fov_y = self.target_fov_y;
            }

            let (target_dir, target_yaw, target_pitch) = self.resolve_target_in_frame(target);
            self.target_yaw = target_yaw;
            self.target_pitch = target_pitch;

            if self.tracking_locked {
                self.yaw = target_yaw;
                self.pitch = target_pitch;
                self.tracking_prev_target_dir = Some(target_dir);
            } else {
                let mut current_dir = self.forward_direction();
                if let Some(prev_target_dir) = self.tracking_prev_target_dir {
                    let target_step = prev_target_dir.dot(target_dir).clamp(-1.0, 1.0).acos();
                    if target_step > 0.0 && target_step < std::f64::consts::FRAC_PI_2 {
                        // Feed-forward continuous sidereal/orbital motion so slewing converges
                        // with zero steady-state lag even at maximum zoom.
                        current_dir = (DQuat::from_rotation_arc(prev_target_dir, target_dir)
                            * current_dir)
                            .normalize_or_zero();
                    }
                }
                self.tracking_prev_target_dir = Some(target_dir);

                let theta = current_dir.dot(target_dir).clamp(-1.0, 1.0).acos();
                let lock_threshold = (self.fov_y * 1e-4).clamp(1e-8, 1e-5);
                if theta <= lock_threshold {
                    self.yaw = target_yaw;
                    self.pitch = target_pitch;
                    self.tracking_locked = true;
                } else {
                    let slew_factor = 1.0 - (-9.0 * dt).exp();
                    let next_dir = slerp_unit(current_dir, target_dir, slew_factor);
                    let (ny, np) = self.frame_dir_to_yaw_pitch(next_dir);
                    self.yaw = ny;
                    self.pitch = np;
                }
            }
            return;
        }

        if let Some(anchor) = self.zoom_anchor {
            self.fov_y += (self.target_fov_y - self.fov_y) * factor;
            if (self.target_fov_y - self.fov_y).abs() <= 1e-7 {
                self.fov_y = self.target_fov_y;
                self.yaw = self.target_yaw;
                self.pitch = self.target_pitch;
                self.zoom_anchor = None;
                return;
            }
            let v_cam = Self::projection_for_fov(self.fov_y, anchor.aspect as f32)
                .unproject_ndc(anchor.ndc.0, anchor.ndc.1);
            let rho = anchor.frame_dir.x.hypot(anchor.frame_dir.y);
            if v_cam.x.abs() <= rho * 0.995 {
                let (yaw, pitch) = solve_orientation_for_anchor(
                    self.view_mode,
                    anchor.frame_dir,
                    v_cam,
                    self.yaw,
                    self.pitch,
                );
                self.yaw = yaw;
                self.pitch = pitch;
                return;
            }
            self.zoom_anchor = None;
        } else {
            self.fov_y += (self.target_fov_y - self.fov_y) * factor;
        }

        let mut diff_yaw = self.target_yaw - self.yaw;
        if diff_yaw > PI {
            diff_yaw -= TAU;
        } else if diff_yaw < -PI {
            diff_yaw += TAU;
        }

        self.yaw = (self.yaw + diff_yaw * factor).rem_euclid(TAU);
        self.pitch += (self.target_pitch - self.pitch) * factor;
    }

    pub fn forward_direction(&self) -> DVec3 {
        let (sin_y, cos_y) = self.yaw.sin_cos();
        let (sin_p, cos_p) = self.pitch.sin_cos();
        match self.view_mode {
            ViewMode::Horizontal => DVec3::new(cos_p * sin_y, cos_p * cos_y, sin_p),
            ViewMode::Equatorial => DVec3::new(cos_p * cos_y, cos_p * sin_y, sin_p),
        }
    }

    pub fn forward_direction_topo(&self) -> DVec3 {
        match self.view_mode {
            ViewMode::Horizontal => self.forward_direction(),
            ViewMode::Equatorial => self
                .observer
                .equatorial_to_topocentric_dir(self.forward_direction()),
        }
    }

    pub fn forward_direction_eq(&self) -> DVec3 {
        match self.view_mode {
            ViewMode::Horizontal => self
                .observer
                .topocentric_to_equatorial_dir(self.forward_direction()),
            ViewMode::Equatorial => self.forward_direction(),
        }
    }

    pub fn rotation_matrix_for(view_mode: ViewMode, yaw: f64, pitch: f64) -> DMat3 {
        let (sin_y, cos_y) = yaw.sin_cos();
        let (sin_p, cos_p) = pitch.sin_cos();

        let (r, u, f) = match view_mode {
            ViewMode::Horizontal => {
                let r = DVec3::new(cos_y, -sin_y, 0.0);
                let u = DVec3::new(-sin_p * sin_y, -sin_p * cos_y, cos_p);
                let f = DVec3::new(cos_p * sin_y, cos_p * cos_y, sin_p);
                (r, u, f)
            }
            ViewMode::Equatorial => {
                let r = DVec3::new(sin_y, -cos_y, 0.0);
                let u = DVec3::new(-sin_p * cos_y, -sin_p * sin_y, cos_p);
                let f = DVec3::new(cos_p * cos_y, cos_p * sin_y, sin_p);
                (r, u, f)
            }
        };

        DMat3::from_cols(
            DVec3::new(r.x, u.x, f.x),
            DVec3::new(r.y, u.y, f.y),
            DVec3::new(r.z, u.z, f.z),
        )
    }

    pub fn camera_rotation_matrix(&self) -> DMat3 {
        Self::rotation_matrix_for(self.view_mode, self.yaw, self.pitch)
    }

    pub fn topocentric_to_camera_matrix(&self) -> DMat3 {
        if self.mode_transition.is_some() {
            return self.icrs_to_camera_matrix() * self.observer.topocentric_to_equatorial_matrix();
        }
        match self.view_mode {
            ViewMode::Horizontal => self.camera_rotation_matrix(),
            ViewMode::Equatorial => {
                self.camera_rotation_matrix() * self.observer.topocentric_to_equatorial_matrix()
            }
        }
    }

    fn live_icrs_to_camera_matrix(&self) -> DMat3 {
        match self.view_mode {
            ViewMode::Horizontal => {
                self.camera_rotation_matrix() * self.observer.equatorial_to_topocentric_matrix()
            }
            ViewMode::Equatorial => self.camera_rotation_matrix(),
        }
    }

    pub fn icrs_to_camera_matrix(&self) -> DMat3 {
        let live = self.live_icrs_to_camera_matrix();
        let Some(transition) = self.mode_transition else {
            return live;
        };
        let t = transition.progress.clamp(0.0, 1.0);
        let eased = t * t * (3.0 - 2.0 * t);
        let (sin_r, cos_r) = (transition.initial_roll * (1.0 - eased)).sin_cos();
        let roll = DMat3::from_cols(
            DVec3::new(cos_r, sin_r, 0.0),
            DVec3::new(-sin_r, cos_r, 0.0),
            DVec3::Z,
        );
        roll * live
    }

    pub fn icrs_to_camera_mat4(&self) -> Mat4 {
        DMat4::from_mat3(self.icrs_to_camera_matrix()).as_mat4()
    }

    pub fn topocentric_to_camera_mat4(&self) -> Mat4 {
        DMat4::from_mat3(self.topocentric_to_camera_matrix()).as_mat4()
    }

    pub fn projection_for_fov(fov_y: f64, aspect_ratio: f32) -> CameraProjection {
        CameraProjection::for_fov(fov_y, aspect_ratio)
    }

    pub fn projection(&self, aspect_ratio: f32) -> CameraProjection {
        Self::projection_for_fov(self.fov_y, aspect_ratio)
    }

    pub fn projection_params(&self, aspect_ratio: f32) -> ProjectionParams {
        let proj = self.projection(aspect_ratio);
        ProjectionParams {
            fov_y: proj.fov_y.radians() as f32,
            aspect_ratio,
            blend_factor: proj.blend_factor() as f32,
            scale_factor: proj.scale_factor() as f32,
        }
    }

    pub fn visible_bounds(&self, aspect_ratio: f64) -> SphericalRect {
        compute_spherical_bounds(self.yaw, self.pitch, self.fov_y, aspect_ratio)
    }

    pub fn grid_level(&self, subdivision_count: u32) -> u32 {
        self.grid_zoom_factor(subdivision_count).0
    }

    pub fn grid_zoom_factor(&self, subdivision_count: u32) -> (u32, f32) {
        let n = subdivision_count.max(2) as f64;
        let ratio = (TAU / self.fov_y.max(Self::MIN_FOV_RAD * 0.5)).max(1.0);
        let level_f = ratio.log(n);
        let base_floor = level_f.floor();
        let level = (base_floor as u32) + 1;
        let frac = (level_f - base_floor).clamp(0.0, 1.0) as f32;
        let minor_alpha = frac * frac * (3.0 - 2.0 * frac);
        (level, minor_alpha)
    }

    /// Computes the idle redraw interval so Earth's sidereal rotation moves
    /// by at most ~0.25 physical pixels per update, clamped between 16 ms (~60 Hz)
    /// at high zoom and 5 s at wide FOV.
    pub fn sky_rotation_update_interval(&self, viewport_height_px: u32) -> std::time::Duration {
        const SIDEREAL_RAD_PER_SEC: f64 = 7.292_115e-5;
        const MAX_DRIFT_PX: f64 = 0.25;
        const MIN_INTERVAL_SECS: f64 = 0.016;
        const MAX_INTERVAL_SECS: f64 = 5.0;

        let height_px = f64::from(viewport_height_px.max(1));
        // Factor of 2 accounts for stereographic edge magnification.
        let rad_per_px = self.fov_y.max(Self::MIN_FOV_RAD) / (height_px * 2.0);
        let secs = (MAX_DRIFT_PX * rad_per_px) / SIDEREAL_RAD_PER_SEC;
        std::time::Duration::from_secs_f64(secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS))
    }

    pub fn view_info(&self, aspect_ratio: f32) -> PlanetariumViewInfo {
        let (az, alt, ra, dec) = match self.view_mode {
            ViewMode::Horizontal => {
                let az = self.yaw.to_degrees();
                let alt = self.pitch.to_degrees();
                let (ra, dec) = self.observer.topocentric_to_equatorial_coords(
                    Angle::from_radians(self.yaw),
                    Angle::from_radians(self.pitch),
                );
                (az, alt, ra.degrees(), dec.degrees())
            }
            ViewMode::Equatorial => {
                let ra = self.yaw.to_degrees();
                let dec = self.pitch.to_degrees();
                let (az, alt) = self.observer.equatorial_to_topocentric_coords(
                    Angle::from_radians(self.yaw),
                    Angle::from_radians(self.pitch),
                );
                (az.degrees(), alt.degrees(), ra, dec)
            }
        };

        PlanetariumViewInfo {
            center_ra_deg: ra,
            center_dec_deg: dec,
            center_az_deg: az,
            center_alt_deg: alt,
            fov_deg: self.fov_y.to_degrees(),
            view_mode: self.view_mode,
            is_tracking: self.tracking_target.is_some(),
            projection: self.projection(aspect_ratio),
            camera_to_topo: self.topocentric_to_camera_matrix().transpose(),
            camera_to_icrs: self.icrs_to_camera_matrix().transpose(),
            observer: self.observer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_spherical_bounds_poles_and_wrapping() {
        // Near north pole: longitude interval becomes full circle and latitude clamps at +90 deg.
        let north =
            compute_spherical_bounds(1.0, 85.0_f64.to_radians(), 20.0_f64.to_radians(), 1.0);
        assert!((north.lon.span().radians() - TAU).abs() < 1e-10);
        assert!((north.lat.end().degrees() - 90.0).abs() < 1e-10);

        // Near south pole: longitude interval becomes full circle and latitude clamps at -90 deg.
        let south =
            compute_spherical_bounds(1.0, -85.0_f64.to_radians(), 20.0_f64.to_radians(), 1.0);
        assert!((south.lon.span().radians() - TAU).abs() < 1e-10);
        assert!((south.lat.start().degrees() + 90.0).abs() < 1e-10);

        // Centered near lon = 2 deg on the equator: interval wraps across 0 / 2pi.
        let wrapped =
            compute_spherical_bounds(2.0_f64.to_radians(), 0.0, 10.0_f64.to_radians(), 1.0);
        assert!(wrapped.lon.is_wrapping());
        assert!(wrapped.lon.contains(Angle::ZERO));
        assert!(wrapped.lon.contains(Angle::from_degrees(2.0)));
        assert!(!wrapped.lon.contains(Angle::PI));
    }

    #[test]
    fn test_set_view_mode_animates_roll_without_jump() {
        let mut camera = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: 1.1,
            pitch: 0.6,
            target_yaw: 1.1,
            target_pitch: 0.6,
            ..Default::default()
        };
        let before = camera.icrs_to_camera_matrix();
        camera.set_view_mode(ViewMode::Equatorial);
        assert!(camera.is_view_mode_transitioning());
        let start = camera.icrs_to_camera_matrix();
        assert!((start - before).abs_diff_eq(DMat3::ZERO, 1e-9));

        for _ in 0..60 {
            camera.update(1.0 / 60.0);
        }
        assert!(!camera.is_view_mode_transitioning());
        let end = camera.icrs_to_camera_matrix();
        let live =
            PlanetariumCamera::rotation_matrix_for(ViewMode::Equatorial, camera.yaw, camera.pitch);
        assert!((end - live).abs_diff_eq(DMat3::ZERO, 1e-9));
    }

    #[test]
    fn test_set_view_mode_preserves_world_forward_direction() {
        let mut camera = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: 1.2,
            pitch: 0.45,
            target_yaw: 1.2,
            target_pitch: 0.45,
            ..Default::default()
        };

        let eq_before = camera.forward_direction_eq();
        let topo_before = camera.forward_direction_topo();

        camera.set_view_mode(ViewMode::Equatorial);
        assert_eq!(camera.view_mode, ViewMode::Equatorial);
        assert!(camera.forward_direction_eq().distance(eq_before) < 1e-12);
        assert!(camera.forward_direction_topo().distance(topo_before) < 1e-12);

        camera.set_view_mode(ViewMode::Horizontal);
        assert_eq!(camera.view_mode, ViewMode::Horizontal);
        assert!(camera.forward_direction_eq().distance(eq_before) < 1e-12);
        assert!(camera.forward_direction_topo().distance(topo_before) < 1e-12);
    }

    #[test]
    fn test_coords_at_ndc_center_and_cardinal_offsets() {
        for mode in [ViewMode::Horizontal, ViewMode::Equatorial] {
            let yaw_rad = 90.0_f64.to_radians();
            let pitch_rad = 20.0_f64.to_radians();
            let camera = PlanetariumCamera {
                view_mode: mode,
                yaw: yaw_rad,
                pitch: pitch_rad,
                fov_y: 60.0_f64.to_radians(),
                target_yaw: yaw_rad,
                target_pitch: pitch_rad,
                target_fov_y: 60.0_f64.to_radians(),
                ..Default::default()
            };

            let info = camera.view_info(16.0 / 9.0);
            let center = info.coords_at_ndc(0.0, 0.0);

            assert!((center.az_deg - info.center_az_deg).abs() < 1e-9);
            assert!((center.alt_deg - info.center_alt_deg).abs() < 1e-9);
            assert!((center.ra_deg - info.center_ra_deg).abs() < 1e-9);
            assert!((center.dec_deg - info.center_dec_deg).abs() < 1e-9);

            let up = info.coords_at_ndc(0.0, 0.25);
            let right = info.coords_at_ndc(0.25, 0.0);
            match mode {
                ViewMode::Horizontal => {
                    assert!(up.alt_deg > center.alt_deg);
                    assert!(right.az_deg > center.az_deg);
                }
                ViewMode::Equatorial => {
                    assert!(up.dec_deg > center.dec_deg);
                    // Looking outward at the celestial sphere (+RA increases East/left in Eq view)
                    assert!(right.ra_deg < center.ra_deg);
                }
            }
        }
    }

    #[test]
    fn test_zoom_at_keeps_cursor_sky_point_pinned_across_modes_and_projections() {
        let aspect = 16.0_f32 / 9.0;
        let modes = [ViewMode::Horizontal, ViewMode::Equatorial];
        let cases = [
            // Zoom in across Stereographic, Blended, Perspective, and crossing 60 -> 20 deg
            (120.0_f64, 2.0_f64),
            (45.0_f64, 2.0_f64),
            (12.0_f64, 2.0_f64),
            (80.0_f64, 10.0_f64),
            // Reachable zoom-outs in Blended and Perspective projections
            (45.0_f64, -2.0_f64),
            (12.0_f64, -2.0_f64),
        ];
        let ndc_points = [(0.45_f64, 0.3_f64), (-0.6_f64, -0.35_f64)];

        for mode in modes {
            for (fov_deg, delta) in cases {
                for (nx, ny) in ndc_points {
                    let yaw_rad = 110.0_f64.to_radians();
                    let pitch_rad = 35.0_f64.to_radians();
                    let fov_rad = fov_deg.to_radians();
                    let mut cam = PlanetariumCamera {
                        view_mode: mode,
                        yaw: yaw_rad,
                        pitch: pitch_rad,
                        fov_y: fov_rad,
                        target_yaw: yaw_rad,
                        target_pitch: pitch_rad,
                        target_fov_y: fov_rad,
                        ..Default::default()
                    };

                    let v_anchor = (cam.camera_rotation_matrix().transpose()
                        * cam.projection(aspect).unproject_ndc(nx, ny))
                    .normalize();

                    cam.zoom_at(delta, (nx, ny), aspect);

                    for step in 0..120 {
                        cam.update(1.0 / 60.0);
                        let projected = cam
                            .projection(aspect)
                            .project_cam_dir(cam.camera_rotation_matrix() * v_anchor)
                            .expect("anchored sky point must remain projectable during zoom");
                        let err = (projected.0 - nx).hypot(projected.1 - ny);
                        assert!(
                            err < 1e-6,
                            "drift at step={step}, mode={mode:?}, fov={fov_deg}, ndc=({nx}, {ny}), delta={delta}: got {projected:?} (err={err})"
                        );
                    }

                    assert!(
                        cam.zoom_anchor.is_none(),
                        "zoom_anchor should clear after convergence for mode={mode:?}, fov={fov_deg}, delta={delta}"
                    );
                    let final_proj = cam
                        .projection(aspect)
                        .project_cam_dir(cam.camera_rotation_matrix() * v_anchor)
                        .expect("anchored sky point must project cleanly at target");
                    let final_err = (final_proj.0 - nx).hypot(final_proj.1 - ny);
                    assert!(
                        final_err < 1e-6,
                        "final drift for mode={mode:?}, fov={fov_deg}, delta={delta}: {final_err}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_zoom_at_unreachable_wide_fov_clamps_smoothly_and_optimally() {
        let aspect = 16.0_f32 / 9.0;
        let ndc = (0.45_f64, 0.3_f64);
        let yaw_rad = 110.0_f64.to_radians();
        let pitch_rad = 35.0_f64.to_radians();
        let fov_rad = 120.0_f64.to_radians();
        let mut cam = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: yaw_rad,
            pitch: pitch_rad,
            fov_y: fov_rad,
            target_yaw: yaw_rad,
            target_pitch: pitch_rad,
            target_fov_y: fov_rad,
            ..Default::default()
        };

        let v_anchor = (cam.camera_rotation_matrix().transpose()
            * cam.projection(aspect).unproject_ndc(ndc.0, ndc.1))
        .normalize();

        cam.zoom_at(-2.0, ndc, aspect);

        for step in 0..120 {
            let prev_yaw = cam.yaw;
            let prev_pitch = cam.pitch;
            cam.update(1.0 / 60.0);
            assert!(cam.yaw.is_finite() && cam.pitch.is_finite());

            let dyaw = wrap_pi(cam.yaw - prev_yaw).abs();
            let dpitch = (cam.pitch - prev_pitch).abs();
            assert!(
                dyaw < 0.15 && dpitch < 0.15,
                "orientation jump at step={step}: dyaw={dyaw}, dpitch={dpitch}"
            );
        }

        assert!(cam.zoom_anchor.is_none());

        let v_cam_target = cam.projection(aspect).unproject_ndc(ndc.0, ndc.1);
        let best_residual =
            (cam.camera_rotation_matrix() * v_anchor - v_cam_target).length_squared();
        let eps = 1e-3;
        for sy in [-eps, 0.0, eps] {
            for sp in [-eps, 0.0, eps] {
                if sy == 0.0 && sp == 0.0 {
                    continue;
                }
                let ny = (cam.yaw + sy).rem_euclid(TAU);
                let np = (cam.pitch + sp).clamp(-FRAC_PI_2, FRAC_PI_2);
                let rot = PlanetariumCamera::rotation_matrix_for(cam.view_mode, ny, np);
                let neighbor_res = (rot * v_anchor - v_cam_target).length_squared();
                assert!(
                    neighbor_res + 1e-9 >= best_residual,
                    "neighbor ({sy}, {sp}) improved residual from {best_residual} to {neighbor_res}"
                );
            }
        }
    }

    #[test]
    fn test_zoom_at_rapid_ticks_and_pan_clears_anchor() {
        let aspect = 16.0_f32 / 9.0;
        let ndc = (0.35_f64, -0.25_f64);
        let mut cam = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: 75.0_f64.to_radians(),
            pitch: 40.0_f64.to_radians(),
            fov_y: 80.0_f64.to_radians(),
            target_yaw: 75.0_f64.to_radians(),
            target_pitch: 40.0_f64.to_radians(),
            target_fov_y: 80.0_f64.to_radians(),
            ..Default::default()
        };

        let v_anchor = (cam.camera_rotation_matrix().transpose()
            * cam.projection(aspect).unproject_ndc(ndc.0, ndc.1))
        .normalize();

        for _ in 0..5 {
            cam.zoom_at(1.0, ndc, aspect);
            cam.update(0.016);
        }

        for _ in 0..120 {
            cam.update(1.0 / 60.0);
        }

        assert!(cam.zoom_anchor.is_none());
        let projected = cam
            .projection(aspect)
            .project_cam_dir(cam.camera_rotation_matrix() * v_anchor)
            .expect("rapid ticks anchor must remain projectable");
        assert!((projected.0 - ndc.0).hypot(projected.1 - ndc.1) < 1e-6);

        // Scrolling outward when already clamped at MAX_FOV_RAD must be a no-op
        let mut max_cam = PlanetariumCamera::default();
        let yaw_before = max_cam.yaw;
        let pitch_before = max_cam.pitch;
        max_cam.zoom_at(-1.0, ndc, aspect);
        assert_eq!(max_cam.yaw, yaw_before);
        assert_eq!(max_cam.pitch, pitch_before);
        assert!(max_cam.zoom_anchor.is_none());

        // Calling pan mid-zoom must clear zoom_anchor
        cam.zoom_at(1.0, ndc, aspect);
        assert!(cam.zoom_anchor.is_some());
        cam.pan(12.0, -8.0, 900.0);
        assert!(cam.zoom_anchor.is_none());
    }

    #[test]
    fn test_tracking_smooth_centering_and_high_zoom_sidereal_follow() {
        let aspect = 16.0_f32 / 9.0;
        let mut cam = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: 120.0_f64.to_radians(),
            pitch: 35.0_f64.to_radians(),
            fov_y: (1.0 / 60.0_f64).to_radians(), // 1 arcminute high zoom
            target_yaw: 120.0_f64.to_radians(),
            target_pitch: 35.0_f64.to_radians(),
            target_fov_y: (1.0 / 60.0_f64).to_radians(),
            ..Default::default()
        };
        let t0 = 1_700_000_000.0;
        cam.observer.set_epoch_unix_seconds(t0);

        // Pick a star ~15 degrees away from the initial view center.
        let (star_ra, star_dec) = cam.observer.topocentric_to_equatorial_coords(
            Angle::from_degrees(135.0),
            Angle::from_degrees(42.0),
        );
        let target = TrackingTarget::Equatorial {
            ra_rad: star_ra.radians(),
            dec_rad: star_dec.radians(),
        };
        let initial_dir = cam.forward_direction();
        let target_dir_0 = cam
            .observer
            .equatorial_to_topocentric_dir(target.icrs_dir(&cam.observer))
            .normalize();
        let initial_angle = initial_dir.dot(target_dir_0).clamp(-1.0, 1.0).acos();
        assert!(initial_angle > 0.1);

        // Selecting the target must not snap immediately.
        cam.set_tracking_target(Some(target));
        assert!(cam.is_tracking());
        assert!(cam.is_tracking_slewing());
        assert!(cam.forward_direction().distance(initial_dir) < 1e-12);

        // First frame should advance smoothly (no snap).
        cam.update(1.0 / 60.0);
        let after_first = cam
            .forward_direction()
            .dot(target_dir_0)
            .clamp(-1.0, 1.0)
            .acos();
        assert!(after_first < initial_angle);
        assert!(after_first > initial_angle * 0.5);

        // Converge to lock while live time ticks forward.
        let mut t = t0;
        for _ in 0..180 {
            t += 1.0 / 60.0;
            cam.observer.set_epoch_unix_seconds(t);
            cam.update(1.0 / 60.0);
        }
        assert!(cam.is_tracking_locked());

        // Advance time by 30 seconds at 60 Hz under 1-arcmin FOV: star must stay dead center.
        for _ in 0..1800 {
            t += 1.0 / 60.0;
            cam.observer.set_epoch_unix_seconds(t);
            cam.update(1.0 / 60.0);
            let info = cam.view_info(aspect);
            let ndc = info
                .project_icrs_to_ndc(target.icrs_dir(&cam.observer))
                .expect("tracked star must remain projectable");
            assert!(
                ndc.0.hypot(ndc.1) < 1e-6,
                "tracked star drifted off center at high zoom: {ndc:?}"
            );
        }
    }

    #[test]
    fn test_tracking_zoom_pan_view_mode_and_time_jump() {
        let aspect = 16.0_f32 / 9.0;
        let mut cam = PlanetariumCamera {
            view_mode: ViewMode::Horizontal,
            yaw: 90.0_f64.to_radians(),
            pitch: 30.0_f64.to_radians(),
            fov_y: 40.0_f64.to_radians(),
            target_yaw: 90.0_f64.to_radians(),
            target_pitch: 30.0_f64.to_radians(),
            target_fov_y: 40.0_f64.to_radians(),
            ..Default::default()
        };
        let t0 = 1_700_000_000.0;
        cam.observer.set_epoch_unix_seconds(t0);

        let target = TrackingTarget::SolarSystem(SolarSystemBody::Moon);
        cam.set_tracking_target(Some(target));
        for _ in 0..180 {
            cam.update(1.0 / 60.0);
        }
        assert!(cam.is_tracking_locked());

        // Zooming with an off-center cursor while tracking must keep the object centered.
        cam.zoom_at(6.0, (0.6, -0.4), aspect);
        assert!(cam.zoom_anchor.is_none());
        for _ in 0..60 {
            cam.update(1.0 / 60.0);
            let ndc = cam
                .view_info(aspect)
                .project_icrs_to_ndc(target.icrs_dir(&cam.observer))
                .expect("tracked moon must stay projectable during zoom");
            assert!(ndc.0.hypot(ndc.1) < 1e-6);
        }

        // Switching view mode keeps tracking locked and centered.
        cam.set_view_mode(ViewMode::Equatorial);
        assert!(cam.is_tracking_locked());
        cam.update(1.0 / 60.0);
        let ndc_eq = cam
            .view_info(aspect)
            .project_icrs_to_ndc(target.icrs_dir(&cam.observer))
            .expect("tracked moon must remain centered after switching view mode");
        assert!(ndc_eq.0.hypot(ndc_eq.1) < 1e-6);

        // Time jump in Horizontal mode while locked snaps immediately to keep the target centered.
        cam.set_view_mode(ViewMode::Horizontal);
        cam.observer.set_epoch_unix_seconds(t0 + 3.0 * 3600.0);
        cam.update(1.0 / 60.0);
        assert!(cam.is_tracking_locked());
        let ndc_jump = cam
            .view_info(aspect)
            .project_icrs_to_ndc(target.icrs_dir(&cam.observer))
            .expect("tracked moon must remain centered immediately after time jump");
        assert!(ndc_jump.0.hypot(ndc_jump.1) < 1e-6);

        // Panning detaches tracking cleanly.
        cam.pan(25.0, -10.0, 900.0);
        assert!(!cam.is_tracking());
        assert!(!cam.is_tracking_locked());
    }

    #[test]
    fn set_mount_override_saves_and_restores_explore_view() {
        let mut camera = PlanetariumCamera {
            view_mode: ViewMode::Equatorial,
            yaw: 1.2,
            pitch: 0.5,
            fov_y: 60.0_f64.to_radians(),
            ..Default::default()
        };

        camera.set_mount_override(Some(crate::viewport::MountCameraOverride {
            az_rad: 2.0,
            alt_rad: -0.3,
            fov_y_rad: 42.0_f64.to_radians(),
        }));
        assert!(camera.is_mount_override_active());
        assert_eq!(camera.view_mode, ViewMode::Horizontal);
        assert!((camera.yaw - 2.0).abs() < 1e-6);
        assert!((camera.pitch + 0.3).abs() < 1e-6);

        // Clearing mount override restores the saved Explore view
        camera.set_mount_override(None);
        assert!(!camera.is_mount_override_active());
        assert_eq!(camera.view_mode, ViewMode::Equatorial);
        assert!((camera.yaw - 1.2).abs() < 1e-6);
        assert!((camera.pitch - 0.5).abs() < 1e-6);
        assert!((camera.fov_y - 60.0_f64.to_radians()).abs() < 1e-6);
    }
}
