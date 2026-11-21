use serde::{Deserialize, Serialize};

/// Default azimuthal sample resolution for horizon profiles (0.5° per bin).
pub const DEFAULT_HORIZON_SAMPLES: usize = 720;

/// Minimum allowed horizon altitude in degrees.
pub const MIN_HORIZON_ALT_DEG: f32 = -89.5;

/// Maximum allowed horizon altitude in degrees.
pub const MAX_HORIZON_ALT_DEG: f32 = 89.5;

#[inline]
fn sanitize_sample(alt_deg: f32) -> f32 {
    if alt_deg.is_finite() {
        alt_deg.clamp(MIN_HORIZON_ALT_DEG, MAX_HORIZON_ALT_DEG)
    } else {
        0.0
    }
}

#[inline]
fn normalize_azimuth_offset(offset_deg: f64) -> f64 {
    if offset_deg.is_finite() {
        let r = offset_deg.rem_euclid(360.0);
        if r >= 360.0 { 0.0 } else { r }
    } else {
        0.0
    }
}

/// 360° topocentric horizon altitude profile sampled uniformly across azimuth.
///
/// Sample `i` corresponds to image-space azimuth `(i / len) * 360°`, advancing clockwise
/// (`0°` North -> `90°` East -> `180°` South -> `270°` West -> `360°` North).
/// True compass azimuth `az_deg` maps to image-space azimuth
/// `(az_deg - azimuth_offset_deg).rem_euclid(360.0)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonProfile {
    pub samples_deg: Vec<f32>,
    #[serde(default)]
    pub azimuth_offset_deg: f64,
}

impl Default for HorizonProfile {
    fn default() -> Self {
        Self::flat(0.0, DEFAULT_HORIZON_SAMPLES)
    }
}

impl HorizonProfile {
    /// Creates a flat horizon at `altitude_deg` across `sample_count` bins.
    pub fn flat(altitude_deg: f32, sample_count: usize) -> Self {
        let count = sample_count.max(1);
        let alt = sanitize_sample(altitude_deg);
        Self {
            samples_deg: vec![alt; count],
            azimuth_offset_deg: 0.0,
        }
    }

    /// Creates a sanitized profile from raw altitude samples and azimuth offset.
    pub fn from_samples(samples_deg: Vec<f32>, azimuth_offset_deg: f64) -> Self {
        let mut profile = Self {
            samples_deg,
            azimuth_offset_deg,
        };
        profile.sanitize();
        profile
    }

    /// Returns `true` when there are no altitude samples.
    pub fn is_empty(&self) -> bool {
        self.samples_deg.is_empty()
    }

    /// Returns the number of azimuthal bins.
    pub fn sample_count(&self) -> usize {
        self.samples_deg.len()
    }

    /// Clamps finite samples into `[MIN_HORIZON_ALT_DEG, MAX_HORIZON_ALT_DEG]`,
    /// replaces non-finite samples with `0.0`, and normalizes `azimuth_offset_deg` into `[0, 360)`.
    pub fn sanitize(&mut self) {
        if self.samples_deg.is_empty() {
            self.samples_deg = vec![0.0; DEFAULT_HORIZON_SAMPLES];
        } else {
            for sample in &mut self.samples_deg {
                *sample = sanitize_sample(*sample);
            }
        }
        self.azimuth_offset_deg = normalize_azimuth_offset(self.azimuth_offset_deg);
    }

