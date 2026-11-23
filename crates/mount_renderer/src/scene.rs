//! Procedural NEQ6-Pro German Equatorial Mount + Optical Train + At-Infinity 3D Limit Widgets.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use celestial::ObservationContext;
use celestial::coords::Angle;
use glam::{Mat4, Vec3};
use protocol::{MountLimits, MountTelemetry, PierSide};

use crate::mesh::{LineBuilder, MeshBuilder, frame_from_y_axis, horizontal_dir};

pub const PIVOT_Y: f32 = 1.22;
pub const COMPASS_RADIUS: f32 = 1.65;
const INFINITY_WIDGET_RADIUS: f32 = 20.0;

/// Orthonormal equatorial basis in mount world space (`+X = East`, `+Y = Up`, `-Z = North`).
#[derive(Clone, Copy, Debug)]
pub struct EquatorialFrame {
    /// Unit vector toward the North Celestial Pole.
    pub u_pole: Vec3,
    /// Unit vector along the elevated celestial pole (`+y >= 0` in both hemispheres).
    pub u_elevated_pole: Vec3,
    /// Unit vector along the celestial equator on the local meridian (`HA = 0, Dec = 0`).
    pub u_meridian: Vec3,
    /// Unit vector pointing East (`+X`).
    pub u_east: Vec3,
}

impl EquatorialFrame {
    pub fn from_latitude_deg(lat_deg: f64) -> Self {
        let lat_rad = (lat_deg as f32).clamp(-89.0, 89.0).to_radians();
        let (sin_lat, cos_lat) = lat_rad.sin_cos();
        let u_pole = Vec3::new(0.0, sin_lat, -cos_lat).normalize();
        let u_elevated_pole = if lat_deg >= 0.0 { u_pole } else { -u_pole };
        let u_meridian = Vec3::new(0.0, cos_lat, sin_lat).normalize();
        Self {
            u_pole,
            u_elevated_pole,
            u_meridian,
            u_east: Vec3::X,
        }
    }

    /// Unit vector pointing to `(ha_rad, dec_rad)` on the celestial sphere.
    #[inline]
    pub fn sky_dir(&self, ha_rad: f32, dec_rad: f32) -> Vec3 {
        let (sin_ha, cos_ha) = ha_rad.sin_cos();
        let (sin_dec, cos_dec) = dec_rad.sin_cos();
        (self.u_pole * sin_dec + (self.u_meridian * cos_ha - self.u_east * sin_ha) * cos_dec)
            .normalize()
    }
}

/// Visibility toggles for the 3D scene widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WidgetVisibility {
    pub altitude_limits: bool,
    pub azimuth_sector: bool,
    pub meridian_flip: bool,
    pub ground_grid: bool,
}

impl Default for WidgetVisibility {
    fn default() -> Self {
        Self {
            altitude_limits: true,
            azimuth_sector: true,
            meridian_flip: true,
            ground_grid: true,
        }
    }
}

/// Complete input state needed to build the 3D mount scene.
#[derive(Clone, Debug, PartialEq)]
pub struct MountSceneState {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub pier_side: PierSide,
    pub is_slewing: bool,
    pub is_parked: bool,
    pub is_connected: bool,
    pub target_ra_deg: Option<f64>,
    pub target_dec_deg: Option<f64>,
    pub limits: MountLimits,
    pub observer_lat_deg: f64,
    pub observer_lst_deg: f64,
    pub widgets: WidgetVisibility,
    pub sky_limits_enabled: bool,
    pub sky_limits_opacity: f32,
}

impl Default for MountSceneState {
    fn default() -> Self {
        let ctx = ObservationContext::default();
        Self {
            ra_deg: 0.0,
            dec_deg: 90.0,
            az_deg: 0.0,
            alt_deg: ctx.location.latitude_deg(),
            pier_side: PierSide::West,
            is_slewing: false,
            is_parked: true,
            is_connected: false,
            target_ra_deg: None,
            target_dec_deg: None,
            limits: MountLimits::default(),
            observer_lat_deg: ctx.location.latitude_deg(),
            observer_lst_deg: ctx.local_sidereal_time().degrees(),
            widgets: WidgetVisibility::default(),
            sky_limits_enabled: true,
            sky_limits_opacity: 1.0,
        }
    }
}

