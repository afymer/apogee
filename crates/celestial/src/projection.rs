use std::f64::consts::FRAC_PI_2;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::coords::angle::Angle;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ProjectionKind {
    #[default]
    Stereographic,
    Perspective,
    /// Blend between Stereographic (0.0) and Perspective (1.0)
    Blended(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraProjection {
    pub fov_y: Angle,
    pub aspect_ratio: f64,
    pub kind: ProjectionKind,
}

impl Default for CameraProjection {
    fn default() -> Self {
        Self {
            fov_y: Angle::from_degrees(180.0),
            aspect_ratio: 1.0,
            kind: ProjectionKind::Stereographic,
        }
    }
}

impl CameraProjection {
    pub fn new(fov_y: Angle, aspect_ratio: f64, kind: ProjectionKind) -> Self {
        Self {
            fov_y,
            aspect_ratio: aspect_ratio.max(0.001),
            kind,
        }
    }

    /// Blend between Stereographic (0.0) and Perspective (1.0)
    #[inline]
    pub fn blend_factor(&self) -> f64 {
        match self.kind {
            ProjectionKind::Stereographic => 0.0,
            ProjectionKind::Perspective => 1.0,
            ProjectionKind::Blended(b) => b.clamp(0.0, 1.0),
        }
    }

    /// Radial projection distance for an angle theta from the optical axis in radians
    #[inline]
    pub fn radial_distance(&self, theta: f64) -> f64 {
        let b = self.blend_factor();
        let r_stereo = 2.0 * (theta * 0.5).tan();
        if b <= 0.0 {
            r_stereo
        } else {
            let r_persp = if theta < FRAC_PI_2 { theta.tan() } else { 1e6 };
            (1.0 - b) * r_stereo + b * r_persp
        }
    }

    /// Maps fov_y / 2 to screen radius 1.0 vertically
    #[inline]
    pub fn scale_factor(&self) -> f64 {
        self.radial_distance(self.fov_y.radians() * 0.5)
    }

    /// Reconstructs a unit camera-space 3D direction vector from screen space NDC (`[-1, 1]^2`).
    ///
    /// Exact inverse of the GPU celestial projection (`+Z` forward, `+X` right, `+Y` up) across
    /// stereographic, blended, and perspective projections.
    pub fn unproject_ndc(&self, ndc_x: f64, ndc_y: f64) -> DVec3 {
        let scale = self.scale_factor().max(1e-12);
        let px = ndc_x * self.aspect_ratio * scale;
        let py = ndc_y * scale;
        let r2 = px * px + py * py;
        if r2 <= 1e-24 {
            return DVec3::Z;
        }

        let b = self.blend_factor().clamp(0.0, 1.0);
        if b <= 1e-9 {
            let t2 = 0.25 * r2;
            let inv = 1.0 / (1.0 + t2);
            return DVec3::new(px * inv, py * inv, (1.0 - t2) * inv);
        }
        if b >= 1.0 - 1e-9 {
            return DVec3::new(px, py, 1.0).normalize();
        }

        let r = r2.sqrt();
        let lo_guess = r / (1.0 + (1.0 + r2).sqrt());
        let hi_guess = (0.5 * r).min(0.9999);
        let mut t = hi_guess + (lo_guess - hi_guess) * b;
        for _ in 0..5 {
            let t2 = t * t;
            let denom = (1.0 - t2).max(1e-12);
            let f = 2.0 * (1.0 - b) * t + (2.0 * b * t) / denom - r;
            let df = 2.0 * (1.0 - b) + (2.0 * b * (1.0 + t2)) / (denom * denom);
            t = (t - f / df.max(1e-12)).clamp(0.0, 0.999_999);
        }

        let t2 = t * t;
        let inv = 1.0 / (1.0 + t2);
        let scale_xy = (2.0 * t * inv) / r;
        DVec3::new(px * scale_xy, py * scale_xy, (1.0 - t2) * inv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproject_ndc_round_trips_across_projections_and_corners() {
        let kinds_and_fovs = [
            (180.0_f64, ProjectionKind::Stereographic),
            (90.0, ProjectionKind::Stereographic),
            (40.0, ProjectionKind::Blended(0.5)),
            (10.0, ProjectionKind::Perspective),
            (1.0, ProjectionKind::Perspective),
        ];
        let aspects = [1.0_f64, 16.0 / 9.0, 2.5];
        let ndc_samples = [
            (0.0, 0.0),
            (0.5, 0.25),
            (-0.8, 0.6),
            (1.0, 1.0),
            (-1.0, -1.0),
            (0.0, -1.0),
        ];

        for (fov_deg, kind) in kinds_and_fovs {
            for aspect in aspects {
                let proj = CameraProjection::new(Angle::from_degrees(fov_deg), aspect, kind);
                let scale = proj.scale_factor();

                for (ndc_x, ndc_y) in ndc_samples {
                    let v = proj.unproject_ndc(ndc_x, ndc_y);
                    assert!(
                        (v.length() - 1.0).abs() < 1e-12,
                        "unit length failed for fov={fov_deg}, aspect={aspect}, ndc=({ndc_x}, {ndc_y})"
                    );

                    let theta = v.z.clamp(-1.0, 1.0).acos();
                    let radial_xy = (v.x * v.x + v.y * v.y).sqrt();
                    let r_projected = proj.radial_distance(theta);
                    let (re_x, re_y) = if radial_xy > 1e-12 {
                        (
                            (v.x / radial_xy) * r_projected / (aspect * scale),
                            (v.y / radial_xy) * r_projected / scale,
                        )
                    } else {
                        (0.0, 0.0)
                    };
                    assert!(
                        (re_x - ndc_x).abs() < 1e-10 && (re_y - ndc_y).abs() < 1e-10,
                        "round-trip failed for fov={fov_deg}, aspect={aspect}, ndc=({ndc_x}, {ndc_y}): got ({re_x}, {re_y})"
                    );
                }
            }
        }
    }
}
