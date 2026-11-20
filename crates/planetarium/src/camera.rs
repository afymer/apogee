use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, PI, TAU};

use celestial::ObservationContext;
use celestial::coords::{
    Angle, AngleInterval, SphericalRect, cartesian_to_canonical_spherical,
    topocentric_enu_to_spherical,
};
use celestial::projection::{CameraProjection, ProjectionKind};
use glam::{DMat3, DMat4, DVec3, Mat4};

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
pub struct PlanetariumViewInfo {
    pub center_ra_deg: f64,
    pub center_dec_deg: f64,
    pub center_az_deg: f64,
    pub center_alt_deg: f64,
    pub fov_deg: f64,
    pub view_mode: ViewMode,
    pub projection: CameraProjection,
    pub camera_to_topo: DMat3,
    pub camera_to_icrs: DMat3,
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
            projection: CameraProjection::default(),
            camera_to_topo: DMat3::IDENTITY,
            camera_to_icrs: DMat3::IDENTITY,
        }
    }
}

impl PlanetariumViewInfo {
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

        // Conversion to prevent camera jump
        // TODO: handle orientation lerping
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
        self.yaw_vel = 0.0;
        self.pitch_vel = 0.0;
        self.zoom_anchor = None;
    }

    pub fn pan(&mut self, dx: f64, dy: f64, viewport_height: f64) {
        self.zoom_anchor = None;
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
    /// that cursor position throughout the smooth zoom interpolation.
    pub fn zoom_at(&mut self, delta: f64, cursor_ndc: (f64, f64), aspect_ratio: f32) {
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

    pub fn update(&mut self, dt_seconds: f64) {
        let dt = dt_seconds.clamp(0.0, 0.1);
        let factor = 1.0 - (-12.0 * dt).exp();

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
        match self.view_mode {
            ViewMode::Horizontal => self.camera_rotation_matrix(),
            ViewMode::Equatorial => {
                self.camera_rotation_matrix() * self.observer.topocentric_to_equatorial_matrix()
            }
        }
    }

    pub fn icrs_to_camera_matrix(&self) -> DMat3 {
        match self.view_mode {
            ViewMode::Horizontal => {
                self.camera_rotation_matrix() * self.observer.equatorial_to_topocentric_matrix()
            }
            ViewMode::Equatorial => self.camera_rotation_matrix(),
        }
    }

    pub fn icrs_to_camera_mat4(&self) -> Mat4 {
        DMat4::from_mat3(self.icrs_to_camera_matrix()).as_mat4()
    }

    pub fn topocentric_to_camera_mat4(&self) -> Mat4 {
        DMat4::from_mat3(self.topocentric_to_camera_matrix()).as_mat4()
    }

    pub fn projection_for_fov(fov_y: f64, aspect_ratio: f32) -> CameraProjection {
        let fov_deg = fov_y.to_degrees();
        let kind = if fov_deg >= 60.0 {
            ProjectionKind::Stereographic
        } else if fov_deg <= 20.0 {
            ProjectionKind::Perspective
        } else {
            ProjectionKind::Blended((60.0 - fov_deg) / 40.0)
        };

        CameraProjection::new(Angle::from_radians(fov_y), aspect_ratio as f64, kind)
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
            projection: self.projection(aspect_ratio),
            camera_to_topo: self.topocentric_to_camera_matrix().transpose(),
            camera_to_icrs: self.icrs_to_camera_matrix().transpose(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_spherical_bounds_poles_and_wrapping() {
        // Near north pole: longitude interval becomes full circle and latitude clamps at +90 deg.
        let north = compute_spherical_bounds(1.0, 85.0_f64.to_radians(), 20.0_f64.to_radians(), 1.0);
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
        let best_residual = (cam.camera_rotation_matrix() * v_anchor - v_cam_target).length_squared();
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
}