impl MountSceneState {
    pub fn apply_telemetry(&mut self, t: &MountTelemetry, is_connected: bool) {
        self.ra_deg = t.ra_deg;
        self.dec_deg = t.dec_deg;
        self.az_deg = t.az_deg;
        self.alt_deg = t.alt_deg;
        self.pier_side = t.pier_side;
        self.is_slewing = t.is_slewing;
        self.is_parked = t.is_parked;
        self.is_connected = is_connected;
        self.target_ra_deg = t.target_ra_deg;
        self.target_dec_deg = t.target_dec_deg;
        self.limits = t.limits;
    }

    pub fn equatorial_frame(&self) -> EquatorialFrame {
        EquatorialFrame::from_latitude_deg(self.observer_lat_deg)
    }

    /// Hour angle in degrees `(-180, 180]`: negative = East of meridian, positive = West.
    pub fn hour_angle_deg(&self) -> f64 {
        Angle::from_degrees(self.observer_lst_deg - self.ra_deg).signed_degrees()
    }

    /// Unit vector of the optical tube pointing direction in the 3D scene (`+X=E, +Y=Up, -Z=N`).
    pub fn ota_direction(&self) -> Vec3 {
        let frame = self.equatorial_frame();
        let ha_rad = (self.hour_angle_deg() as f32).to_radians();
        let dec_rad = (self.dec_deg as f32).clamp(-90.0, 90.0).to_radians();
        frame.sky_dir(ha_rad, dec_rad)
    }
}

pub struct SceneGeometry {
    /// Translucent safety & meridian limit zones (projected at infinity, zero parallax).
    pub sky_translucent: MeshBuilder,
    /// Crisp HUD arcs, limit rims, meridian line & pointing reticle (projected at infinity).
    pub sky_lines: LineBuilder,
    /// Foreground 3D NEQ6-Pro mount & optical train (world space).
    pub opaque: MeshBuilder,
    /// Foreground world-space lines (ground compass & local OTA beam).
    pub lines: LineBuilder,
}

