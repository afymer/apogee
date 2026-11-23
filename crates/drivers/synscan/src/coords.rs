//! Topocentric / equatorial coordinate conversions and time helpers for SynScan.

use std::time::{SystemTime, UNIX_EPOCH};

/// Default observer coordinates (Paris) used until `SetSite` syncs the active profile.
pub const DEFAULT_SITE_LAT_DEG: f64 = 48.8566;
pub const DEFAULT_SITE_LON_DEG: f64 = 2.3522;

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn wrap_signed_180(deg: f64) -> f64 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Default counterweight-down celestial-pole home/park position `(az_deg, alt_deg)`.
pub fn pole_horizontal(lat_deg: f64) -> (f64, f64) {
    let az = if lat_deg >= 0.0 { 0.0 } else { 180.0 };
    let alt = lat_deg.abs().clamp(1.0, 89.0);
    (az, alt)
}

/// Computes approximate Local Sidereal Time in degrees `[0, 360)` for the current UTC instant.
pub fn current_lst_deg(longitude_deg: f64) -> f64 {
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    lst_deg_at_unix_secs(unix_secs, longitude_deg)
}

/// Computes approximate Local Sidereal Time in degrees `[0, 360)` at a given UTC Unix timestamp.
pub fn lst_deg_at_unix_secs(unix_secs: f64, longitude_deg: f64) -> f64 {
    let jd = unix_secs / 86_400.0 + 2_440_587.5;
    let d = jd - 2_451_545.0;
    let t = d / 36_525.0;
    let gmst = 280.460_618_37 + 360.985_647_366_29 * d + 0.000_387_933 * t * t
        - (t * t * t) / 38_710_000.0;
    (gmst + longitude_deg).rem_euclid(360.0)
}

/// Converts equatorial `(ra_deg, dec_deg)` to horizontal `(az_deg, alt_deg)` (Az=0 N, 90 E).
pub fn equatorial_to_horizontal(
    ra_deg: f64,
    dec_deg: f64,
    lat_deg: f64,
    lst_deg: f64,
) -> (f64, f64) {
    let ha = (lst_deg - ra_deg).to_radians();
    let dec = dec_deg.to_radians();
    let lat = lat_deg.to_radians();

    let (sin_dec, cos_dec) = dec.sin_cos();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_ha, cos_ha) = ha.sin_cos();

    let sin_alt = (sin_dec * sin_lat + cos_dec * cos_lat * cos_ha).clamp(-1.0, 1.0);
    let alt = sin_alt.asin();

    let y = -cos_dec * sin_ha;
    let x = sin_dec * cos_lat - cos_dec * sin_lat * cos_ha;
    let az = y.atan2(x).to_degrees().rem_euclid(360.0);
    (az, alt.to_degrees())
}

/// Converts horizontal `(az_deg, alt_deg)` to equatorial `(ra_deg, dec_deg)`.
pub fn horizontal_to_equatorial(
    az_deg: f64,
    alt_deg: f64,
    lat_deg: f64,
    lst_deg: f64,
) -> (f64, f64) {
    let az = az_deg.to_radians();
    let alt = alt_deg.to_radians();
    let lat = lat_deg.to_radians();

    let (sin_alt, cos_alt) = alt.sin_cos();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_az, cos_az) = az.sin_cos();

    let sin_dec = (sin_alt * sin_lat + cos_alt * cos_lat * cos_az).clamp(-1.0, 1.0);
    let dec = sin_dec.asin();

    let y = -cos_alt * sin_az;
    let x = sin_alt * cos_lat - cos_alt * sin_lat * cos_az;
    let ha = y.atan2(x).to_degrees();
    let ra = (lst_deg - ha).rem_euclid(360.0);
    (ra, dec.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equatorial_horizontal_matches_known_reference_points() {
        let lat = 45.0;
        let lst = 120.0;

        // Zenith: RA = LST, Dec = lat -> alt = 90°
        let (_, alt_z) = equatorial_to_horizontal(lst, lat, lat, lst);
        assert!((alt_z - 90.0).abs() < 1e-6);

        // North Celestial Pole -> Az = 0°, Alt = lat
        let (az_p, alt_p) = equatorial_to_horizontal(0.0, 90.0, lat, lst);
        assert!(wrap_signed_180(az_p).abs() < 1e-6);
        assert!((alt_p - lat).abs() < 1e-6);

        // Rising due East on celestial equator (HA = -90°, Dec = 0°) -> Az = 90°, Alt = 0°
        let (az_e, alt_e) = equatorial_to_horizontal(lst + 90.0, 0.0, lat, lst);
        assert!((az_e - 90.0).abs() < 1e-6);
        assert!(alt_e.abs() < 1e-6);

        // Round-trip at mid-sky point
        let (az, alt) = equatorial_to_horizontal(105.0, 32.0, lat, lst);
        let (ra2, dec2) = horizontal_to_equatorial(az, alt, lat, lst);
        assert!(wrap_signed_180(ra2 - 105.0).abs() < 1e-6);
        assert!((dec2 - 32.0).abs() < 1e-6);
    }

    #[test]
    fn pole_horizontal_handles_both_hemispheres() {
        assert_eq!(pole_horizontal(48.8566), (0.0, 48.8566));
        assert_eq!(pole_horizontal(-33.8688), (180.0, 33.8688));
    }
}
