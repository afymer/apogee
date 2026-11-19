use std::ops::{Add, Div, Mul, Neg, Sub};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Angle(f64);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AngleInterval {
    start: Angle,
    end: Angle,
}

impl Angle {
    pub const ZERO: Self = Self(0.);
    pub const PI: Self = Self(std::f64::consts::PI);
    pub const HALF_PI: Self = Self(std::f64::consts::FRAC_PI_2);
    pub const TAU: Self = Self(std::f64::consts::TAU);

    #[inline]
    pub fn from_radians(radians: f64) -> Self {
        Self(radians)
    }

    #[inline]
    pub fn from_degrees(degrees: f64) -> Self {
        Self(degrees.to_radians())
    }

    #[inline]
    pub fn from_hours(hours: f64) -> Self {
        Self::from_degrees(hours * 15.)
    }

    #[inline]
    pub fn from_radians_normalized(radians: f64) -> Self {
        Self(radians).normalize()
    }

    #[inline]
    pub fn from_degrees_normalized(degrees: f64) -> Self {
        Self::from_degrees(degrees).normalize()
    }

    #[inline]
    pub fn radians(&self) -> f64 {
        self.0
    }

    #[inline]
    pub fn degrees(&self) -> f64 {
        self.0.to_degrees()
    }

    #[inline]
    pub fn hours(&self) -> f64 {
        self.0.to_degrees() / 15.
    }

    #[inline]
    pub fn signed_radians(&self) -> f64 {
        let norm = self.0.rem_euclid(Self::TAU.0);
        if norm > Self::PI.0 {
            norm - Self::TAU.0
        } else {
            norm
        }
    }

    #[inline]
    pub fn signed_degrees(&self) -> f64 {
        self.signed_radians().to_degrees()
    }

    #[inline]
    #[must_use]
    pub fn normalize(self) -> Self {
        let r = self.0.rem_euclid(Self::TAU.0);
        Self(if r >= Self::TAU.0 { 0.0 } else { r })
    }

    #[inline]
    #[must_use]
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    #[inline]
    pub fn sin_cos(&self) -> (f64, f64) {
        self.0.sin_cos()
    }

    #[inline]
    pub fn sin(&self) -> f64 {
        self.0.sin()
    }

    #[inline]
    pub fn cos(&self) -> f64 {
        self.0.cos()
    }

    #[inline]
    pub fn tan(&self) -> f64 {
        self.0.tan()
    }
}

impl Add for Angle {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Angle {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Mul<f64> for Angle {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self(self.0 * rhs)
    }
}

impl Mul<Angle> for f64 {
    type Output = Angle;
    #[inline]
    fn mul(self, rhs: Angle) -> Angle {
        Angle(self * rhs.0)
    }
}

impl Div<f64> for Angle {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f64) -> Self {
        Self(self.0 / rhs)
    }
}

impl Div<Angle> for Angle {
    type Output = f64;
    #[inline]
    fn div(self, rhs: Angle) -> f64 {
        self.0 / rhs.0
    }
}

impl Neg for Angle {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl AngleInterval {
    #[inline]
    pub fn new(start: Angle, end: Angle) -> Self {
        Self { start, end }
    }

    #[inline]
    pub fn from_degrees(start_deg: f64, end_deg: f64) -> Self {
        Self::new(Angle::from_degrees(start_deg), Angle::from_degrees(end_deg))
    }

    #[inline]
    pub fn from_radians(start_rad: f64, end_rad: f64) -> Self {
        Self::new(Angle::from_radians(start_rad), Angle::from_radians(end_rad))
    }

    #[inline]
    pub fn full_circle() -> Self {
        Self::new(Angle::ZERO, Angle::TAU)
    }

    #[inline]
    pub fn latitude_range() -> Self {
        Self::new(-Angle::HALF_PI, Angle::HALF_PI)
    }

    #[inline]
    pub fn start(&self) -> Angle {
        self.start
    }

    #[inline]
    pub fn end(&self) -> Angle {
        self.end
    }

    #[inline]
    pub fn is_wrapping(&self) -> bool {
        self.start.radians() > self.end.radians()
    }

    #[inline]
    pub fn span(&self) -> Angle {
        let s = self.start.radians();
        let e = self.end.radians();
        if e >= s {
            Angle::from_radians(e - s)
        } else {
            Angle::from_radians((e - s).rem_euclid(Angle::TAU.0))
        }
    }