/// Builds the at-infinity limit widgets, 3D NEQ6-Pro mount, optical train, and ground compass.
pub fn build_scene(state: &MountSceneState) -> SceneGeometry {
    let mut sky_translucent = MeshBuilder::default();
    let mut sky_lines = LineBuilder::default();
    let mut opaque = MeshBuilder::default();
    let mut lines = LineBuilder::default();

    let neq6_white = [0.87, 0.89, 0.92, 1.0];
    let neq6_offwhite = [0.76, 0.79, 0.83, 1.0];
    let anodized_black = [0.14, 0.16, 0.20, 1.0];
    let dark_metal = [0.24, 0.27, 0.32, 1.0];
    let chrome = [0.78, 0.81, 0.85, 1.0];
    let dovetail_green = [0.16, 0.56, 0.45, 1.0];
    let accent_red = [0.78, 0.22, 0.24, 1.0];
    let lens_blue = [0.12, 0.24, 0.42, 1.0];

    // 1. Ground compass & tripod (fixed in topocentric ENU frame)
    if state.widgets.ground_grid {
        build_ground_compass(&mut lines);
    }

    let hub_y = 0.84_f32;
    let leg_spread = 0.62_f32;
    for i in 0..3 {
        let angle = -FRAC_PI_2 + (i as f32) * (2.0 * PI / 3.0);
        let foot = Vec3::new(angle.cos() * leg_spread, 0.0, angle.sin() * leg_spread);
        let top = Vec3::new(angle.cos() * 0.11, hub_y, angle.sin() * 0.11);
        opaque.add_strut(foot.lerp(top, 0.32), top, 0.024, 12, chrome);
        opaque.add_strut(foot, foot.lerp(top, 0.38), 0.017, 10, chrome);
        opaque.add_frustum(
            Mat4::from_translation(foot),
            0.025,
            0.020,
            0.0,
            0.04,
            10,
            anodized_black,
        );
        let clamp = foot.lerp(top, 0.35);
        opaque.add_strut(
            clamp - (top - foot).normalize() * 0.025,
            clamp + (top - foot).normalize() * 0.025,
            0.030,
            10,
            anodized_black,
        );
    }

    opaque.add_frustum(
        Mat4::from_translation(Vec3::new(0.0, 0.54, 0.0)),
        0.16,
        0.16,
        -0.012,
        0.012,
        18,
        anodized_black,
    );
    opaque.add_frustum(
        Mat4::from_translation(Vec3::new(0.0, hub_y, 0.0)),
        0.13,
        0.11,
        -0.04,
        0.07,
        20,
        neq6_white,
    );
    opaque.add_frustum(
        Mat4::from_translation(Vec3::new(0.0, hub_y + 0.07, 0.0)),
        0.095,
        0.095,
        0.0,
        0.06,
        20,
        anodized_black,
    );
    opaque.add_box(
        Mat4::from_translation(Vec3::new(0.0, hub_y + 0.18, 0.0)),
        Vec3::new(0.14, 0.16, 0.17),
        neq6_white,
    );

    // 2. Equatorial Head & Kinematics (RA / Dec / Pier Side, both hemispheres)
    let eq = state.equatorial_frame();
    let pivot = Vec3::new(0.0, PIVOT_Y, 0.0);

    // Orient RA housing upward along the elevated pole (NCP in North, SCP in South).
    let polar_frame = frame_from_y_axis(pivot, eq.u_elevated_pole);
    opaque.add_frustum(polar_frame, 0.088, 0.082, -0.18, 0.12, 24, neq6_white);
    opaque.add_frustum(polar_frame, 0.042, 0.038, -0.23, -0.18, 16, anodized_black);
    opaque.add_frustum(polar_frame, 0.092, 0.092, 0.10, 0.135, 24, anodized_black);

    let ha_rad = (state.hour_angle_deg() as f32).to_radians();
    let dec_rad = (state.dec_deg as f32).clamp(-90.0, 90.0).to_radians();
    let (sin_ha, cos_ha) = ha_rad.sin_cos();
    let ota_dir = eq.sky_dir(ha_rad, dec_rad);

    let effective_pier = match state.pier_side {
        PierSide::Unknown => {
            if ha_rad >= 0.0 {
                PierSide::East
            } else {
                PierSide::West
            }
        }
        other => other,
    };
    let saddle_dir = match effective_pier {
        PierSide::East => (eq.u_meridian * sin_ha + eq.u_east * cos_ha).normalize(),
        PierSide::West | PierSide::Unknown => {
            (-eq.u_meridian * sin_ha - eq.u_east * cos_ha).normalize()
        }
    };
    let cw_dir = -saddle_dir;

    let dec_center = pivot + eq.u_elevated_pole * 0.13;
    let dec_frame = frame_from_y_axis(dec_center, saddle_dir);
    opaque.add_frustum(dec_frame, 0.072, 0.068, -0.09, 0.17, 22, neq6_white);
    opaque.add_frustum(dec_frame, 0.078, 0.078, 0.15, 0.19, 22, anodized_black);

    let cw_start = dec_center + cw_dir * 0.08;
    let cw_end = dec_center + cw_dir * 0.62;
    opaque.add_strut(cw_start, cw_end, 0.014, 12, chrome);
    opaque.add_frustum(
        frame_from_y_axis(cw_end, cw_dir),
        0.028,
        0.026,
        -0.015,
        0.015,
        14,
        anodized_black,
    );
    for offset in [0.34_f32, 0.44_f32] {
        opaque.add_frustum(
            frame_from_y_axis(dec_center + cw_dir * offset, cw_dir),
            0.095,
            0.095,
            -0.036,
            0.036,
            24,
            neq6_offwhite,
        );
    }

    // 3. Saddle, Optical Tube Assembly (OTA), Focuser, DSLR & Guide Scope
    let saddle_pos = dec_center + saddle_dir * 0.21;
    let ota_side = ota_dir.cross(saddle_dir).normalize_or_zero();
    let ota_rot = Mat4::from_cols(
        ota_side.extend(0.0),
        ota_dir.extend(0.0),
        saddle_dir.extend(0.0),
        saddle_pos.extend(1.0),
    );

    opaque.add_box(
        ota_rot * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.015)),
        Vec3::new(0.11, 0.20, 0.032),
        anodized_black,
    );
    opaque.add_box(
        ota_rot * Mat4::from_translation(Vec3::new(0.0, 0.02, 0.042)),
        Vec3::new(0.065, 0.42, 0.022),
        dovetail_green,
    );

    let ota_frame = ota_rot * Mat4::from_translation(Vec3::new(0.0, 0.02, 0.165));

    for ring_y in [-0.15_f32, 0.15_f32] {
        opaque.add_frustum(
            ota_frame * Mat4::from_translation(Vec3::new(0.0, ring_y, 0.0)),
            0.108,
            0.108,
            -0.018,
            0.018,
            24,
            neq6_white,
        );
    }

    opaque.add_frustum(ota_frame, 0.095, 0.095, -0.32, 0.42, 28, neq6_white);
    opaque.add_frustum(ota_frame, 0.106, 0.106, 0.40, 0.62, 28, neq6_offwhite);
    opaque.add_frustum(ota_frame, 0.109, 0.109, 0.61, 0.635, 28, anodized_black);
    opaque.add_frustum(ota_frame, 0.090, 0.090, 0.57, 0.615, 24, lens_blue);

    // Rear focuser & dual-speed knobs
    opaque.add_frustum(ota_frame, 0.068, 0.088, -0.40, -0.32, 22, dark_metal);
    opaque.add_frustum(ota_frame, 0.044, 0.044, -0.49, -0.40, 20, anodized_black);
    let knob_frame = frame_from_y_axis(
        ota_frame.transform_point3(Vec3::new(-0.09, -0.37, 0.0)),
        ota_side,
    );
    opaque.add_frustum(knob_frame, 0.014, 0.014, 0.0, 0.18, 14, chrome);
    opaque.add_frustum(knob_frame, 0.024, 0.024, -0.015, 0.02, 16, anodized_black);
    opaque.add_frustum(knob_frame, 0.024, 0.024, 0.16, 0.195, 16, anodized_black);
    opaque.add_frustum(
        knob_frame,
        0.015,
        0.015,
        0.195,
        0.215,
        14,
        [0.85, 0.66, 0.25, 1.0],
    );

    // DSLR Camera body
    opaque.add_box(
        ota_frame * Mat4::from_translation(Vec3::new(0.0, -0.545, 0.0)),
        Vec3::new(0.135, 0.075, 0.100),
        anodized_black,
    );
    opaque.add_box(
        ota_frame * Mat4::from_translation(Vec3::new(0.0, -0.545, 0.062)),
        Vec3::new(0.068, 0.055, 0.030),
        anodized_black,
    );
    opaque.add_frustum(ota_frame, 0.047, 0.047, -0.505, -0.49, 20, accent_red);

    // Guide scope & guide camera
    let guide_frame = ota_frame * Mat4::from_translation(Vec3::new(0.0, 0.04, 0.155));
    opaque.add_box(
        ota_frame * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.118)),
        Vec3::new(0.036, 0.34, 0.016),
        dovetail_green,
    );
    opaque.add_frustum(guide_frame, 0.028, 0.030, -0.11, 0.15, 16, dark_metal);
    opaque.add_frustum(guide_frame, 0.026, 0.026, -0.17, -0.11, 16, accent_red);

    // 4. Local OTA Beam + At-Infinity Sky Reticle
    let ota_aperture = ota_frame.transform_point3(Vec3::new(0.0, 0.63, 0.0));
    let ray_color = if !state.is_connected {
        [0.65, 0.68, 0.75, 0.55]
    } else if state.is_slewing {
        [0.98, 0.72, 0.25, 0.95]
    } else if state.is_parked {
        [0.55, 0.62, 0.72, 0.65]
    } else {
        [0.24, 0.85, 0.78, 0.95]
    };
    lines.add_segment(
        ota_aperture,
        ota_aperture + ota_dir * 0.85,
        [ray_color[0], ray_color[1], ray_color[2], ray_color[3] * 0.5],
    );
    add_infinity_reticle(&mut sky_lines, ota_dir, ray_color, 0.028);

    if let (Some(tgt_ra), Some(tgt_dec)) = (state.target_ra_deg, state.target_dec_deg) {
        let tgt_ha = Angle::from_degrees(state.observer_lst_deg - tgt_ra)
            .signed_degrees()
            .to_radians() as f32;
        let tgt_d = (tgt_dec as f32).clamp(-90.0, 90.0).to_radians();
        let tgt_dir = eq.sky_dir(tgt_ha, tgt_d);
        add_infinity_reticle(&mut sky_lines, tgt_dir, [0.98, 0.68, 0.22, 0.85], 0.022);
    }

    // 5. At-Infinity Safety & Meridian Limit Widgets
    let sky_limits_alpha = if state.sky_limits_enabled && state.sky_limits_opacity.is_finite() {
        state.sky_limits_opacity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if sky_limits_alpha > 1e-3 {
        let limit_mesh_start = sky_translucent.vertices.len();
        let limit_line_start = sky_lines.vertices.len();

        if state.widgets.altitude_limits {
            build_altitude_limit_widgets(
                Vec3::ZERO,
                INFINITY_WIDGET_RADIUS,
                &state.limits,
                &mut sky_translucent,
                &mut sky_lines,
            );
        }
        if state.widgets.azimuth_sector && state.limits.az_avoid_enabled {
            build_azimuth_exclusion_widget(
                Vec3::ZERO,
                INFINITY_WIDGET_RADIUS,
                &state.limits,
                &mut sky_translucent,
                &mut sky_lines,
            );
        }
        if state.widgets.meridian_flip {
            build_meridian_flip_widget(
                Vec3::ZERO,
                INFINITY_WIDGET_RADIUS,
                eq,
                &state.limits,
                &mut sky_translucent,
                &mut sky_lines,
            );
        }

        if sky_limits_alpha < 0.999 {
            for v in &mut sky_translucent.vertices[limit_mesh_start..] {
                v.color[3] *= sky_limits_alpha;
            }
            for v in &mut sky_lines.vertices[limit_line_start..] {
                v.color[3] *= sky_limits_alpha;
            }
        }
    }

    SceneGeometry {
        sky_translucent,
        sky_lines,
        opaque,
        lines,
    }
}

