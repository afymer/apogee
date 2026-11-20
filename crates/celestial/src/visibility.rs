use hifitime::Epoch;

use crate::coords::{Angle, geodetic::GeodeticPosition};
use crate::observer::ObservationContext;

/// Computes topocentric altitude in degrees above the horizon for an equatorial ICRS target.
pub fn equatorial_altitude_deg(
    location: GeodeticPosition,
    epoch: Epoch,
    ra: Angle,
    dec: Angle,
) -> f64 {
    ObservationContext::new(location, epoch)
        .equatorial_to_topocentric_coords(ra, dec)
        .1
        .degrees()
}

/// Samples topocentric altitude (in degrees) evenly across a 24-hour window `[0h..24h]`.
pub fn sample_day_equatorial_altitude(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    samples: usize,
) -> Vec<f64> {
    let count = samples.max(2);
    let denom = (count - 1) as f64;
    (0..count)
        .map(|i| {
            let t_secs = local_midnight_unix_secs + (i as f64 / denom) * 86_400.0;
            equatorial_altitude_deg(location, Epoch::from_unix_seconds(t_secs), ra, dec)
        })
        .collect()
}

/// Rise, transit, set, and altitude extrema over a 24-hour local calendar day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyTargetVisibility {
    pub rise_hour: Option<f64>,
    pub transit_hour: f64,
    pub max_altitude_deg: f64,
    pub min_altitude_deg: f64,
    pub set_hour: Option<f64>,
    pub circumpolar: bool,
    pub never_rises: bool,
}

fn refine_horizon_crossing(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    mut h_lo: f64,
    mut h_hi: f64,
    rising: bool,
) -> f64 {
    for _ in 0..8 {
        let mid = f64::midpoint(h_lo, h_hi);
        let alt = equatorial_altitude_deg(
            location,
            Epoch::from_unix_seconds(local_midnight_unix_secs + mid * 3600.0),
            ra,
            dec,
        );
        let above = alt >= 0.0;
        if (rising && above) || (!rising && !above) {
            h_hi = mid;
        } else {
            h_lo = mid;
        }
    }
    f64::midpoint(h_lo, h_hi)
}

fn refine_extremum(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    mut h_lo: f64,
    mut h_hi: f64,
    find_max: bool,
) -> (f64, f64) {
    // 2/3-point ternary search converges quickly on the unimodal transit/lower-culmination window.
    for _ in 0..16 {
        let third = (h_hi - h_lo) / 3.0;
        let m1 = h_lo + third;
        let m2 = h_hi - third;
        let a1 = equatorial_altitude_deg(
            location,
            Epoch::from_unix_seconds(local_midnight_unix_secs + m1 * 3600.0),
            ra,
            dec,
        );
        let a2 = equatorial_altitude_deg(
            location,
            Epoch::from_unix_seconds(local_midnight_unix_secs + m2 * 3600.0),
            ra,
            dec,
        );
        let move_right = if find_max { a1 < a2 } else { a1 > a2 };
        if move_right {
            h_lo = m1;
        } else {
            h_hi = m2;
        }
    }
    let mid = f64::midpoint(h_lo, h_hi);
    let alt = equatorial_altitude_deg(
        location,
        Epoch::from_unix_seconds(local_midnight_unix_secs + mid * 3600.0),
        ra,
        dec,
    );
    (mid, alt)
}