    pub fn contains(&self, angle: Angle) -> bool {
        let s = self.start.radians();
        let e = self.end.radians();
        if self.is_wrapping() {
            let a_norm = angle.normalize().radians();
            let s_norm = self.start.normalize().radians();
            let e_norm = self.end.normalize().radians();
            if s_norm <= e_norm {
                a_norm >= s_norm && a_norm <= e_norm
            } else {
                a_norm >= s_norm || a_norm <= e_norm
            }
        } else if s >= 0.0 && e <= Angle::TAU.0 {
            if e - s >= Angle::TAU.0 - 1e-12 {
                return true;
            }
            let a_norm = angle.normalize().radians();
            a_norm >= s && a_norm <= e
        } else {
            let a = angle.radians();
            a >= s && a <= e
        }
    }

    pub fn quantized_steps(&self, step: Angle) -> Vec<Angle> {
        let step_rad = step.radians().abs();
        if step_rad < 1e-9 {
            return Vec::new();
        }

        let mut results = Vec::new();
        let s = self.start.radians();
        let e = self.end.radians();

        if self.is_wrapping() {
            let tau = Angle::TAU.radians();

            let k_min_1 = (s / step_rad - 1e-9).ceil() as i64;
            let k_max_1 = ((tau - 1e-9) / step_rad).floor() as i64;
            for k in k_min_1..=k_max_1 {
                results.push(Angle::from_radians(k as f64 * step_rad));
            }

            let k_max_2 = (e / step_rad + 1e-9).floor() as i64;
            for k in 0..=k_max_2 {
                results.push(Angle::from_radians(k as f64 * step_rad));
            }
        } else {
            let k_min = (s / step_rad - 1e-9).ceil() as i64;
            let k_max = (e / step_rad + 1e-9).floor() as i64;
            for k in k_min..=k_max {
                results.push(Angle::from_radians(k as f64 * step_rad));
            }
        }

        results
    }

    pub fn subdivide(&self, max_step: Angle) -> Vec<Angle> {
        let span_rad = self.span().radians();
        let max_rad = max_step.radians().abs().max(1e-6);
        let num_segments = (span_rad / max_rad).ceil().max(1.0) as usize;

        let mut points = Vec::with_capacity(num_segments + 1);
        let s = self.start.radians();
        let is_wrap = self.is_wrapping();
        let tau = Angle::TAU.radians();

        for i in 0..=num_segments {
            let t = (i as f64) / (num_segments as f64);
            let mut val = s + t * span_rad;
            if is_wrap && val >= tau {
                val -= tau;
            }
            points.push(Angle::from_radians(val));
        }

        points
    }
}

#[inline]
pub fn is_multiple_of(val: Angle, step: Angle) -> bool {
    let s = step.radians().abs();
    if s < 1e-9 {
        return true;
    }
    let v = val.radians();
    let rem = (v / s).round() * s - v;
    rem.abs() < 1e-5
}

#[inline]
pub fn canonical_spherical_to_cartesian(lon: Angle, lat: Angle) -> glam::DVec3 {
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_lon, cos_lon) = lon.sin_cos();
    glam::DVec3::new(cos_lat * cos_lon, cos_lat * sin_lon, sin_lat)
}

#[inline]
pub fn cartesian_to_canonical_spherical(v: glam::DVec3) -> (Angle, Angle) {
    let norm = v.normalize_or_zero();
    if norm == glam::DVec3::ZERO {
        return (Angle::ZERO, Angle::ZERO);
    }
    let lat = Angle::from_radians(norm.z.clamp(-1.0, 1.0).asin());
    let lon = Angle::from_radians_normalized(norm.y.atan2(norm.x));
    (lon, lat)
}

#[inline]
pub fn topocentric_spherical_to_enu(az: Angle, alt: Angle) -> glam::DVec3 {
    let (sin_alt, cos_alt) = alt.sin_cos();
    let (sin_az, cos_az) = az.sin_cos();
    glam::DVec3::new(cos_alt * sin_az, cos_alt * cos_az, sin_alt)
}

#[inline]
pub fn topocentric_enu_to_spherical(v: glam::DVec3) -> (Angle, Angle) {
    let norm = v.normalize_or_zero();
    if norm == glam::DVec3::ZERO {
        return (Angle::ZERO, Angle::ZERO);
    }
    let alt = Angle::from_radians(norm.z.clamp(-1.0, 1.0).asin());
    let az = Angle::from_radians_normalized(norm.x.atan2(norm.y));
    (az, alt)
}