fn add_infinity_reticle(lines: &mut LineBuilder, dir: Vec3, color: [f32; 4], angular_radius: f32) {
    let center = dir * (INFINITY_WIDGET_RADIUS * 0.98);
    let ref_up = if dir.y.abs() < 0.92 { Vec3::Y } else { Vec3::X };
    let u = dir.cross(ref_up).normalize_or_zero();
    let v = u.cross(dir).normalize_or_zero();
    let r = INFINITY_WIDGET_RADIUS * angular_radius;
    lines.add_circle(center, u, v, r, 32, color);
    let inner = r * 1.15;
    let outer = r * 1.85;
    lines.add_segment(center + u * inner, center + u * outer, color);
    lines.add_segment(center - u * inner, center - u * outer, color);
    lines.add_segment(center + v * inner, center + v * outer, color);
    lines.add_segment(center - v * inner, center - v * outer, color);
}

fn build_ground_compass(lines: &mut LineBuilder) {
    let grid_color = [0.22, 0.26, 0.34, 0.35];
    let ring_color = [0.28, 0.34, 0.44, 0.55];
    let north_color = [0.92, 0.36, 0.34, 0.90];
    let cardinal_color = [0.46, 0.56, 0.70, 0.75];

    let y = Vec3::new(0.0, 0.002, 0.0);
    for r in [0.75_f32, 1.25, COMPASS_RADIUS] {
        lines.add_circle(y, Vec3::X, Vec3::Z, r, 72, ring_color);
    }

    for deg in (0..360).step_by(30) {
        let rad = (deg as f32).to_radians();
        let dir = Vec3::new(rad.sin(), 0.0, -rad.cos());
        let color = if deg == 0 {
            north_color
        } else if deg % 90 == 0 {
            cardinal_color
        } else {
            grid_color
        };
        lines.add_segment(y + dir * 0.35, y + dir * COMPASS_RADIUS, color);
    }

    let tip = Vec3::new(0.0, 0.004, -(COMPASS_RADIUS + 0.16));
    let left = Vec3::new(-0.07, 0.004, -(COMPASS_RADIUS + 0.02));
    let right = Vec3::new(0.07, 0.004, -(COMPASS_RADIUS + 0.02));
    lines.add_segment(left, tip, north_color);
    lines.add_segment(right, tip, north_color);
    lines.add_segment(left, right, north_color);
}

