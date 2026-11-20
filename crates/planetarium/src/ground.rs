use bevy::prelude::*;

/// Renderer-side ground hemisphere mask configuration controlled via [`crate::ViewportCommand`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct GroundSettings {
    pub opacity: f32,
}

impl Default for GroundSettings {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Computes subtle linear-RGB horizon and nadir ground colors adapted to solar altitude.
pub fn compute_ground_palette(sun_alt_deg: f32) -> (Vec3, Vec3) {
    let night_horizon = Vec3::new(0.004, 0.005, 0.009);
    let night_nadir = Vec3::new(0.0015, 0.002, 0.0035);

    let twilight_horizon = Vec3::new(0.014, 0.014, 0.022);
    let twilight_nadir = Vec3::new(0.004, 0.0045, 0.007);

    let day_horizon = Vec3::new(0.032, 0.044, 0.036);
    let day_nadir = Vec3::new(0.012, 0.018, 0.015);

    let tw_t = smoothstep(-16.0, -2.0, sun_alt_deg);
    let day_t = smoothstep(-2.0, 10.0, sun_alt_deg);

    let horizon = night_horizon
        .lerp(twilight_horizon, tw_t)
        .lerp(day_horizon, day_t);
    let nadir = night_nadir
        .lerp(twilight_nadir, tw_t)
        .lerp(day_nadir, day_t);
    (horizon, nadir)
}

#[cfg(test)]
mod tests {
    use celestial::coords::{Angle, topocentric_spherical_to_enu};

    use super::*;
    use crate::camera::{PlanetariumCamera, ViewMode};

    #[test]
    fn zenith_cam_dot_matches_topocentric_altitude_sign() {
        for mode in [ViewMode::Horizontal, ViewMode::Equatorial] {
            let camera = PlanetariumCamera {
                view_mode: mode,
                yaw: 0.9,
                pitch: 0.35,
                ..Default::default()
            };
            let topo_to_cam = camera.topocentric_to_camera_matrix();
            let cam_to_topo = topo_to_cam.transpose();
            let zenith_cam = topo_to_cam.z_axis.normalize();

            for alt_deg in [-60.0_f64, -5.0, 5.0, 45.0, 89.0] {
                let dir_enu = topocentric_spherical_to_enu(
                    Angle::from_degrees(123.0),
                    Angle::from_degrees(alt_deg),
                );
                let v_cam = topo_to_cam * dir_enu;
                let reconstructed_up = (cam_to_topo * v_cam).z;
                let dot_up = zenith_cam.dot(v_cam);

                assert!(
                    (dot_up - dir_enu.z).abs() < 1e-12,
                    "mode={mode:?}, alt={alt_deg}, dot_up={dot_up}, expected={}",
                    dir_enu.z
                );
                assert!((dot_up - reconstructed_up).abs() < 1e-12);
                if alt_deg > 0.0 {
                    assert!(dot_up > 0.0);
                } else {
                    assert!(dot_up < 0.0);
                }
            }
        }
    }

    #[test]
    fn ground_palette_smoothly_darkens_from_day_to_night() {
        let (day_h, day_n) = compute_ground_palette(45.0);
        let (tw_h, tw_n) = compute_ground_palette(-4.0);
        let (night_h, night_n) = compute_ground_palette(-20.0);

        assert!(day_h.length() > tw_h.length());
        assert!(tw_h.length() > night_h.length());
        assert!(day_n.length() > tw_n.length());
        assert!(tw_n.length() > night_n.length());
        assert!(day_h.length() > day_n.length());
        assert!(night_h.length() > night_n.length());
    }
}
