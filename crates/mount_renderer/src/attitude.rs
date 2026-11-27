//! Shared exponential low-pass filter for smoothing discrete mount telemetry updates.

use celestial::coords::Angle;

/// Exponential low-pass filter time constant (seconds) for smoothing 150ms mount telemetry.
pub const ATTITUDE_FILTER_TAU_SECS: f64 = 0.095;
/// Angular error threshold (degrees) below which the filter snaps cleanly to the target.
pub const ATTITUDE_SETTLE_EPS_DEG: f64 = 0.002;

/// Steps a single `(ra_deg, dec_deg)` or `(az_deg, alt_deg)` pair toward its target using
/// an exponential low-pass filter. Returns `true` if still moving, or `false` once settled.
///
/// Non-finite target coordinates are ignored to prevent `NaN` poisoning.
pub fn step_angular_lowpass(
    cur_lon_deg: &mut f64,
    cur_lat_deg: &mut f64,
    target_lon_deg: f64,
    target_lat_deg: f64,
    dt_secs: f64,
) -> bool {
    if !target_lon_deg.is_finite() || !target_lat_deg.is_finite() {
        return false;
    }
    if !cur_lon_deg.is_finite() || !cur_lat_deg.is_finite() {
        *cur_lon_deg = target_lon_deg.rem_euclid(360.0);
        *cur_lat_deg = target_lat_deg.clamp(-90.0, 90.0);
        return false;
    }

    let dt = dt_secs.clamp(0.001, 1.0 / 30.0);
    let alpha = 1.0 - (-dt / ATTITUDE_FILTER_TAU_SECS).exp();

    let dlon = Angle::from_degrees(target_lon_deg - *cur_lon_deg).signed_degrees();
    let dlat = target_lat_deg - *cur_lat_deg;
    let max_err = dlon.abs().max(dlat.abs());

    if max_err > ATTITUDE_SETTLE_EPS_DEG {
        *cur_lon_deg = (*cur_lon_deg + dlon * alpha).rem_euclid(360.0);
        *cur_lat_deg = (*cur_lat_deg + dlat * alpha).clamp(-90.0, 90.0);
        true
    } else {
        *cur_lon_deg = target_lon_deg.rem_euclid(360.0);
        *cur_lat_deg = target_lat_deg.clamp(-90.0, 90.0);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_across_zero_360_boundary_shortest_path() {
        let mut ra = 359.0;
        let mut dec = 10.0;
        let moving = step_angular_lowpass(&mut ra, &mut dec, 1.0, 10.0, 1.0 / 60.0);
        assert!(moving);
        // Should move forward past 359° (either >359 or <1), never backwards toward 180°
        assert!(ra > 359.0 || ra < 1.0, "unexpected wrapped ra={ra}");
    }

    #[test]
    fn ignores_nan_targets_without_poisoning_state() {
        let mut ra = 42.0;
        let mut dec = 23.0;
        assert!(!step_angular_lowpass(
            &mut ra,
            &mut dec,
            f64::NAN,
            23.0,
            0.016
        ));
        assert_eq!(ra, 42.0);
        assert_eq!(dec, 23.0);
    }

    #[test]
    fn snaps_when_within_settle_epsilon() {
        let mut ra = 100.0;
        let mut dec = 20.0;
        assert!(!step_angular_lowpass(
            &mut ra, &mut dec, 100.001, 20.001, 0.016
        ));
        assert_eq!(ra, 100.001);
        assert_eq!(dec, 20.001);
    }
}