fn build_altitude_limit_widgets(
    origin: Vec3,
    radius: f32,
    limits: &MountLimits,
    translucent: &mut MeshBuilder,
    lines: &mut LineBuilder,
) {
    let min_alt = (limits.min_altitude_deg as f32)
        .clamp(0.0, 80.0)
        .to_radians();
    let max_alt = (limits.max_altitude_deg as f32)
        .clamp(5.0, 90.0)
        .to_radians();

    if min_alt > 0.01 {
        translucent.add_spherical_band(
            origin,
            radius,
            0.0,
            min_alt,
            0.0,
            TAU,
            64,
            4,
            [0.92, 0.48, 0.22, 0.12],
        );
        let rim_y = origin.y + radius * min_alt.sin();
        let rim_r = radius * min_alt.cos();
        lines.add_circle(
            Vec3::new(origin.x, rim_y, origin.z),
            Vec3::X,
            Vec3::Z,
            rim_r,
            72,
            [0.96, 0.58, 0.28, 0.78],
        );
    }

    if max_alt < FRAC_PI_2 - 0.005 {
        translucent.add_spherical_band(
            origin,
            radius,
            max_alt,
            FRAC_PI_2,
            0.0,
            TAU,
            48,
            4,
            [0.90, 0.28, 0.34, 0.14],
        );
        let cap_y = origin.y + radius * max_alt.sin();
        let cap_r = radius * max_alt.cos();
        lines.add_circle(
            Vec3::new(origin.x, cap_y, origin.z),
            Vec3::X,
            Vec3::Z,
            cap_r,
            64,
            [0.95, 0.36, 0.40, 0.78],
        );
    }
}

