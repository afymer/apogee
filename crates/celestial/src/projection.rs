use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;

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
}
