use hifitime::Epoch;

use crate::coords::{Angle, geodetic::GeodeticPosition};
use crate::observer::ObservationContext;

/// Twilight and daylight classification based on solar altitude above the horizon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyPhase {
    Day,
    CivilTwilight,
    NauticalTwilight,
    AstronomicalTwilight,
    AstronomicalNight,
}

impl SkyPhase {
    /// Classifies sky darkness from solar geometric altitude in degrees.
    pub fn from_solar_altitude_deg(alt_deg: f64) -> Self {
        if alt_deg > 0.0 {
            Self::Day
        } else if alt_deg > -6.0 {
            Self::CivilTwilight
        } else if alt_deg > -12.0 {
            Self::NauticalTwilight
        } else if alt_deg > -18.0 {
            Self::AstronomicalTwilight
        } else {
            Self::AstronomicalNight
        }
    }

    /// Short human-readable description for UI badges.
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Daylight",
            Self::CivilTwilight => "Civil Twilight",
            Self::NauticalTwilight => "Nautical Twilight",
            Self::AstronomicalTwilight => "Astro Twilight",
            Self::AstronomicalNight => "Astro Night",
        }
    }
}

/// Computes the Sun's apparent equatorial coordinates (RA, Dec) of date using the
/// Astronomical Almanac low-precision analytical series (~0.01° accuracy).
pub fn solar_equatorial_of_date(epoch: Epoch) -> (Angle, Angle) {
    let n = epoch.to_jde_utc_days() - 2451545.0;
    let mean_lon_deg = (280.460 + 0.9856474 * n).rem_euclid(360.0);
    let mean_anom_rad = (357.528 + 0.9856003 * n).rem_euclid(360.0).to_radians();
    let ecliptic_lon_rad = (mean_lon_deg
        + 1.915 * mean_anom_rad.sin()
        + 0.020 * (2.0 * mean_anom_rad).sin())
    .to_radians();
    let obliquity_rad = (23.439 - 0.0000004 * n).to_radians();

    let (sin_lam, cos_lam) = ecliptic_lon_rad.sin_cos();
    let (sin_eps, cos_eps) = obliquity_rad.sin_cos();

    let ra = Angle::from_radians_normalized((cos_eps * sin_lam).atan2(cos_lam));
    let dec = Angle::from_radians((sin_eps * sin_lam).clamp(-1.0, 1.0).asin());
    (ra, dec)
}

/// Computes the Sun's geometric altitude in degrees above the observer's horizon.
pub fn solar_altitude_deg(location: GeodeticPosition, epoch: Epoch) -> f64 {
    let lst = ObservationContext::new(location, epoch).local_sidereal_time();
    let (ra, dec) = solar_equatorial_of_date(epoch);
    let hour_angle = lst - ra;

    let (sin_lat, cos_lat) = location.latitude.sin_cos();
    let (sin_dec, cos_dec) = dec.sin_cos();
    let sin_alt = (sin_lat * sin_dec + cos_lat * cos_dec * hour_angle.cos()).clamp(-1.0, 1.0);
    sin_alt.asin().to_degrees()
}

/// Samples solar altitude (in degrees) evenly over a 24-hour local day `[0h..24h]`.
pub fn sample_day_solar_profile(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    samples: usize,
) -> Vec<f64> {
    let count = samples.max(2);
    let denom = (count - 1) as f64;
    (0..count)
        .map(|i| {
            let t_secs = local_midnight_unix_secs + (i as f64 / denom) * 86_400.0;
            solar_altitude_deg(location, Epoch::from_unix_seconds(t_secs))
        })
        .collect()
}

/// Key solar transition hours `[0.0, 24.0)` across a local calendar day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailySolarEvents {
    pub sunrise_hour: Option<f64>,
    pub sunset_hour: Option<f64>,
    pub astro_dusk_hour: Option<f64>,
    pub astro_dawn_hour: Option<f64>,
    pub solar_noon_hour: f64,
    pub solar_midnight_hour: f64,
}

fn refine_crossing(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
    mut h_lo: f64,
    mut h_hi: f64,
    threshold_deg: f64,
    rising: bool,
) -> f64 {
    for _ in 0..8 {
        let mid = f64::midpoint(h_lo, h_hi);
        let alt = solar_altitude_deg(
            location,
            Epoch::from_unix_seconds(local_midnight_unix_secs + mid * 3600.0),
        );
        let above = alt >= threshold_deg;
        if (rising && above) || (!rising && !above) {
            h_hi = mid;
        } else {
            h_lo = mid;
        }
    }
    f64::midpoint(h_lo, h_hi)
}

