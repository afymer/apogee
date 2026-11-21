use hifitime::Epoch;

use crate::coords::{Angle, geodetic::GeodeticPosition};
use crate::horizon::HorizonProfile;
use crate::observer::ObservationContext;

/// Computes topocentric `(azimuth_deg, altitude_deg)` for an equatorial ICRS target.
pub fn equatorial_az_alt_deg(
    location: GeodeticPosition,
    epoch: Epoch,
    ra: Angle,
    dec: Angle,
) -> (f64, f64) {
    let (az, alt) = ObservationContext::new(location, epoch)
        .equatorial_to_topocentric_coords(ra, dec);
    (az.degrees(), alt.degrees())
}

/// Computes topocentric altitude in degrees above the horizon for an equatorial ICRS target.
pub fn equatorial_altitude_deg(
    location: GeodeticPosition,
    epoch: Epoch,
    ra: Angle,
    dec: Angle,
) -> f64 {
    equatorial_az_alt_deg(location, epoch, ra, dec).1
}

/// Samples topocentric `(azimuth_deg, altitude_deg)` evenly across a 24-hour window `[0h..24h]`.
pub fn sample_day_equatorial_track(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    samples: usize,
) -> Vec<(f64, f64)> {
    let count = samples.max(2);
    let denom = (count - 1) as f64;
    (0..count)
        .map(|i| {
            let t_secs = local_midnight_unix_secs + (i as f64 / denom) * 86_400.0;
            equatorial_az_alt_deg(location, Epoch::from_unix_seconds(t_secs), ra, dec)
        })
        .collect()
}

/// Samples topocentric altitude (in degrees) evenly across a 24-hour window `[0h..24h]`.
pub fn sample_day_equatorial_altitude(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    samples: usize,
) -> Vec<f64> {
    sample_day_equatorial_track(location, local_midnight_unix_secs, ra, dec, samples)
        .into_iter()
        .map(|(_, alt)| alt)
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

fn clearance_at_hour(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    hour: f64,
    horizon: Option<&HorizonProfile>,
) -> f64 {
    let (az, alt) = equatorial_az_alt_deg(
        location,
        Epoch::from_unix_seconds(local_midnight_unix_secs + hour * 3600.0),
        ra,
        dec,
    );
    alt - horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az))
}

