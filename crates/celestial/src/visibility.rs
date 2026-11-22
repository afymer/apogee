use glam::DVec3;
use hifitime::Epoch;

use crate::coords::{Angle, geodetic::GeodeticPosition, topocentric_spherical_to_enu};
use crate::horizon::HorizonProfile;
use crate::observer::ObservationContext;
use crate::solar::{SolarSystemBody, solar_system_ephemeris};

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

/// Computes topocentric `(azimuth_deg, altitude_deg)` for a [`SolarSystemBody`].
pub fn solar_system_az_alt_deg(
    location: GeodeticPosition,
    epoch: Epoch,
    body: SolarSystemBody,
) -> (f64, f64) {
    let eph = solar_system_ephemeris(body, location, epoch);
    (eph.az_deg, eph.alt_deg)
}

/// Samples topocentric `(azimuth_deg, altitude_deg)` evenly across a 24-hour window `[0h..24h]`.
pub fn sample_day_equatorial_track(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    samples: usize,
) -> Vec<(f64, f64)> {
    sample_day_track_with(local_midnight_unix_secs, samples, &|epoch| {
        equatorial_az_alt_deg(location, epoch, ra, dec)
    })
}

/// Samples topocentric `(azimuth_deg, altitude_deg)` for a [`SolarSystemBody`] across `[0h..24h]`.
pub fn sample_day_solar_system_track(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    body: SolarSystemBody,
    samples: usize,
) -> Vec<(f64, f64)> {
    sample_day_track_with(local_midnight_unix_secs, samples, &|epoch| {
        solar_system_az_alt_deg(location, epoch, body)
    })
}

fn sample_day_track_with(
    local_midnight_unix_secs: f64,
    samples: usize,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> Vec<(f64, f64)> {
    let count = samples.max(2);
    let denom = (count - 1) as f64;
    (0..count)
        .map(|i| {
            let t_secs = local_midnight_unix_secs + (i as f64 / denom) * 86_400.0;
            az_alt_at_epoch(Epoch::from_unix_seconds(t_secs))
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
    pub lower_transit_hour: f64,
    pub max_altitude_deg: f64,
    pub min_altitude_deg: f64,
    pub set_hour: Option<f64>,
    pub circumpolar: bool,
    pub never_rises: bool,
}

/// Notable event kind marked along a selected object's trajectory in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrajectoryPointKind {
    Rise,
    Transit,
    Set,
    LowerTransit,
}

impl TrajectoryPointKind {
    /// Short display label for planetarium trajectory badges.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rise => "Rise",
            Self::Transit => "Transit",
            Self::Set => "Set",
            Self::LowerTransit => "Low",
        }
    }
}

/// Notable event point (Rise, Transit, Set, or Lower Culmination) along a daily trajectory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrajectoryEventPoint {
    pub kind: TrajectoryPointKind,
    pub hour: f64,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub topo_enu: DVec3,
    pub above_horizon: bool,
}

/// Single sampled point along an object's 24-hour topocentric sky trajectory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrajectorySample {
    pub hour: f64,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub topo_enu: DVec3,
    pub above_horizon: bool,
}

/// Complete 24-hour topocentric sky trajectory, visibility summary, and notable event markers.
#[derive(Clone, Debug, PartialEq)]
pub struct DailyTrajectory {
    pub samples: Vec<TrajectorySample>,
    pub events: Vec<TrajectoryEventPoint>,
    pub visibility: DailyTargetVisibility,
}

fn clearance_at_hour_with(
    local_midnight_unix_secs: f64,
    hour: f64,
    horizon: Option<&HorizonProfile>,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> f64 {
    let (az, alt) = az_alt_at_epoch(Epoch::from_unix_seconds(
        local_midnight_unix_secs + hour * 3600.0,
    ));
    alt - horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az))
}