fn build_azimuth_exclusion_widget(
    origin: Vec3,
    radius: f32,
    limits: &MountLimits,
    translucent: &mut MeshBuilder,
    lines: &mut LineBuilder,
) {
    let start = (limits.az_avoid_start_deg as f32).rem_euclid(360.0);
    let end = (limits.az_avoid_end_deg as f32).rem_euclid(360.0);
    let sweep_deg = (end - start).rem_euclid(360.0);
    if sweep_deg < 0.5 {
        return;
    }

    let az_start = start.to_radians();
    let az_sweep = sweep_deg.to_radians();
    let min_alt = (limits.min_altitude_deg as f32)
        .clamp(0.0, 80.0)
        .to_radians();
    let max_alt = (limits.max_altitude_deg as f32)
        .clamp(5.0, 90.0)
        .to_radians();

    translucent.add_spherical_band(
        origin,
        radius * 0.992,
        min_alt,
        max_alt,
        az_start,
        az_sweep,
        32,
        6,
        [0.94, 0.26, 0.28, 0.16],
    );

    let edge_color = [0.98, 0.34, 0.36, 0.78];
    for az in [az_start, az_start + az_sweep] {
        let mut pts = Vec::with_capacity(20);
        for i in 0..=18 {
            let t = i as f32 / 18.0;
            let alt = min_alt + (max_alt - min_alt) * t;
            pts.push(origin + horizontal_dir(az, alt) * (radius * 0.995));
        }
        lines.add_polyline(&pts, edge_color);
    }
}

