use serde::{Deserialize, Serialize};

use crate::coords::angle::AngleInterval;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SphericalRect {
    pub lon: AngleInterval,
    pub lat: AngleInterval,
}

impl SphericalRect {
    #[inline]
    pub fn new(lon: AngleInterval, lat: AngleInterval) -> Self {
        Self { lon, lat }
    }

    #[inline]
    pub fn from_degrees(lon_start: f64, lon_end: f64, lat_start: f64, lat_end: f64) -> Self {
        Self {
            lon: AngleInterval::from_degrees(lon_start, lon_end),
            lat: AngleInterval::from_degrees(lat_start, lat_end),
        }
    }

    #[inline]
    pub fn full_sphere() -> Self {
        Self {
            lon: AngleInterval::full_circle(),
            lat: AngleInterval::latitude_range(),
        }
    }
}