fn refine_clearance_crossing_with(
    local_midnight_unix_secs: f64,
    horizon: Option<&HorizonProfile>,
    mut h_lo: f64,
    mut h_hi: f64,
    rising: bool,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> f64 {
    for _ in 0..8 {
        let mid = f64::midpoint(h_lo, h_hi);
        let clearance =
            clearance_at_hour_with(local_midnight_unix_secs, mid, horizon, az_alt_at_epoch);
        let above = clearance >= 0.0;
        if (rising && above) || (!rising && !above) {
            h_hi = mid;
        } else {
            h_lo = mid;
        }
    }
    f64::midpoint(h_lo, h_hi)
}

fn refine_extremum_with(
    local_midnight_unix_secs: f64,
    mut h_lo: f64,
    mut h_hi: f64,
    find_max: bool,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> (f64, f64) {
    // 2/3-point ternary search converges quickly on the unimodal transit/lower-culmination window.
    for _ in 0..26 {
        let third = (h_hi - h_lo) / 3.0;
        let m1 = h_lo + third;
        let m2 = h_hi - third;
        let a1 = az_alt_at_epoch(Epoch::from_unix_seconds(
            local_midnight_unix_secs + m1 * 3600.0,
        ))
        .1;
        let a2 = az_alt_at_epoch(Epoch::from_unix_seconds(
            local_midnight_unix_secs + m2 * 3600.0,
        ))
        .1;
        let move_right = if find_max { a1 < a2 } else { a1 > a2 };
        if move_right {
            h_lo = m1;
        } else {
            h_hi = m2;
        }
    }
    let mid = f64::midpoint(h_lo, h_hi);
    let alt = az_alt_at_epoch(Epoch::from_unix_seconds(
        local_midnight_unix_secs + mid * 3600.0,
    ))
    .1;
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
/// `transit_hour`, `lower_transit_hour`, `max_altitude_deg`, and `min_altitude_deg` always report
/// the true astronomical culmination values; `rise_hour`, `set_hour`, `circumpolar`, and `never_rises`
/// reflect visibility above `horizon` (or `0°` when `None`).
pub fn compute_target_visibility_with_horizon(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    horizon: Option<&HorizonProfile>,
) -> DailyTargetVisibility {
    compute_visibility_with_horizon_fn(local_midnight_unix_secs, horizon, &|epoch| {
        equatorial_az_alt_deg(location, epoch, ra, dec)
    })
}

/// Computes rise, transit, set, and visibility flags for a [`SolarSystemBody`] over `[0h, 24h]`,
/// optionally accounting for a custom [`HorizonProfile`].
pub fn compute_solar_system_visibility_with_horizon(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    body: SolarSystemBody,
    horizon: Option<&HorizonProfile>,
) -> DailyTargetVisibility {
    compute_visibility_with_horizon_fn(local_midnight_unix_secs, horizon, &|epoch| {
        solar_system_az_alt_deg(location, epoch, body)
    })
}

fn compute_visibility_with_horizon_fn(
    local_midnight_unix_secs: f64,
    horizon: Option<&HorizonProfile>,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> DailyTargetVisibility {
    let steps: usize = if horizon.is_some() { 288 } else { 96 };
    let step_hours = 24.0 / steps as f64;

    let track = sample_day_track_with(local_midnight_unix_secs, steps + 1, az_alt_at_epoch);

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
    let (refined_transit, refined_max) = refine_extremum_with(
        local_midnight_unix_secs,
        max_lo,
        max_hi,
        true,
        az_alt_at_epoch,
    );
    let (transit_hour, max_altitude_deg) = if refined_max >= profile[max_idx] {
        (refined_transit, refined_max)
    } else {
        (max_idx as f64 * step_hours, profile[max_idx])
    };

    let min_lo = min_idx.saturating_sub(1) as f64 * step_hours;
    let min_hi = (min_idx + 1).min(steps) as f64 * step_hours;
    let (refined_lower_transit, refined_min) = refine_extremum_with(
        local_midnight_unix_secs,
        min_lo,
        min_hi,
        false,
        az_alt_at_epoch,
    );
    let (lower_transit_hour, min_altitude_deg) = if refined_min <= profile[min_idx] {
        (refined_lower_transit, refined_min)
    } else {
        (min_idx as f64 * step_hours, profile[min_idx])
    };

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
            let rise = refine_clearance_crossing_with(
                local_midnight_unix_secs,
                horizon,
                h0,
                h1,
                true,
                az_alt_at_epoch,
            );
            open_start = Some(rise);
        } else if prev >= 0.0 && curr < 0.0 {
            let set = refine_clearance_crossing_with(
                local_midnight_unix_secs,
                horizon,
                h0,
                h1,
                false,
                az_alt_at_epoch,
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
        lower_transit_hour,
        max_altitude_deg,
        min_altitude_deg,
        set_hour,
        circumpolar,
        never_rises,
    }
}

/// Builds a 24-hour topocentric [`DailyTrajectory`] and notable event markers (Rise, Transit, Set,
/// Lower Culmination) for an equatorial ICRS target.
pub fn compute_equatorial_daily_trajectory(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    ra: Angle,
    dec: Angle,
    horizon: Option<&HorizonProfile>,
    base_samples: usize,
) -> DailyTrajectory {
    compute_daily_trajectory_with(local_midnight_unix_secs, horizon, base_samples, &|epoch| {
        equatorial_az_alt_deg(location, epoch, ra, dec)
    })
}

/// Builds a 24-hour topocentric [`DailyTrajectory`] and notable event markers (Rise, Transit, Set,
/// Lower Culmination) for a [`SolarSystemBody`].
pub fn compute_solar_system_daily_trajectory(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    body: SolarSystemBody,
    horizon: Option<&HorizonProfile>,
    base_samples: usize,
) -> DailyTrajectory {
    compute_daily_trajectory_with(local_midnight_unix_secs, horizon, base_samples, &|epoch| {
        solar_system_az_alt_deg(location, epoch, body)
    })
}

/// Builds a continuous topocentric [`DailyTrajectory`] for a [`SolarSystemBody`] over an arbitrary
/// window `[start_unix_secs .. start_unix_secs + duration_hours * 3600]` (for example a 12-hour
/// `[-6h .. +6h]` arc centered on the current time).
///
/// Sample and event `hour` values are offsets in `[0.0 ..= duration_hours]` from `start_unix_secs`.
/// Only Rise, Set, Transit, and LowerTransit events that genuinely occur within the window are emitted.
pub fn compute_solar_system_window_trajectory(
    location: GeodeticPosition,
    start_unix_secs: f64,
    duration_hours: f64,
    body: SolarSystemBody,
    horizon: Option<&HorizonProfile>,
    base_samples: usize,
) -> DailyTrajectory {
    let duration = duration_hours.max(1.0);
    let az_alt_at_epoch = |epoch: Epoch| solar_system_az_alt_deg(location, epoch, body);
    let count = base_samples.max(25);
    let step_hours = duration / (count - 1) as f64;

    let mut raw_alt = Vec::with_capacity(count);
    let mut raw_clear = Vec::with_capacity(count);
    for i in 0..count {
        let h = i as f64 * step_hours;
        let (az, alt) = az_alt_at_epoch(Epoch::from_unix_seconds(start_unix_secs + h * 3600.0));
        let h_alt = horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az));
        raw_alt.push(alt);
        raw_clear.push(alt - h_alt);
    }

    let mut rise_hours = Vec::new();
    let mut set_hours = Vec::new();
    let mut transit_hour_opt = None;
    let mut lower_transit_hour_opt = None;

    let mut max_idx = 0usize;
    let mut min_idx = 0usize;
    for i in 0..count {
        if raw_alt[i] > raw_alt[max_idx] {
            max_idx = i;
        }
        if raw_alt[i] < raw_alt[min_idx] {
            min_idx = i;
        }
        if i > 0 {
            let prev = raw_clear[i - 1];
            let curr = raw_clear[i];
            let h0 = (i - 1) as f64 * step_hours;
            let h1 = i as f64 * step_hours;
            if prev < 0.0 && curr >= 0.0 {
                rise_hours.push(refine_clearance_crossing_with(
                    start_unix_secs,
                    horizon,
                    h0,
                    h1,
                    true,
                    &az_alt_at_epoch,
                ));
            } else if prev >= 0.0 && curr < 0.0 {
                set_hours.push(refine_clearance_crossing_with(
                    start_unix_secs,
                    horizon,
                    h0,
                    h1,
                    false,
                    &az_alt_at_epoch,
                ));
            }
        }
    }

    let mut max_altitude_deg = raw_alt[max_idx];
    let mut min_altitude_deg = raw_alt[min_idx];
    let mut best_transit_hour = max_idx as f64 * step_hours;
    let mut best_lower_hour = min_idx as f64 * step_hours;

    // Only emit a Transit / LowerTransit event if the extremum is strictly interior to the window.
    if max_idx > 0 && max_idx + 1 < count {
        let (h, alt) = refine_extremum_with(
            start_unix_secs,
            (max_idx - 1) as f64 * step_hours,
            (max_idx + 1) as f64 * step_hours,
            true,
            &az_alt_at_epoch,
        );
        best_transit_hour = h;
        max_altitude_deg = alt;
        transit_hour_opt = Some(h);
    }
    if min_idx > 0 && min_idx + 1 < count {
        let (h, alt) = refine_extremum_with(
            start_unix_secs,
            (min_idx - 1) as f64 * step_hours,
            (min_idx + 1) as f64 * step_hours,
            false,
            &az_alt_at_epoch,
        );
        best_lower_hour = h;
        min_altitude_deg = alt;
        lower_transit_hour_opt = Some(h);
    }

    let mut hours: Vec<f64> = (0..count).map(|i| i as f64 * step_hours).collect();
    for event_h in rise_hours
        .iter()
        .copied()
        .chain(set_hours.iter().copied())
        .chain(transit_hour_opt)
        .chain(lower_transit_hour_opt)
    {
        let h = event_h.clamp(0.0, duration);
        if hours.iter().all(|&existing| (existing - h).abs() > 1e-4) {
            hours.push(h);
        }
    }
    hours.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let sample_at_hour = |hour: f64, force_above: bool| -> TrajectorySample {
        let (az_deg, alt_deg) =
            az_alt_at_epoch(Epoch::from_unix_seconds(start_unix_secs + hour * 3600.0));
        let topo_enu = topocentric_spherical_to_enu(
            Angle::from_degrees(az_deg),
            Angle::from_degrees(alt_deg),
        )
        .normalize_or_zero();
        let h_alt = horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az_deg));
        let above_horizon = force_above || alt_deg >= h_alt - 1e-3;
        TrajectorySample {
            hour,
            az_deg,
            alt_deg,
            topo_enu,
            above_horizon,
        }
    };

    let samples: Vec<TrajectorySample> = hours
        .into_iter()
        .map(|h| {
            let is_crossing = rise_hours.iter().any(|&r| (r - h).abs() < 1e-4)
                || set_hours.iter().any(|&s| (s - h).abs() < 1e-4);
            sample_at_hour(h, is_crossing)
        })
        .collect();

    let mut events = Vec::with_capacity(4);
    for &rise_h in &rise_hours {
        let s = sample_at_hour(rise_h, true);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Rise,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: true,
        });
    }
    if let Some(transit_h) = transit_hour_opt {
        let s = sample_at_hour(transit_h, false);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Transit,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: s.above_horizon,
        });
    }
    for &set_h in &set_hours {
        let s = sample_at_hour(set_h, true);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Set,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: true,
        });
    }
    if let Some(lower_h) = lower_transit_hour_opt {
        let s = sample_at_hour(lower_h, false);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::LowerTransit,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: s.above_horizon,
        });
    }

    let all_above = raw_clear.iter().all(|&c| c >= 0.0);
    let all_below = raw_clear.iter().all(|&c| c < 0.0);
    let visibility = DailyTargetVisibility {
        rise_hour: rise_hours.first().copied(),
        transit_hour: best_transit_hour,
        lower_transit_hour: best_lower_hour,
        max_altitude_deg,
        min_altitude_deg,
        set_hour: set_hours.first().copied(),
        circumpolar: all_above,
        never_rises: all_below,
    };

    DailyTrajectory {
        samples,
        events,
        visibility,
    }
}