/// Finds sunrise, sunset, astronomical dusk/dawn, and solar noon/midnight hours
/// over the 24-hour interval starting at `local_midnight_unix_secs`.
pub fn find_sky_events(
    location: GeodeticPosition,
    local_midnight_unix_secs: f64,
) -> DailySolarEvents {
    const STEPS: usize = 96;
    const SUNRISE_SUNSET_ALT_DEG: f64 = -0.833;
    const ASTRO_TWILIGHT_ALT_DEG: f64 = -18.0;

    let profile = sample_day_solar_profile(location, local_midnight_unix_secs, STEPS + 1);
    let step_hours = 24.0 / STEPS as f64;

    let mut sunrise_hour = None;
    let mut sunset_hour = None;
    let mut astro_dusk_hour = None;
    let mut astro_dawn_hour = None;
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

        if sunrise_hour.is_none()
            && prev < SUNRISE_SUNSET_ALT_DEG
            && curr >= SUNRISE_SUNSET_ALT_DEG
        {
            sunrise_hour = Some(refine_crossing(
                location,
                local_midnight_unix_secs,
                h0,
                h1,
                SUNRISE_SUNSET_ALT_DEG,
                true,
            ));
        }
        if sunset_hour.is_none()
            && prev >= SUNRISE_SUNSET_ALT_DEG
            && curr < SUNRISE_SUNSET_ALT_DEG
        {
            sunset_hour = Some(refine_crossing(
                location,
                local_midnight_unix_secs,
                h0,
                h1,
                SUNRISE_SUNSET_ALT_DEG,
                false,
            ));
        }
        if astro_dusk_hour.is_none()
            && prev >= ASTRO_TWILIGHT_ALT_DEG
            && curr < ASTRO_TWILIGHT_ALT_DEG
        {
            astro_dusk_hour = Some(refine_crossing(
                location,
                local_midnight_unix_secs,
                h0,
                h1,
                ASTRO_TWILIGHT_ALT_DEG,
                false,
            ));
        }
        if astro_dawn_hour.is_none()
            && prev < ASTRO_TWILIGHT_ALT_DEG
            && curr >= ASTRO_TWILIGHT_ALT_DEG
        {
            astro_dawn_hour = Some(refine_crossing(
                location,
                local_midnight_unix_secs,
                h0,
                h1,
                ASTRO_TWILIGHT_ALT_DEG,
                true,
            ));
        }
    }

    DailySolarEvents {
        sunrise_hour,
        sunset_hour,
        astro_dusk_hour,
        astro_dawn_hour,
        solar_noon_hour: max_idx as f64 * step_hours,
        solar_midnight_hour: min_idx as f64 * step_hours,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vernal_equinox_equator_noon_is_near_zenith() {
        // 2026-03-20 12:07 UTC is roughly solar noon at 0° lon on equinox day.
        let equator = GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        let epoch = Epoch::from_gregorian_utc(2026, 3, 20, 12, 7, 0, 0);
        let alt = solar_altitude_deg(equator, epoch);
        assert!(alt > 87.0, "expected near-zenith solar altitude, got {alt}");
        assert_eq!(SkyPhase::from_solar_altitude_deg(alt), SkyPhase::Day);
    }

    #[test]
    fn paris_solstice_twilight_vs_winter_astro_night() {
        let paris = GeodeticPosition::default();

        // Near June solstice (2026-06-21 00:00 UTC): Paris (~48.8°N) never drops below -18°.
        let june_midnight = Epoch::from_gregorian_utc(2026, 6, 21, 0, 0, 0, 0).to_unix_seconds();
        let june_events = find_sky_events(paris, june_midnight);
        assert!(june_events.sunrise_hour.is_some());
        assert!(june_events.sunset_hour.is_some());
        assert!(june_events.astro_dusk_hour.is_none());
        assert!(june_events.astro_dawn_hour.is_none());

        // Near December solstice (2026-12-21 00:00 UTC): Paris has clear astro dusk and dawn.
        let dec_midnight = Epoch::from_gregorian_utc(2026, 12, 21, 0, 0, 0, 0).to_unix_seconds();
        let dec_events = find_sky_events(paris, dec_midnight);
        assert!(dec_events.astro_dusk_hour.is_some());
        assert!(dec_events.astro_dawn_hour.is_some());
        assert!(dec_events.sunset_hour.unwrap() < dec_events.astro_dusk_hour.unwrap());
    }

    #[test]
    fn north_pole_polar_day_and_polar_night() {
        let pole = GeodeticPosition::from_degrees(89.9, 0.0, 0.0);

        let june_noon = Epoch::from_gregorian_utc(2026, 6, 21, 12, 0, 0, 0);
        assert_eq!(
            SkyPhase::from_solar_altitude_deg(solar_altitude_deg(pole, june_noon)),
            SkyPhase::Day
        );

        let dec_noon = Epoch::from_gregorian_utc(2026, 12, 21, 12, 0, 0, 0);
        assert_eq!(
            SkyPhase::from_solar_altitude_deg(solar_altitude_deg(pole, dec_noon)),
            SkyPhase::AstronomicalNight
        );
    }
}