/// Computes rise, transit, set, and visibility flags over `[0h, 24h]` from `local_midnight_unix_secs`.
pub fn compute_target_visibility(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
) -> DailyTargetVisibility {
    const STEPS: usize = 96;

    let profile = sample_day_equatorial_altitude(
        location,
        local_midnight_unix_secs,
        ra,
        dec,
        STEPS + 1,
    );
    let step_hours = 24.0 / STEPS as f64;

    let mut rise_hour = None;
    let mut set_hour = None;
    let mut max_idx = 0usize;
    let mut min_idx = 0usize;

    for i in 0..profile.len() {
        if profile[i] > profile[max_idx] {
            max_idx = i;
        }
        if profile[i] < profile[min_idx] {
            min_idx = i;
        }
        if i == 0 {
            continue;
        }
        let prev = profile[i - 1];
        let curr = profile[i];
        let h0 = (i - 1) as f64 * step_hours;
        let h1 = i as f64 * step_hours;

        if rise_hour.is_none() && prev < 0.0 && curr >= 0.0 {
            rise_hour = Some(refine_horizon_crossing(
                location,
                local_midnight_unix_secs,
                ra,
                dec,
                h0,
                h1,
                true,
            ));
        }
        if set_hour.is_none() && prev >= 0.0 && curr < 0.0 {
            set_hour = Some(refine_horizon_crossing(
                location,
                local_midnight_unix_secs,
                ra,
                dec,
                h0,
                h1,
                false,
            ));
        }
    }

    let max_lo = max_idx.saturating_sub(1) as f64 * step_hours;
    let max_hi = (max_idx + 1).min(STEPS) as f64 * step_hours;
    let (refined_transit, refined_max) = refine_extremum(
        location,
        local_midnight_unix_secs,
        ra,
        dec,
        max_lo,
        max_hi,
        true,
    );
    let (transit_hour, max_altitude_deg) = if refined_max >= profile[max_idx] {
        (refined_transit, refined_max)
    } else {
        (max_idx as f64 * step_hours, profile[max_idx])
    };

    let min_lo = min_idx.saturating_sub(1) as f64 * step_hours;
    let min_hi = (min_idx + 1).min(STEPS) as f64 * step_hours;
    let (_, refined_min) = refine_extremum(
        location,
        local_midnight_unix_secs,
        ra,
        dec,
        min_lo,
        min_hi,
        false,
    );
    let min_altitude_deg = refined_min.min(profile[min_idx]);

    let circumpolar = min_altitude_deg >= 0.0;
    let never_rises = max_altitude_deg < 0.0;
    if circumpolar || never_rises {
        rise_hour = None;
        set_hour = None;
    }

    DailyTargetVisibility {
        rise_hour,
        transit_hour,
        max_altitude_deg,
        min_altitude_deg,
        set_hour,
        circumpolar,
        never_rises,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polaris_is_circumpolar_from_paris() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let midnight = Epoch::from_gregorian_utc(2026, 6, 21, 0, 0, 0, 0).to_unix_seconds();
        let vis = compute_target_visibility(
            paris,
            midnight,
            Angle::from_degrees(37.9546),
            Angle::from_degrees(89.2641),
        );
        assert!(vis.circumpolar);
        assert!(!vis.never_rises);
        assert!(vis.min_altitude_deg > 45.0);
        assert!(vis.rise_hour.is_none());
        assert!(vis.set_hour.is_none());
    }

    #[test]
    fn acrux_never_rises_from_paris() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let midnight = Epoch::from_gregorian_utc(2026, 6, 21, 0, 0, 0, 0).to_unix_seconds();
        let vis = compute_target_visibility(
            paris,
            midnight,
            Angle::from_degrees(186.6496),
            Angle::from_degrees(-63.0991),
        );
        assert!(vis.never_rises);
        assert!(!vis.circumpolar);
        assert!(vis.max_altitude_deg < 0.0);
        assert!(vis.rise_hour.is_none());
        assert!(vis.set_hour.is_none());
    }

    #[test]
    fn equatorial_target_rises_transits_zenith_and_sets_from_equator() {
        let equator = GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        let midnight = Epoch::from_gregorian_utc(2026, 3, 20, 0, 0, 0, 0).to_unix_seconds();
        let ra = Angle::from_degrees(90.0);
        let dec = Angle::from_degrees(0.0);
        let vis = compute_target_visibility(equator, midnight, ra, dec);

        assert!(!vis.circumpolar);
        assert!(!vis.never_rises);
        assert!(vis.max_altitude_deg > 88.0, "max={}", vis.max_altitude_deg);

        let rise = vis.rise_hour.expect("equatorial star should rise");
        let set = vis.set_hour.expect("equatorial star should set");
        let above_hours = (set - rise).rem_euclid(24.0);
        assert!(
            (above_hours - 11.97).abs() < 0.15,
            "expected ~12h above horizon, got {above_hours}"
        );
    }

    #[test]
    fn sample_endpoints_match_direct_altitude() {
        let loc = GeodeticPosition::from_degrees(40.0, -75.0, 10.0);
        let midnight = Epoch::from_gregorian_utc(2026, 9, 15, 4, 0, 0, 0).to_unix_seconds();
        let ra = Angle::from_degrees(101.287);
        let dec = Angle::from_degrees(-16.716);

        let samples = sample_day_equatorial_altitude(loc, midnight, ra, dec, 97);
        assert_eq!(samples.len(), 97);
        let first = equatorial_altitude_deg(loc, Epoch::from_unix_seconds(midnight), ra, dec);
        let last =
            equatorial_altitude_deg(loc, Epoch::from_unix_seconds(midnight + 86_400.0), ra, dec);
        assert!((samples[0] - first).abs() < 1e-10);
        assert!((samples[96] - last).abs() < 1e-10);
    }
}
