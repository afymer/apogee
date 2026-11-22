use bevy::prelude::*;

/// Renderer-side atmosphere configuration controlled via [`crate::ViewportCommand`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct AtmosphereSettings {
    pub strength: f32,
}

impl Default for AtmosphereSettings {
    fn default() -> Self {
        Self { strength: 1.0 }
    }
}

/// CPU-precomputed linear-RGB sky palette for a given solar altitude.
#[derive(Clone, Copy, Debug)]
pub struct SkyPalette {
    pub zenith_color: Vec3,
    pub horizon_color: Vec3,
    pub dusk_color: Vec3,
    pub sun_glow_color: Vec3,
    pub sun_glow_weight: f32,
}

impl SkyPalette {
    pub fn max_intensity(&self) -> f32 {
        self.zenith_color
            .max_element()
            .max(self.horizon_color.max_element())
            .max(self.dusk_color.max_element())
            .max((self.sun_glow_color * self.sun_glow_weight).max_element())
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Computes linear-RGB sky dome, dusk arch, and solar forward-scatter colors from solar altitude.
pub fn compute_sky_palette(sun_alt_deg: f32) -> SkyPalette {
    let day_env = smoothstep(-4.5, 7.0, sun_alt_deg);
    let sun_mu = sun_alt_deg.to_radians().sin().max(0.0);

    // High-altitude zenith optical depth stays clearer than the low-altitude horizon beam so
    // the midday zenith remains a vivid Rayleigh sky blue.
    let zenith_airmass = 1.0 / (sun_mu + 0.18);
    let zenith_k = (zenith_airmass - 0.85).max(0.0) * 0.22;
    let zenith_trans = Vec3::new(
        (-0.058 * zenith_k).exp(),
        (-0.135 * zenith_k).exp(),
        (-0.331 * zenith_k).exp(),
    );

    let sun_airmass = 1.0 / (sun_mu + 0.06);
    let optical_k = sun_airmass * 0.30;
    let horizon_trans = Vec3::new(
        (-0.058 * optical_k).exp(),
        (-0.135 * optical_k).exp(),
        (-0.331 * optical_k).exp(),
    );

    let day_zenith = Vec3::new(0.20, 0.56, 1.48) * zenith_trans * day_env;
    let day_horizon = Vec3::new(1.10, 1.78, 2.85) * horizon_trans * day_env;

    let sun_glow_weight = smoothstep(-3.5, 6.0, sun_alt_deg);
    let sun_glow_color =
        Vec3::new(1.0, 0.96, 0.90) * horizon_trans * (0.55 + 0.45 * smoothstep(-1.0, 6.0, sun_alt_deg));

    let twilight_rise = smoothstep(-18.0, -1.5, sun_alt_deg);
    let twilight_fade_day = 1.0 - smoothstep(0.0, 10.0, sun_alt_deg);
    let dome_weight = twilight_rise.powf(2.3) * twilight_fade_day;

    let twilight_zenith = Vec3::new(0.022, 0.068, 0.24) * dome_weight;
    let twilight_horizon = Vec3::new(0.08, 0.15, 0.36) * dome_weight;

    let warm_weight = smoothstep(-15.0, -2.5, sun_alt_deg).powf(1.8)
        * (1.0 - smoothstep(2.0, 12.0, sun_alt_deg));
    let dusk_color = Vec3::new(1.15, 0.30, 0.07) * warm_weight;

    SkyPalette {
        zenith_color: day_zenith + twilight_zenith,
        horizon_color: day_horizon + twilight_horizon,
        dusk_color,
        sun_glow_color,
        sun_glow_weight,
    }
}

/// Computes the effective stellar limiting magnitude given solar altitude, camera vertical FOV,
/// atmosphere strength (`[0.0, 1.0]`), and dark-sky baseline limiting magnitude.
pub fn compute_sky_limiting_magnitude(
    sun_alt_deg: f32,
    fov_y_rad: f32,
    atmosphere_strength: f32,
    base_limiting_mag: f32,
) -> f32 {
    let atmo_lim = if sun_alt_deg <= -18.0 {
        base_limiting_mag
    } else if sun_alt_deg <= -12.0 {
        let t = smoothstep(-18.0, -12.0, sun_alt_deg);
        base_limiting_mag + (6.0 - base_limiting_mag) * t
    } else if sun_alt_deg <= -6.0 {
        let t = smoothstep(-12.0, -6.0, sun_alt_deg);
        6.0 + (3.2 - 6.0) * t
    } else if sun_alt_deg <= 0.0 {
        let t = smoothstep(-6.0, 0.0, sun_alt_deg);
        3.2 + (-2.4 - 3.2) * t
    } else {
        let t = smoothstep(0.0, 5.0, sun_alt_deg);
        -2.4 + (-4.5 - (-2.4)) * t
    };

    // Modest telescopic zoom gain when zoomed in, suppressed in daylight so plain daytime
    // never reveals catalog stars.
    let fov = fov_y_rad.max(1e-4);
    let zoom_t = 1.0 - smoothstep(0.02, 1.047, fov);
    let zoom_day_suppress = 1.0 - smoothstep(-1.5, 1.5, sun_alt_deg);
    let zoom_gain = 1.8 * zoom_t * zoom_day_suppress;

    let effective_atmo_lim = atmo_lim + zoom_gain;
    let s = if atmosphere_strength.is_finite() {
        atmosphere_strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    base_limiting_mag + (effective_atmo_lim - base_limiting_mag) * s.powf(1.35)
}

/// Computes deep-sky (HiPS survey) visibility multiplier `[0.0, 1.0]` adapted to sky brightness.
pub fn compute_hips_atmosphere_visibility(sun_alt_deg: f32, atmosphere_strength: f32) -> f32 {
    let night_vis = 1.0 - smoothstep(-16.0, -7.0, sun_alt_deg);
    let s = if atmosphere_strength.is_finite() {
        atmosphere_strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    1.0 - s * (1.0 - night_vis)
}

#[cfg(test)]
mod tests {
    use glam::{DVec3, Vec2, Vec3};

    use super::*;
    use crate::camera::{PlanetariumCamera, ProjectionParams, ViewMode};

    fn project_celestial_cpu(v_cam: Vec3, proj: &ProjectionParams) -> Vec2 {
        let norm = v_cam.normalize();
        let nz = norm.z.clamp(-1.0, 1.0);
        let stereo_factor = 2.0 / (1.0 + nz).max(0.002);
        let persp_factor = 1.0 / nz.max(0.005);
        let factor = stereo_factor + (persp_factor - stereo_factor) * proj.blend_factor;
        let scale = proj.scale_factor.max(1e-9);
        Vec2::new(
            (norm.x * factor) / (proj.aspect_ratio * scale),
            (norm.y * factor) / scale,
        )
    }

    fn unproject_celestial_cpu(ndc: Vec2, proj: &ProjectionParams) -> Vec3 {
        let scale = proj.scale_factor.max(1e-9);
        let plane_pt = Vec2::new(ndc.x * proj.aspect_ratio * scale, ndc.y * scale);
        let r2 = plane_pt.length_squared();
        if r2 <= 1e-12 {
            return Vec3::Z;
        }

        let blend = proj.blend_factor.clamp(0.0, 1.0);
        if blend <= 1e-5 {
            let t2 = 0.25 * r2;
            let inv = 1.0 / (1.0 + t2);
            return Vec3::new(plane_pt.x * inv, plane_pt.y * inv, (1.0 - t2) * inv);
        }
        if blend >= 1.0 - 1e-5 {
            return Vec3::new(plane_pt.x, plane_pt.y, 1.0).normalize();
        }

        let radius = r2.sqrt();
        let lo_guess = radius / (1.0 + (1.0 + r2).sqrt());
        let hi_guess = (0.5 * radius).min(0.999);
        let mut half_tan = hi_guess + (lo_guess - hi_guess) * blend;
        for _ in 0..3 {
            let t2 = half_tan * half_tan;
            let denom = (1.0 - t2).max(1e-4);
            let err =
                2.0 * (1.0 - blend) * half_tan + (2.0 * blend * half_tan) / denom - radius;
            let deriv = 2.0 * (1.0 - blend) + (2.0 * blend * (1.0 + t2)) / (denom * denom);
            half_tan = (half_tan - err / deriv.max(1e-6)).clamp(0.0, 0.9999);
        }

        let t2 = half_tan * half_tan;
        let inv = 1.0 / (1.0 + t2);
        let xy = (plane_pt / radius) * (2.0 * half_tan * inv);
        Vec3::new(xy.x, xy.y, (1.0 - t2) * inv)
    }

    #[test]
    fn unproject_round_trips_across_projections_and_corners() {
        let fovs_deg = [180.0_f64, 90.0, 40.0, 10.0, 1.0];
        let aspects = [1.0_f32, 16.0 / 9.0, 2.5];
        let ndc_samples = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.5, 0.25),
            Vec2::new(-0.8, 0.6),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, -1.0),
            Vec2::new(0.0, -1.0),
        ];

        for fov_deg in fovs_deg {
            for aspect in aspects {
                let camera = PlanetariumCamera {
                    fov_y: fov_deg.to_radians(),
                    ..Default::default()
                };
                let proj = camera.projection_params(aspect);
                let cp = camera.projection(aspect);

                for ndc in ndc_samples {
                    let v_cam = unproject_celestial_cpu(ndc, &proj);
                    assert!(
                        (v_cam.length() - 1.0).abs() < 1e-5,
                        "unit length failed for fov={fov_deg}, aspect={aspect}, ndc={ndc:?}"
                    );

                    let reprojected = project_celestial_cpu(v_cam, &proj);
                    assert!(
                        (reprojected - ndc).length() < 1e-4,
                        "round-trip failed for fov={fov_deg}, aspect={aspect}, ndc={ndc:?}: got {reprojected:?}"
                    );

                    // Verify agreement with CameraProjection::radial_distance for center-to-top edge
                    if (ndc.x).abs() < 1e-6 && ndc.y > 0.0 {
                        let theta = f64::from(v_cam.z.clamp(-1.0, 1.0).acos());
                        let r_expected = cp.radial_distance(theta) / cp.scale_factor();
                        assert!(
                            (r_expected - f64::from(ndc.y)).abs() < 1e-4,
                            "radial_distance mismatch at fov={fov_deg}: {r_expected} vs {}",
                            ndc.y
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn camera_to_topo_maps_z_to_forward_direction_topo() {
        for mode in [ViewMode::Horizontal, ViewMode::Equatorial] {
            let camera = PlanetariumCamera {
                view_mode: mode,
                yaw: 1.35,
                pitch: 0.42,
                ..Default::default()
            };
            let cam_to_topo = camera.topocentric_to_camera_matrix().transpose();
            let mapped = cam_to_topo * DVec3::Z;
            let expected = camera.forward_direction_topo();
            assert!(
                mapped.distance(expected) < 1e-12,
                "mode={mode:?}, mapped={mapped:?}, expected={expected:?}"
            );
        }
    }

    #[test]
    fn sky_palette_day_dusk_and_night_disabled() {
        let noon = compute_sky_palette(45.0);
        assert!(noon.zenith_color.z > noon.zenith_color.x);
        assert!(noon.horizon_color.length() > noon.zenith_color.length());
        assert!(noon.dusk_color.length() < 1e-4);
        assert!(noon.sun_glow_weight > 0.99);

        let dusk = compute_sky_palette(-2.0);
        assert!(dusk.dusk_color.x > dusk.dusk_color.z);
        assert!(dusk.dusk_color.x > 0.5);
        assert!(dusk.horizon_color.z > dusk.horizon_color.x);

        let night = compute_sky_palette(-20.0);
        assert_eq!(night.max_intensity(), 0.0);
    }

    #[test]
    fn sky_limiting_magnitude_scales_from_night_through_twilight_to_day() {
        let base = 10.5_f32;
        let wide_fov = std::f32::consts::PI;

        let night = compute_sky_limiting_magnitude(-20.0, wide_fov, 1.0, base);
        let astro_tw = compute_sky_limiting_magnitude(-15.0, wide_fov, 1.0, base);
        let naut_tw = compute_sky_limiting_magnitude(-9.0, wide_fov, 1.0, base);
        let civil_tw = compute_sky_limiting_magnitude(-3.0, wide_fov, 1.0, base);
        let day_low = compute_sky_limiting_magnitude(10.0, wide_fov, 1.0, base);
        let day_high = compute_sky_limiting_magnitude(45.0, wide_fov, 1.0, base);

        assert!((night - base).abs() < 1e-5);
        assert!(astro_tw < night && astro_tw > 7.0);
        assert!(naut_tw < astro_tw && naut_tw > 3.5 && naut_tw < 6.0);
        assert!(civil_tw < naut_tw && civil_tw > -1.0 && civil_tw < 3.0);
        assert!(day_low < -2.0);
        assert!(day_high < -2.0);

        // Even zoomed in during plain daytime (>= 1.5°), limiting magnitude stays brighter than Sirius (-1.46).
        let day_zoomed = compute_sky_limiting_magnitude(1.5, 0.01, 1.0, base);
        assert!(day_zoomed <= -2.4, "expected <= -2.4, got {day_zoomed}");

        // Disabling atmosphere restores full dark-sky limiting magnitude even at noon.
        let atmo_off = compute_sky_limiting_magnitude(45.0, wide_fov, 0.0, base);
        assert!((atmo_off - base).abs() < 1e-5);
    }

    #[test]
    fn hips_visibility_fades_out_before_civil_twilight_and_daytime() {
        assert!((compute_hips_atmosphere_visibility(-20.0, 1.0) - 1.0).abs() < 1e-5);
        let mid_tw = compute_hips_atmosphere_visibility(-11.5, 1.0);
        assert!(mid_tw > 0.1 && mid_tw < 0.9);
        assert_eq!(compute_hips_atmosphere_visibility(-3.0, 1.0), 0.0);
        assert_eq!(compute_hips_atmosphere_visibility(30.0, 1.0), 0.0);
        assert!((compute_hips_atmosphere_visibility(30.0, 0.0) - 1.0).abs() < 1e-5);
    }
}