#[allow(clippy::too_many_arguments)]
fn refine_clearance_crossing(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    horizon: Option<&HorizonProfile>,
    mut h_lo: f64,
    mut h_hi: f64,
    rising: bool,
) -> f64 {
    for _ in 0..8 {
        let mid = f64::midpoint(h_lo, h_hi);
        let clearance =
            clearance_at_hour(location, local_midnight_unix_secs, ra, dec, mid, horizon);
        let above = clearance >= 0.0;
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
    for _ in 0..26 {
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

/// Computes rise, transit, set, and visibility flags over `[0h, 24h]` from `local_midnight_unix_secs`
/// using the flat geometric `0°` horizon.
pub fn compute_target_visibility(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
) -> DailyTargetVisibility {
    compute_target_visibility_with_horizon(location, local_midnight_unix_secs, ra, dec, None)
}

/// Computes rise, transit, set, and visibility flags over `[0h, 24h]` from `local_midnight_unix_secs`,
/// optionally accounting for a custom [`HorizonProfile`].
///
/// `transit_hour`, `max_altitude_deg`, and `min_altitude_deg` always report the true astronomical
/// culmination values; `rise_hour`, `set_hour`, `circumpolar`, and `never_rises` reflect visibility
/// above `horizon` (or `0°` when `None`).
pub fn compute_target_visibility_with_horizon(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    horizon: Option<&HorizonProfile>,
) -> DailyTargetVisibility {
    let steps: usize = if horizon.is_some() { 288 } else { 96 };
    let step_hours = 24.0 / steps as f64;

    let track = sample_day_equatorial_track(
        location,
        local_midnight_unix_secs,
        ra,
        dec,
        steps + 1,
    );

    let mut profile = Vec::with_capacity(track.len());
    let mut clearances = Vec::with_capacity(track.len());
    for &(az, alt) in &track {
        profile.push(alt);
        let h_alt = horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az));
        clearances.push(alt - h_alt);
    }

    let mut max_idx = 0usize;
    let mut min_idx = 0usize;
    for (i, &alt) in profile.iter().enumerate() {
        if alt > profile[max_idx] {
            max_idx = i;
        }
        if alt < profile[min_idx] {
            min_idx = i;
        }
    }

    let max_lo = max_idx.saturating_sub(1) as f64 * step_hours;
    let max_hi = (max_idx + 1).min(steps) as f64 * step_hours;
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
    let min_hi = (min_idx + 1).min(steps) as f64 * step_hours;
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

    let mut intervals: Vec<(f64, f64)> = Vec::new();
    let mut open_start: Option<f64> = if clearances[0] >= 0.0 {
        Some(0.0)
    } else {
        None
    };

    for i in 1..clearances.len() {
        let prev = clearances[i - 1];
        let curr = clearances[i];
        let h0 = (i - 1) as f64 * step_hours;
        let h1 = i as f64 * step_hours;

        if prev < 0.0 && curr >= 0.0 {
            let rise = refine_clearance_crossing(
                location,
                local_midnight_unix_secs,
                ra,
                dec,
                horizon,
                h0,
                h1,
                true,
            );
            open_start = Some(rise);
        } else if prev >= 0.0 && curr < 0.0 {
            let set = refine_clearance_crossing(
                location,
                local_midnight_unix_secs,
                ra,
                dec,
                horizon,
                h0,
                h1,
                false,
            );
            if let Some(start) = open_start.take() {
                intervals.push((start, set));
            }
        }
    }
    if let Some(start) = open_start.take() {
        intervals.push((start, 24.0));
    }

    let (circumpolar, never_rises) = if horizon.is_none() {
        (min_altitude_deg >= 0.0, max_altitude_deg < 0.0)
    } else {
        let all_above =
            intervals.len() == 1 && intervals[0].0 == 0.0 && intervals[0].1 == 24.0;
        let all_below = intervals.is_empty();
        (all_above, all_below)
    };

    let (rise_hour, set_hour) = if circumpolar || never_rises || intervals.is_empty() {
        (None, None)
    } else {
        let has_wrap = intervals.len() >= 2
            && intervals[0].0 == 0.0
            && intervals[intervals.len() - 1].1 == 24.0;
        let mut best: Option<(f64, f64, f64, bool)> = None;

        if has_wrap {
            let first_end = intervals[0].1;
            let last_start = intervals[intervals.len() - 1].0;
            let wrap_dur = (24.0 - last_start) + first_end;
            best = Some((last_start, first_end, wrap_dur, true));
            for &(s, e) in &intervals[1..intervals.len() - 1] {
                let dur = e - s;
                if best.is_none_or(|(_, _, bd, _)| dur > bd) {
                    best = Some((s, e, dur, false));
                }
            }
        } else {
            for &(s, e) in &intervals {
                let dur = e - s;
                if best.is_none_or(|(_, _, bd, _)| dur > bd) {
                    best = Some((s, e, dur, false));
                }
            }
        }

        match best {
            Some((r, s, _, wrapped)) => (
                if wrapped || r > 0.0 { Some(r) } else { None },
                if wrapped || s < 24.0 { Some(s) } else { None },
            ),
            None => (None, None),
        }
    };

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

    #[test]
    fn elevated_flat_horizon_shortens_visibility_window() {
        let equator = GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        let midnight = Epoch::from_gregorian_utc(2026, 3, 20, 0, 0, 0, 0).to_unix_seconds();
        let ra = Angle::from_degrees(90.0);
        let dec = Angle::from_degrees(0.0);

        let flat_zero = compute_target_visibility_with_horizon(equator, midnight, ra, dec, None);
        let elevated = HorizonProfile::flat(30.0, 360);
        let vis_elevated =
            compute_target_visibility_with_horizon(equator, midnight, ra, dec, Some(&elevated));

        assert!(!vis_elevated.circumpolar);
        assert!(!vis_elevated.never_rises);
        assert!((vis_elevated.max_altitude_deg - flat_zero.max_altitude_deg).abs() < 1e-3);

        let rise = vis_elevated.rise_hour.expect("should rise above 30 deg");
        let set = vis_elevated.set_hour.expect("should set below 30 deg");
        let hours_above = (set - rise).rem_euclid(24.0);
        // At equator for dec=0, altitude > 30 deg spans 120 deg of hour angle (~8 sidereal hours).
        assert!(
            (hours_above - 7.98).abs() < 0.25,
            "expected ~8h above 30 deg, got {hours_above}"
        );
    }

    #[test]
    fn directional_eastern_obstacle_delays_rise_without_shifting_set() {
        let equator = GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        let midnight = Epoch::from_gregorian_utc(2026, 3, 20, 0, 0, 0, 0).to_unix_seconds();
        let ra = Angle::from_degrees(90.0);
        let dec = Angle::from_degrees(0.0);

        let baseline = compute_target_visibility_with_horizon(equator, midnight, ra, dec, None);
        let mut east_wall = HorizonProfile::flat(0.0, 360);
        // Block azimuth 45..135 (East) up to 45 deg
        east_wall.set_segment_world_az(45.0, 45.0, 135.0, 45.0);

        let vis =
            compute_target_visibility_with_horizon(equator, midnight, ra, dec, Some(&east_wall));
        let base_rise = baseline.rise_hour.unwrap();
        let wall_rise = vis.rise_hour.unwrap();
        let base_set = baseline.set_hour.unwrap();
        let wall_set = vis.set_hour.unwrap();

        let rise_delay = (wall_rise - base_rise).rem_euclid(24.0);
        assert!(
            (rise_delay - 3.0).abs() < 0.25,
            "expected ~3h rise delay from 45 deg East wall, got {rise_delay}"
        );
        assert!(
            (wall_set - base_set).abs() < 0.1,
            "West set should be unchanged: base={base_set}, wall={wall_set}"
        );
    }

    #[test]
    fn blocked_target_is_never_rises_even_when_max_altitude_positive() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let midnight = Epoch::from_gregorian_utc(2026, 6, 21, 0, 0, 0, 0).to_unix_seconds();
        // Rigel-ish southern star peaking around ~33 deg at Paris, blocked by 45 deg horizon
        let ra = Angle::from_degrees(78.634);
        let dec = Angle::from_degrees(-8.2016);
        let high_horizon = HorizonProfile::flat(45.0, 360);
        let vis = compute_target_visibility_with_horizon(
            paris,
            midnight,
            ra,
            dec,
            Some(&high_horizon),
        );
        assert!(vis.max_altitude_deg > 20.0);
        assert!(vis.never_rises);
        assert!(!vis.circumpolar);
        assert!(vis.rise_hour.is_none());
        assert!(vis.set_hour.is_none());
    }

    #[test]
    fn obstructed_circumpolar_target_loses_circumpolar_flag() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let midnight = Epoch::from_gregorian_utc(2026, 6, 21, 0, 0, 0, 0).to_unix_seconds();
        // Dec +60 at lat +48.86 ranges between ~18.9 deg and ~78.9 deg altitude.
        let ra = Angle::from_degrees(180.0);
        let dec = Angle::from_degrees(60.0);
        let base = compute_target_visibility_with_horizon(paris, midnight, ra, dec, None);
        assert!(base.circumpolar);

        let wall = HorizonProfile::flat(35.0, 360);
        let vis = compute_target_visibility_with_horizon(paris, midnight, ra, dec, Some(&wall));
        assert!(!vis.circumpolar);
        assert!(!vis.never_rises);
        assert!(vis.rise_hour.is_some());
        assert!(vis.set_hour.is_some());
    }
}