fn build_meridian_flip_widget(
    origin: Vec3,
    radius: f32,
    eq: EquatorialFrame,
    limits: &MountLimits,
    translucent: &mut MeshBuilder,
    lines: &mut LineBuilder,
) {
    let mut meridian_pts = Vec::with_capacity(49);
    for i in 0..=48 {
        let alt = (i as f32 / 48.0) * PI;
        let dir = Vec3::new(0.0, alt.sin(), -alt.cos());
        meridian_pts.push(origin + dir * (radius * 1.005));
    }
    lines.add_polyline(&meridian_pts, [0.35, 0.72, 0.98, 0.78]);

    let flip_rad = (limits.meridian_limit_deg as f32)
        .clamp(0.5, 45.0)
        .to_radians();
    let fan_color = if limits.auto_meridian_flip {
        [0.30, 0.74, 0.98, 0.14]
    } else {
        [0.95, 0.68, 0.24, 0.14]
    };
    let edge_color = if limits.auto_meridian_flip {
        [0.42, 0.82, 1.0, 0.78]
    } else {
        [0.98, 0.74, 0.30, 0.78]
    };

    let dec_steps = 28u32;
    let ha_steps = 6u32;
    let r = radius * 0.985;
    let stride = dec_steps + 1;
    let base = translucent.vertices.len() as u32;

    for ih in 0..=ha_steps {
        let h = (ih as f32 / ha_steps as f32) * flip_rad;
        for id in 0..=dec_steps {
            let d = -FRAC_PI_2 + (id as f32 / dec_steps as f32) * PI;
            let p = eq.sky_dir(h, d);
            translucent.push_vertex(origin + p * r, p, fan_color);
        }
    }

    for ih in 0..ha_steps {
        for id in 0..dec_steps {
            let v00 = base + ih * stride + id;
            let v10 = base + (ih + 1) * stride + id;
            let v11 = v10 + 1;
            let v01 = v00 + 1;
            if translucent.vertices[v00 as usize].normal[1] < -0.05
                && translucent.vertices[v10 as usize].normal[1] < -0.05
                && translucent.vertices[v11 as usize].normal[1] < -0.05
                && translucent.vertices[v01 as usize].normal[1] < -0.05
            {
                continue;
            }
            translucent.push_quad(v00, v10, v11, v01);
        }
    }

    let mut limit_arc = Vec::new();
    for id in 0..=40 {
        let d = -FRAC_PI_2 + (id as f32 / 40.0) * PI;
        let p = eq.sky_dir(flip_rad, d);
        if p.y >= 0.0 {
            limit_arc.push(origin + p * (radius * 1.005));
        }
    }
    lines.add_polyline(&limit_arc, edge_color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::assert_outward_ccw_winding;

    #[test]
    fn all_opaque_mount_triangles_have_outward_ccw_winding_in_both_hemispheres() {
        for lat in [48.8566, -33.8688] {
            let state = MountSceneState {
                observer_lat_deg: lat,
                ..MountSceneState::default()
            };
            let geo = build_scene(&state);
            assert_outward_ccw_winding(&geo.opaque);
        }
    }

    #[test]
    fn elevated_pole_points_upward_in_southern_hemisphere() {
        let north = EquatorialFrame::from_latitude_deg(45.0);
        assert!(north.u_elevated_pole.y > 0.0);
        assert!(north.u_elevated_pole.z < 0.0); // North

        let south = EquatorialFrame::from_latitude_deg(-33.8);
        assert!(south.u_elevated_pole.y > 0.0);
        assert!(south.u_elevated_pole.z > 0.0); // South
    }
}
