use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, PI, TAU};

use celestial::ObservationContext;
use celestial::coords::{Angle, AngleInterval, SphericalRect};
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
pub struct PlanetariumViewInfo {
    pub center_ra_deg: f64,
    pub center_dec_deg: f64,
    pub center_az_deg: f64,
    pub center_alt_deg: f64,
    pub fov_deg: f64,
    pub view_mode: ViewMode,
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
        }
    }
}

use bevy::prelude::Component;

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
    }

    pub fn pan(&mut self, dx: f64, dy: f64, viewport_height: f64) {
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
        let zoom_factor = (-delta * 0.15).exp();
        self.target_fov_y =
            (self.target_fov_y * zoom_factor).clamp(Self::MIN_FOV_RAD, Self::MAX_FOV_RAD);
    }

    pub fn update(&mut self, dt_seconds: f64) {
        let dt = dt_seconds.clamp(0.0, 0.1);
        let factor = 1.0 - (-12.0 * dt).exp();

        let mut diff_yaw = self.target_yaw - self.yaw;
        if diff_yaw > PI {
            diff_yaw -= TAU;
        } else if diff_yaw < -PI {
            diff_yaw += TAU;
        }

        self.yaw = (self.yaw + diff_yaw * factor).rem_euclid(TAU);
        self.pitch += (self.target_pitch - self.pitch) * factor;
        self.fov_y += (self.target_fov_y - self.fov_y) * factor;
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

    pub fn camera_rotation_matrix(&self) -> DMat3 {
        let (sin_y, cos_y) = self.yaw.sin_cos();
        let (sin_p, cos_p) = self.pitch.sin_cos();

        let (r, u, f) = match self.view_mode {
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

    pub fn projection(&self, aspect_ratio: f32) -> CameraProjection {
        let fov_deg = self.fov_y.to_degrees();
        let kind = if fov_deg >= 60.0 {
            ProjectionKind::Stereographic
        } else if fov_deg <= 20.0 {
            ProjectionKind::Perspective
        } else {
            ProjectionKind::Blended((60.0 - fov_deg) / 40.0)
        };

        CameraProjection::new(Angle::from_radians(self.fov_y), aspect_ratio as f64, kind)
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

    pub fn view_info(&self) -> PlanetariumViewInfo {
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
        }
    }
}