    /// Returns a sanitized clone of this profile.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let mut cloned = self.clone();
        cloned.sanitize();
        cloned
    }

    /// Evaluates the horizon altitude (degrees) at true compass azimuth `az_deg`
    /// using circular linear interpolation.
    pub fn altitude_at_azimuth_deg(&self, az_deg: f64) -> f64 {
        if self.samples_deg.is_empty() || !az_deg.is_finite() {
            return 0.0;
        }
        let n = self.samples_deg.len();
        let offset = normalize_azimuth_offset(self.azimuth_offset_deg);
        let img_az = (az_deg - offset).rem_euclid(360.0);
        let pos = (img_az / 360.0) * n as f64;
        let floor_pos = pos.floor();
        let i0 = (floor_pos as usize) % n;
        let i1 = (i0 + 1) % n;
        let frac = pos - floor_pos;
        let a0 = f64::from(sanitize_sample(self.samples_deg[i0]));
        let a1 = f64::from(sanitize_sample(self.samples_deg[i1]));
        a0 + (a1 - a0) * frac
    }

    /// Returns `true` when `alt_deg` is at or above the horizon at `az_deg`.
    pub fn is_visible(&self, az_deg: f64, alt_deg: f64) -> bool {
        if !alt_deg.is_finite() {
            return false;
        }
        alt_deg >= self.altitude_at_azimuth_deg(az_deg)
    }

    /// Returns signed altitude clearance above the horizon (`alt_deg - horizon_alt(az_deg)`).
    pub fn clearance_deg(&self, az_deg: f64, alt_deg: f64) -> f64 {
        if !alt_deg.is_finite() {
            return -90.0;
        }
        alt_deg - self.altitude_at_azimuth_deg(az_deg)
    }

    /// Returns `(min_altitude_deg, max_altitude_deg)` across all samples.
    pub fn min_max_altitude_deg(&self) -> (f32, f32) {
        if self.samples_deg.is_empty() {
            return (0.0, 0.0);
        }
        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        for &s in &self.samples_deg {
            let v = sanitize_sample(s);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        (min_v, max_v)
    }

    /// Returns all altitude samples converted to radians for GPU upload.
    pub fn to_radians_vec(&self) -> Vec<f32> {
        self.samples_deg
            .iter()
            .map(|&deg| sanitize_sample(deg).to_radians())
            .collect()
    }

    /// Linearly paints a horizon segment between two world compass azimuths along the shortest
    /// circular arc, handling wrap-around across `0° / 360°`.
    pub fn set_segment_world_az(
        &mut self,
        az0_deg: f64,
        alt0_deg: f32,
        az1_deg: f64,
        alt1_deg: f32,
    ) {
        if self.samples_deg.is_empty() || !az0_deg.is_finite() || !az1_deg.is_finite() {
            return;
        }
        let n = self.samples_deg.len();
        let offset = normalize_azimuth_offset(self.azimuth_offset_deg);
        let img_az0 = (az0_deg - offset).rem_euclid(360.0);
        let img_az1 = (az1_deg - offset).rem_euclid(360.0);

        let mut delta_deg = (img_az1 - img_az0).rem_euclid(360.0);
        if delta_deg > 180.0 {
            delta_deg -= 360.0;
        }

        let p0 = (img_az0 / 360.0) * n as f64;
        let p1 = p0 + (delta_deg / 360.0) * n as f64;
        let a0 = sanitize_sample(alt0_deg);
        let a1 = sanitize_sample(alt1_deg);

        let min_p = p0.min(p1);
        let max_p = p0.max(p1);
        let k_start = min_p.round() as i64;
        let k_end = max_p.round() as i64;

        if k_start == k_end || (p1 - p0).abs() <= 1e-6 {
            let idx = k_end.rem_euclid(n as i64) as usize;
            self.samples_deg[idx] = a1;
            return;
        }

        let span = p1 - p0;
        for k in k_start..=k_end {
            let t = ((k as f64 - p0) / span).clamp(0.0, 1.0) as f32;
            let idx = k.rem_euclid(n as i64) as usize;
            self.samples_deg[idx] = sanitize_sample(a0 + (a1 - a0) * t);
        }
    }

    /// Applies `passes` iterations of circular `[0.25, 0.5, 0.25]` smoothing.
    pub fn smooth_circular(&mut self, passes: usize) {
        let n = self.samples_deg.len();
        if n < 3 || passes == 0 {
            return;
        }
        let mut scratch = vec![0.0_f32; n];
        for _ in 0..passes {
            for i in 0..n {
                let prev = sanitize_sample(self.samples_deg[(i + n - 1) % n]);
                let curr = sanitize_sample(self.samples_deg[i]);
                let next = sanitize_sample(self.samples_deg[(i + 1) % n]);
                scratch[i] = 0.25 * prev + 0.5 * curr + 0.25 * next;
            }
            self.samples_deg.copy_from_slice(&scratch);
        }
        self.sanitize();
    }

    /// Shifts every sample uniformly by `delta_deg` and clamps to valid bounds.
    pub fn shift_altitude(&mut self, delta_deg: f32) {
        if !delta_deg.is_finite() {
            return;
        }
        for sample in &mut self.samples_deg {
            *sample = sanitize_sample(*sample + delta_deg);
        }
    }

    /// Sets every sample uniformly to `altitude_deg`.
    pub fn flatten(&mut self, altitude_deg: f32) {
        let alt = sanitize_sample(altitude_deg);
        self.samples_deg.fill(alt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_profile_returns_constant_altitude_and_visibility() {
        let profile = HorizonProfile::flat(12.5, 360);
        assert_eq!(profile.sample_count(), 360);
        for az in [0.0, 45.3, 180.0, 359.9] {
            assert!((profile.altitude_at_azimuth_deg(az) - 12.5).abs() < 1e-6);
        }
        assert!(profile.is_visible(90.0, 12.5));
        assert!(profile.is_visible(90.0, 30.0));
        assert!(!profile.is_visible(90.0, 12.4));
        assert!((profile.clearance_deg(90.0, 20.0) - 7.5).abs() < 1e-6);
    }

    #[test]
    fn circular_interpolation_and_azimuth_offset_wrap_cleanly() {
        // 4 bins: idx 0 -> 0 deg (10), idx 1 -> 90 deg (20), idx 2 -> 180 deg (30), idx 3 -> 270 deg (0)
        let mut profile = HorizonProfile::from_samples(vec![10.0, 20.0, 30.0, 0.0], 0.0);
        assert!((profile.altitude_at_azimuth_deg(0.0) - 10.0).abs() < 1e-6);
        assert!((profile.altitude_at_azimuth_deg(45.0) - 15.0).abs() < 1e-6);
        assert!((profile.altitude_at_azimuth_deg(315.0) - 5.0).abs() < 1e-6);
        assert!((profile.altitude_at_azimuth_deg(-45.0) - 5.0).abs() < 1e-6);

        // Shift compass North offset by +90 deg: true East (az=90) now reads image bin 0 (10 deg).
        profile.azimuth_offset_deg = 90.0;
        assert!((profile.altitude_at_azimuth_deg(90.0) - 10.0).abs() < 1e-6);
        assert!((profile.altitude_at_azimuth_deg(180.0) - 20.0).abs() < 1e-6);
        assert!((profile.altitude_at_azimuth_deg(0.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn set_segment_world_az_wraps_across_north() {
        let mut profile = HorizonProfile::flat(0.0, 360);
        profile.set_segment_world_az(355.0, 10.0, 5.0, 20.0);

        assert!((profile.altitude_at_azimuth_deg(355.0) - 10.0).abs() < 1e-4);
        assert!((profile.altitude_at_azimuth_deg(0.0) - 15.0).abs() < 1e-4);
        assert!((profile.altitude_at_azimuth_deg(5.0) - 20.0).abs() < 1e-4);
        // Far side untouched
        assert!((profile.altitude_at_azimuth_deg(180.0) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn smooth_circular_smooths_across_seam_and_sanitize_clamps() {
        let profile = HorizonProfile::from_samples(
            vec![f32::NAN, 120.0, -120.0, 0.0],
            725.0,
        );
        assert_eq!(profile.samples_deg[0], 0.0);
        assert_eq!(profile.samples_deg[1], MAX_HORIZON_ALT_DEG);
        assert_eq!(profile.samples_deg[2], MIN_HORIZON_ALT_DEG);
        assert!((profile.azimuth_offset_deg - 5.0).abs() < 1e-9);

        let mut spike = HorizonProfile::flat(0.0, 8);
        spike.samples_deg[0] = 40.0;
        spike.smooth_circular(1);
        assert!((spike.samples_deg[0] - 20.0).abs() < 1e-5);
        assert!((spike.samples_deg[1] - 10.0).abs() < 1e-5);
        assert!((spike.samples_deg[7] - 10.0).abs() < 1e-5);
    }
}
