use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use crate::coords::{
    angle::{Angle, AngleInterval},
    frames::{Frame, ICRS, J2000, Topocentric},
};

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

#[derive(Debug, Clone, Copy)]
pub struct EquatorialCoord<F: Frame> {
    pub ra: Angle,
    pub dec: Angle,
    _frame: PhantomData<F>,
}

impl EquatorialCoord<J2000> {
    fn new(ra: Angle, dec: Angle) -> Self {
        Self {
            ra,
            dec,
            _frame: PhantomData {},
        }
    }
    pub fn from_degrees(ra: f64, dec: f64) -> Self {
        Self::new(Angle::from_degrees(ra), Angle::from_degrees(dec))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SphericalBounds<F: Frame> {
    pub phi: AngleInterval,
    pub psi: AngleInterval,
    _frame: PhantomData<F>,
}

impl<F: Frame> SphericalBounds<F> {
    #[inline]
    pub fn new(phi: AngleInterval, psi: AngleInterval) -> Self {
        Self {
            phi,
            psi,
            _frame: PhantomData,
        }
    }

    #[inline]
    pub fn to_rect(&self) -> SphericalRect {
        SphericalRect {
            lon: self.phi,
            lat: self.psi,
        }
    }
}

impl<F: Frame> From<SphericalBounds<F>> for SphericalRect {
    #[inline]
    fn from(b: SphericalBounds<F>) -> Self {
        b.to_rect()
    }
}

pub type EquatorialBounds = SphericalBounds<ICRS>;
pub type HorizontalBounds = SphericalBounds<Topocentric>;