fn compute_daily_trajectory_with(
    local_midnight_unix_secs: f64,
    horizon: Option<&HorizonProfile>,
    base_samples: usize,
    az_alt_at_epoch: &impl Fn(Epoch) -> (f64, f64),
) -> DailyTrajectory {
    let visibility =
        compute_visibility_with_horizon_fn(local_midnight_unix_secs, horizon, az_alt_at_epoch);

    let count = base_samples.max(25);
    let denom = (count - 1) as f64;
    let mut hours: Vec<f64> = (0..count).map(|i| (i as f64 / denom) * 24.0).collect();

    for event_hour in [
        visibility.rise_hour,
        Some(visibility.transit_hour),
        visibility.set_hour,
        Some(visibility.lower_transit_hour),
    ]
    .into_iter()
    .flatten()
    {
        let h = event_hour.clamp(0.0, 24.0);
        if hours.iter().all(|&existing| (existing - h).abs() > 1e-4) {
            hours.push(h);
        }
    }
    hours.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let sample_at_hour = |hour: f64, force_above: bool| -> TrajectorySample {
        let (az_deg, alt_deg) = az_alt_at_epoch(Epoch::from_unix_seconds(
            local_midnight_unix_secs + hour * 3600.0,
        ));
        let topo_enu = topocentric_spherical_to_enu(
            Angle::from_degrees(az_deg),
            Angle::from_degrees(alt_deg),
        )
        .normalize_or_zero();
        let h_alt = horizon.map_or(0.0, |prof| prof.altitude_at_azimuth_deg(az_deg));
        let above_horizon = force_above || alt_deg >= h_alt - 1e-3;
        TrajectorySample {
            hour,
            az_deg,
            alt_deg,
            topo_enu,
            above_horizon,
        }
    };

    let samples: Vec<TrajectorySample> = hours
        .into_iter()
        .map(|h| {
            let is_rise_or_set = visibility
                .rise_hour
                .is_some_and(|r| (r - h).abs() < 1e-4)
                || visibility
                    .set_hour
                    .is_some_and(|s| (s - h).abs() < 1e-4);
            sample_at_hour(h, is_rise_or_set)
        })
        .collect();

    let mut events = Vec::with_capacity(4);
    if let Some(rise_h) = visibility.rise_hour {
        let s = sample_at_hour(rise_h, true);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Rise,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: true,
        });
    }

    {
        let s = sample_at_hour(visibility.transit_hour, false);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Transit,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: s.above_horizon,
        });
    }

    if let Some(set_h) = visibility.set_hour {
        let s = sample_at_hour(set_h, true);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::Set,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: true,
        });
    }

    if (visibility.lower_transit_hour - visibility.transit_hour).abs() > 0.25 {
        let s = sample_at_hour(visibility.lower_transit_hour, false);
        events.push(TrajectoryEventPoint {
            kind: TrajectoryPointKind::LowerTransit,
            hour: s.hour,
            az_deg: s.az_deg,
            alt_deg: s.alt_deg,
            topo_enu: s.topo_enu,
            above_horizon: s.above_horizon,
        });
    }

    DailyTrajectory {
        samples,
        events,
        visibility,
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

    #[test]
    fn daily_trajectory_includes_rise_transit_set_and_solar_system_events() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let midnight = Epoch::from_gregorian_utc(2026, 3, 20, 0, 0, 0, 0).to_unix_seconds();

        let sun_traj =
            compute_solar_system_daily_trajectory(paris, midnight, SolarSystemBody::Sun, None, 97);
        assert!(sun_traj.samples.len() >= 97);
        assert!(
            sun_traj
                .events
                .iter()
                .any(|e| e.kind == TrajectoryPointKind::Rise && e.above_horizon)
        );
        let transit = sun_traj
            .events
            .iter()
            .find(|e| e.kind == TrajectoryPointKind::Transit)
            .expect("Sun trajectory must have Transit event");
        assert!(transit.above_horizon);
        assert!((transit.alt_deg - sun_traj.visibility.max_altitude_deg).abs() < 1e-4);
        assert!(
            sun_traj
                .events
                .iter()
                .any(|e| e.kind == TrajectoryPointKind::Set && e.above_horizon)
        );

        let moon_traj =
            compute_solar_system_daily_trajectory(paris, midnight, SolarSystemBody::Moon, None, 97);
        assert!(moon_traj.samples.len() >= 97);
        assert!(
            moon_traj
                .events
                .iter()
                .any(|e| e.kind == TrajectoryPointKind::Transit)
        );
    }
}
