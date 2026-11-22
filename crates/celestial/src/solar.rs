use glam::DVec3;
use hifitime::Epoch;
use serde::{Deserialize, Serialize};

use crate::coords::{
    Angle, cartesian_to_canonical_spherical, geodetic::GeodeticPosition,
    topocentric_enu_to_spherical,
};
use crate::observer::ObservationContext;

/// Solar-system bodies rendered and tracked in the planetarium (distinct category from catalog stars).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SolarSystemBody {
    Sun,
    Moon,
}

impl SolarSystemBody {
    pub const ALL: [Self; 2] = [Self::Sun, Self::Moon];

    /// Primary display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sun => "Sun",
            Self::Moon => "Moon",
        }
    }

    /// Object category label distinguishing solar-system bodies from catalog stars.
    pub fn category_label(self) -> &'static str {
        "Solar System"
    }

    /// Short physical classification label for details UI.
    pub fn kind_label(self) -> &'static str {
        match self {
            Self::Sun => "G2V Star",
            Self::Moon => "Natural Satellite",
        }
    }

    /// Representative sRGB accent tint for reticles, trajectories, and graphs.
    pub fn srgb(self) -> [f32; 3] {
        match self {
            Self::Sun => [1.0, 0.84, 0.32],
            Self::Moon => [0.86, 0.90, 0.98],
        }
    }

    /// Computes full topocentric and equatorial ephemeris at `(location, epoch)`.
    pub fn ephemeris(self, location: GeodeticPosition, epoch: Epoch) -> SolarSystemEphemeris {
        solar_system_ephemeris(self, location, epoch)
    }
}

/// Illumination, phase angle, elongation, synodic age, and optical libration of the Moon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LunarPhaseInfo {
    /// Sun-Moon-Observer phase angle in degrees (`0°` = Full, `180°` = New).
    pub phase_angle_deg: f64,
    /// Ecliptic elongation `λ_Moon - λ_Sun` in `[0°, 360°)`.
    pub elongation_deg: f64,
    /// Illuminated disc fraction `k ∈ [0.0, 1.0]`.
    pub illuminated_fraction: f64,
    /// `true` when waxing (`elongation_deg < 180°`), `false` when waning.
    pub waxing: bool,
    /// Approximate lunar age in days within the 29.53059-day synodic month.
    pub age_days: f64,
    /// Optical libration in selenographic longitude `l' ∈ [-180°, 180°)` (sub-Earth longitude).
    pub libration_lon_deg: f64,
    /// Optical libration in selenographic latitude `b' ∈ [-90°, 90°]` (sub-Earth latitude).
    pub libration_lat_deg: f64,
    /// Unit ICRS direction of the Moon's North rotation axis from Cassini's laws.
    pub north_pole_icrs: DVec3,
}

impl LunarPhaseInfo {
    /// Human-readable 8-octant lunar phase name.
    pub fn phase_name(self) -> &'static str {
        let e = self.elongation_deg.rem_euclid(360.0);
        if !(22.5..337.5).contains(&e) {
            "New Moon"
        } else if e < 67.5 {
            "Waxing Crescent"
        } else if e < 112.5 {
            "First Quarter"
        } else if e < 157.5 {
            "Waxing Gibbous"
        } else if e < 202.5 {
            "Full Moon"
        } else if e < 247.5 {
            "Waning Gibbous"
        } else if e < 292.5 {
            "Last Quarter"
        } else {
            "Waning Crescent"
        }
    }
}

/// Complete topocentric and equatorial ephemeris state for a [`SolarSystemBody`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolarSystemEphemeris {
    pub body: SolarSystemBody,
    /// Unit topocentric direction rotated into ICRS (J2000) so camera projection matches stars.
    pub icrs_dir: DVec3,
    /// Unit topocentric ENU direction (`+X` East, `+Y` North, `+Z` Up).
    pub topo_enu: DVec3,
    /// Apparent topocentric ICRS Right Ascension in degrees `[0, 360)`.
    pub ra_deg: f64,
    /// Apparent topocentric ICRS Declination in degrees `[-90, 90]`.
    pub dec_deg: f64,
    /// Topocentric Azimuth in degrees `[0, 360)` (`0°` North, `90°` East).
    pub az_deg: f64,
    /// Topocentric geometric Altitude in degrees `[-90, 90]`.
    pub alt_deg: f64,
    /// Topocentric distance from observer in kilometers.
    pub distance_km: f64,
    /// Apparent angular semi-diameter (radius) in degrees.
    pub angular_radius_deg: f64,
    /// Apparent visual magnitude `V`.
    pub apparent_magnitude: f32,
    /// Unit ICRS direction toward the Sun (for 3D lunar phase lighting orientation).
    pub sun_icrs_dir: DVec3,
    /// Lunar phase metadata when `body == SolarSystemBody::Moon`.
    pub lunar_phase: Option<LunarPhaseInfo>,
}

impl SolarSystemEphemeris {
    /// Returns the primary display title and category/phase subtitle for hover and details UI.
    pub fn display_title_and_subtitle(&self) -> (String, String) {
        let title = self.body.name().to_string();
        let subtitle = match (self.body, self.lunar_phase) {
            (SolarSystemBody::Sun, _) => {
                format!("{} · {}", self.body.category_label(), self.body.kind_label())
            }
            (SolarSystemBody::Moon, Some(phase)) => {
                format!(
                    "{} · {} ({:.0}%)",
                    self.body.category_label(),
                    phase.phase_name(),
                    phase.illuminated_fraction * 100.0
                )
            }
            (SolarSystemBody::Moon, None) => {
                format!("{} · {}", self.body.category_label(), self.body.kind_label())
            }
        };
        (title, subtitle)
    }
}

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
    let (ra, dec, _, _) = solar_equatorial_and_distance_of_date(epoch);
    (ra, dec)
}

fn solar_equatorial_and_distance_of_date(epoch: Epoch) -> (Angle, Angle, f64, f64) {
    const AU_KM: f64 = 149_597_870.7;
    let n = epoch.to_jde_utc_days() - 2451545.0;
    let mean_lon_deg = (280.460 + 0.9856474 * n).rem_euclid(360.0);
    let mean_anom_rad = (357.528 + 0.9856003 * n).rem_euclid(360.0).to_radians();
    let ecliptic_lon_deg = (mean_lon_deg
        + 1.915 * mean_anom_rad.sin()
        + 0.020 * (2.0 * mean_anom_rad).sin())
    .rem_euclid(360.0);
    let ecliptic_lon_rad = ecliptic_lon_deg.to_radians();
    let obliquity_rad = (23.439 - 0.0000004 * n).to_radians();
    let dist_au = (1.00014 - 0.01671 * mean_anom_rad.cos() - 0.00014 * (2.0 * mean_anom_rad).cos())
        .max(0.98);

    let (sin_lam, cos_lam) = ecliptic_lon_rad.sin_cos();
    let (sin_eps, cos_eps) = obliquity_rad.sin_cos();

    let ra = Angle::from_radians_normalized((cos_eps * sin_lam).atan2(cos_lam));
    let dec = Angle::from_radians((sin_eps * sin_lam).clamp(-1.0, 1.0).asin());
    (ra, dec, dist_au * AU_KM, ecliptic_lon_deg)
}

/// Computes the Sun's unit direction vector in topocentric ENU coordinates
/// (`+X` East, `+Y` North, `+Z` Up).
pub fn solar_topocentric_enu(location: GeodeticPosition, epoch: Epoch) -> DVec3 {
    let lst = ObservationContext::new(location, epoch).local_sidereal_time();
    let (ra, dec) = solar_equatorial_of_date(epoch);
    equatorial_of_date_to_enu(location, lst, ra, dec)
}

fn equatorial_of_date_to_enu(
    location: GeodeticPosition,
    lst: Angle,
    ra: Angle,
    dec: Angle,
) -> DVec3 {
    let ha = lst - ra;

    let (sin_lat, cos_lat) = location.latitude.sin_cos();
    let (sin_dec, cos_dec) = dec.sin_cos();
    let (sin_ha, cos_ha) = ha.sin_cos();

    let east = -cos_dec * sin_ha;
    let north = cos_lat * sin_dec - sin_lat * cos_dec * cos_ha;
    let up = sin_lat * sin_dec + cos_lat * cos_dec * cos_ha;

    DVec3::new(east, north, up).normalize()
}

/// Computes the Moon's geocentric apparent equatorial coordinates (RA, Dec) of date,
/// geocentric distance in km, and ecliptic longitude/latitude in degrees using
/// Meeus Ch. 47 truncated ELP-2000/82 series.
#[allow(clippy::many_single_char_names)]
pub fn lunar_geocentric_of_date(epoch: Epoch) -> (Angle, Angle, f64, f64, f64) {
    let t = (epoch.to_jde_tt_days() - 2451545.0) / 36525.0;
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;

    let l_prime = (218.316_447_7 + 481_267.881_234_21 * t - 0.001_578_6 * t2 + t3 / 538_841.0
        - t4 / 65_194_000.0)
        .rem_euclid(360.0);
    let d = (297.850_192_1 + 445_267.111_403_4 * t - 0.001_881_9 * t2 + t3 / 545_868.0
        - t4 / 113_065_000.0)
        .rem_euclid(360.0);
    let m = (357.529_109_2 + 35_999.050_290_9 * t - 0.000_153_6 * t2 + t3 / 24_490_000.0)
        .rem_euclid(360.0);
    let m_prime = (134.963_396_4 + 477_198.867_505_5 * t + 0.008_741_4 * t2 + t3 / 69_699.0
        - t4 / 14_712_000.0)
        .rem_euclid(360.0);
    let f = (93.272_095_0 + 483_202.017_523_3 * t - 0.003_653_9 * t2 - t3 / 3_526_000.0
        + t4 / 863_310_000.0)
        .rem_euclid(360.0);

    let d_rad = d.to_radians();
    let m_rad = m.to_radians();
    let mp_rad = m_prime.to_radians();
    let f_rad = f.to_radians();

    let sum_l_deg = 6.288_774 * mp_rad.sin()
        + 1.274_027 * (2.0 * d_rad - mp_rad).sin()
        + 0.658_314 * (2.0 * d_rad).sin()
        + 0.213_618 * (2.0 * mp_rad).sin()
        - 0.185_116 * m_rad.sin()
        - 0.114_332 * (2.0 * f_rad).sin()
        + 0.058_793 * (2.0 * d_rad - 2.0 * mp_rad).sin()
        + 0.057_066 * (2.0 * d_rad - m_rad - mp_rad).sin()
        + 0.053_322 * (2.0 * d_rad + mp_rad).sin()
        + 0.045_758 * (2.0 * d_rad - m_rad).sin()
        - 0.040_923 * (m_rad - mp_rad).sin()
        - 0.034_720 * d_rad.sin()
        - 0.030_383 * (m_rad + mp_rad).sin()
        + 0.015_327 * (2.0 * d_rad - 2.0 * f_rad).sin()
        - 0.012_528 * (mp_rad + 2.0 * f_rad).sin()
        + 0.010_980 * (mp_rad - 2.0 * f_rad).sin();

    let sum_b_deg = 5.128_122 * f_rad.sin()
        + 0.280_602 * (mp_rad + f_rad).sin()
        + 0.277_693 * (mp_rad - f_rad).sin()
        + 0.173_237 * (2.0 * d_rad - f_rad).sin()
        + 0.055_413 * (2.0 * d_rad - mp_rad + f_rad).sin()
        + 0.046_271 * (2.0 * d_rad - mp_rad - f_rad).sin()
        + 0.032_573 * (2.0 * d_rad + f_rad).sin()
        + 0.017_198 * (2.0 * mp_rad + f_rad).sin()
        + 0.009_266 * (2.0 * d_rad + mp_rad - f_rad).sin()
        + 0.008_822 * (2.0 * mp_rad - f_rad).sin();

    let dist_km = (385_000.56 - 20_905.355 * mp_rad.cos()
        - 3_699.111 * (2.0 * d_rad - mp_rad).cos()
        - 2_955.968 * (2.0 * d_rad).cos()
        - 569.925 * (2.0 * mp_rad).cos()
        + 246.158 * (2.0 * mp_rad - 2.0 * d_rad).cos()
        - 204.586 * (2.0 * d_rad - m_rad).cos()
        - 170.733 * (2.0 * d_rad + mp_rad).cos()
        - 152.138 * (2.0 * d_rad - m_rad - mp_rad).cos()
        - 129.620 * (mp_rad - m_rad).cos()
        + 108.743 * d_rad.cos()
        + 104.755 * (m_rad + mp_rad).cos()
        + 79.661 * (mp_rad - 2.0 * f_rad).cos()
        + 48.888 * m_rad.cos())
    .clamp(350_000.0, 415_000.0);

    let lon_deg = (l_prime + sum_l_deg).rem_euclid(360.0);
    let lat_deg = sum_b_deg.clamp(-89.9, 89.9);
    let lon_rad = lon_deg.to_radians();
    let lat_rad = lat_deg.to_radians();
    let eps_rad = (23.439_291_1 - 0.013_004_2 * t).to_radians();

    let (sin_lon, cos_lon) = lon_rad.sin_cos();
    let (sin_lat, cos_lat) = lat_rad.sin_cos();
    let (sin_eps, cos_eps) = eps_rad.sin_cos();

    let x = cos_lat * cos_lon;
    let y = cos_eps * cos_lat * sin_lon - sin_eps * sin_lat;
    let z = sin_eps * cos_lat * sin_lon + cos_eps * sin_lat;

    let ra = Angle::from_radians_normalized(y.atan2(x));
    let dec = Angle::from_radians(z.clamp(-1.0, 1.0).asin());
    (ra, dec, dist_km, lon_deg, lat_deg)
}

/// Computes the Moon's topocentric equatorial coordinates of date `(RA, Dec)` and
/// topocentric distance in km, corrected for the observer's geodetic parallax.
pub fn lunar_topocentric_of_date(
    location: GeodeticPosition,
    epoch: Epoch,
) -> (Angle, Angle, f64, f64, f64) {
    const EARTH_REQ_KM: f64 = 6_378.137;
    const EARTH_FLATTENING: f64 = 1.0 / 298.257_223_563;

    let (ra_geo, dec_geo, dist_geo_km, lon_deg, lat_deg) = lunar_geocentric_of_date(epoch);
    let (sin_ra, cos_ra) = ra_geo.sin_cos();
    let (sin_dec, cos_dec) = dec_geo.sin_cos();
    let r_moon_eq = DVec3::new(cos_dec * cos_ra, cos_dec * sin_ra, sin_dec) * dist_geo_km;

    let lat_rad = location.latitude.radians();
    let (sin_lat, cos_lat) = lat_rad.sin_cos();
    let u = ((1.0 - EARTH_FLATTENING) * lat_rad.tan()).atan();
    let (sin_u, cos_u) = u.sin_cos();
    let elev_ratio = (location.elevation_m() * 1e-3) / EARTH_REQ_KM;
    let rho_sin_phi = (1.0 - EARTH_FLATTENING) * sin_u + elev_ratio * sin_lat;
    let rho_cos_phi = cos_u + elev_ratio * cos_lat;

    let lst = ObservationContext::new(location, epoch).local_sidereal_time();
    let (sin_lst, cos_lst) = lst.sin_cos();
    let r_obs_eq = DVec3::new(
        rho_cos_phi * cos_lst,
        rho_cos_phi * sin_lst,
        rho_sin_phi,
    ) * EARTH_REQ_KM;

    let topo_vec = r_moon_eq - r_obs_eq;
    let topo_dist_km = topo_vec.length().max(1.0);
    let topo_unit = topo_vec / topo_dist_km;

    let ra_topo = Angle::from_radians_normalized(topo_unit.y.atan2(topo_unit.x));
    let dec_topo = Angle::from_radians(topo_unit.z.clamp(-1.0, 1.0).asin());
    (ra_topo, dec_topo, topo_dist_km, lon_deg, lat_deg)
}

/// Computes full topocentric and ICRS ephemeris for the Sun at `(location, epoch)`.
pub fn solar_ephemeris(location: GeodeticPosition, epoch: Epoch) -> SolarSystemEphemeris {
    const SOLAR_RADIUS_KM: f64 = 695_700.0;
    const AU_KM: f64 = 149_597_870.7;

    let obs_ctx = ObservationContext::new(location, epoch);
    let lst = obs_ctx.local_sidereal_time();
    let (ra_date, dec_date, dist_km, _) = solar_equatorial_and_distance_of_date(epoch);
    let topo_enu = equatorial_of_date_to_enu(location, lst, ra_date, dec_date);
    let icrs_dir = obs_ctx.topocentric_to_equatorial_dir(topo_enu).normalize_or_zero();
    let (ra_icrs, dec_icrs) = cartesian_to_canonical_spherical(icrs_dir);
    let (az, alt) = topocentric_enu_to_spherical(topo_enu);

    let angular_radius_deg = (SOLAR_RADIUS_KM / dist_km).clamp(0.0, 1.0).asin().to_degrees();
    let dist_au = (dist_km / AU_KM).max(0.1);
    let apparent_magnitude = (-26.74 + 5.0 * dist_au.log10()) as f32;

    SolarSystemEphemeris {
        body: SolarSystemBody::Sun,
        icrs_dir,
        topo_enu,
        ra_deg: ra_icrs.degrees(),
        dec_deg: dec_icrs.degrees(),
        az_deg: az.degrees(),
        alt_deg: alt.degrees(),
        distance_km: dist_km,
        angular_radius_deg,
        apparent_magnitude,
        sun_icrs_dir: icrs_dir,
        lunar_phase: None,
    }
}

/// Computes full topocentric and ICRS ephemeris for the Moon at `(location, epoch)`,
/// including topocentric parallax, angular radius, apparent magnitude, and lunar phase.
pub fn lunar_ephemeris(location: GeodeticPosition, epoch: Epoch) -> SolarSystemEphemeris {
    const LUNAR_RADIUS_KM: f64 = 1_737.4;
    const MEAN_LUNAR_DIST_KM: f64 = 384_400.0;
    const SYNODIC_MONTH_DAYS: f64 = 29.530_588_853;

    let obs_ctx = ObservationContext::new(location, epoch);
    let lst = obs_ctx.local_sidereal_time();
    let (sun_ra_date, sun_dec_date, _, sun_lon_deg) = solar_equatorial_and_distance_of_date(epoch);
    let sun_enu = equatorial_of_date_to_enu(location, lst, sun_ra_date, sun_dec_date);
    let sun_icrs_dir = obs_ctx.topocentric_to_equatorial_dir(sun_enu).normalize_or_zero();

    let (ra_date, dec_date, topo_dist_km, moon_lon_deg, _) =
        lunar_topocentric_of_date(location, epoch);
    let topo_enu = equatorial_of_date_to_enu(location, lst, ra_date, dec_date);
    let icrs_dir = obs_ctx.topocentric_to_equatorial_dir(topo_enu).normalize_or_zero();
    let (ra_icrs, dec_icrs) = cartesian_to_canonical_spherical(icrs_dir);
    let (az, alt) = topocentric_enu_to_spherical(topo_enu);

    let angular_radius_deg = (LUNAR_RADIUS_KM / topo_dist_km)
        .clamp(0.0, 1.0)
        .asin()
        .to_degrees();

    // Elongation psi is the angular separation between Sun and Moon as seen by the observer;
    // phase angle i ≈ 180° - psi (with ~0.15° finite-distance correction).
    let cos_psi = topo_enu.dot(sun_enu).clamp(-1.0, 1.0);
    let psi_rad = cos_psi.acos();
    let phase_angle_rad = (std::f64::consts::PI - psi_rad).clamp(0.0, std::f64::consts::PI);
    let phase_angle_deg = phase_angle_rad.to_degrees();
    let illuminated_fraction = f64::midpoint(1.0, phase_angle_rad.cos()).clamp(0.0, 1.0);

    let elongation_deg = (moon_lon_deg - sun_lon_deg).rem_euclid(360.0);
    let waxing = elongation_deg < 180.0;
    let age_days = (elongation_deg / 360.0) * SYNODIC_MONTH_DAYS;

    let alpha = phase_angle_deg;
    let mag = -12.73
        + 0.026 * alpha
        + 4.0e-9 * alpha.powi(4)
        + 5.0 * (topo_dist_km / MEAN_LUNAR_DIST_KM).max(0.1).log10();

    // Meeus Ch. 53: optical libration (l', b') and Cassini-law lunar North rotation axis
    let t = (epoch.to_jde_tt_days() - 2451545.0) / 36525.0;
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let f_deg = (93.272_095_0 + 483_202.017_523_3 * t - 0.003_653_9 * t2 - t3 / 3_526_000.0
        + t4 / 863_310_000.0)
        .rem_euclid(360.0);
    let omega_deg = (125.044_547_9 - 1_934.136_289_1 * t + 0.002_075_4 * t2 + t3 / 467_441.0
        - t4 / 60_616_000.0)
        .rem_euclid(360.0);
    let eps_rad = (23.439_291_1 - 0.013_004_2 * t).to_radians();
    let (sin_eps, cos_eps) = eps_rad.sin_cos();

    let (sin_ra_t, cos_ra_t) = ra_date.sin_cos();
    let (sin_dec_t, cos_dec_t) = dec_date.sin_cos();
    let ecl_x = cos_dec_t * cos_ra_t;
    let ecl_y = cos_dec_t * sin_ra_t * cos_eps + sin_dec_t * sin_eps;
    let ecl_z = -cos_dec_t * sin_ra_t * sin_eps + sin_dec_t * cos_eps;
    let lambda_topo_rad = ecl_y.atan2(ecl_x);
    let beta_topo_rad = ecl_z.clamp(-1.0, 1.0).asin();

    const LUNAR_INCLINATION_DEG: f64 = 1.542_42;
    let (sin_i, cos_i) = LUNAR_INCLINATION_DEG.to_radians().sin_cos();
    let omega_rad = omega_deg.to_radians();
    let w_rad = lambda_topo_rad - omega_rad;
    let (sin_w, cos_w) = w_rad.sin_cos();
    let (sin_beta, cos_beta) = beta_topo_rad.sin_cos();

    let a_rad = (sin_w * cos_beta * cos_i - sin_beta * sin_i).atan2(cos_w * cos_beta);
    let mut libration_lon_deg = (a_rad.to_degrees() - f_deg).rem_euclid(360.0);
    if libration_lon_deg >= 180.0 {
        libration_lon_deg -= 360.0;
    }
    let libration_lat_deg = (-sin_w * cos_beta * sin_i - sin_beta * cos_i)
        .clamp(-1.0, 1.0)
        .asin()
        .to_degrees();

    // Cassini's 3rd law: Moon North pole at ecliptic (lon = Ω + 90°, lat = 90° - I)
    let (sin_om, cos_om) = omega_rad.sin_cos();
    let pole_ecl_x = -sin_i * sin_om;
    let pole_ecl_y = sin_i * cos_om;
    let pole_ecl_z = cos_i;
    let pole_eq_x = pole_ecl_x;
    let pole_eq_y = cos_eps * pole_ecl_y - sin_eps * pole_ecl_z;
    let pole_eq_z = sin_eps * pole_ecl_y + cos_eps * pole_ecl_z;
    let pole_ra_date = Angle::from_radians_normalized(pole_eq_y.atan2(pole_eq_x));
    let pole_dec_date = Angle::from_radians(pole_eq_z.clamp(-1.0, 1.0).asin());
    let pole_enu = equatorial_of_date_to_enu(location, lst, pole_ra_date, pole_dec_date);
    let north_pole_icrs = obs_ctx
        .topocentric_to_equatorial_dir(pole_enu)
        .normalize_or_zero();

    let lunar_phase = LunarPhaseInfo {
        phase_angle_deg,
        elongation_deg,
        illuminated_fraction,
        waxing,
        age_days,
        libration_lon_deg,
        libration_lat_deg,
        north_pole_icrs,
    };

    SolarSystemEphemeris {
        body: SolarSystemBody::Moon,
        icrs_dir,
        topo_enu,
        ra_deg: ra_icrs.degrees(),
        dec_deg: dec_icrs.degrees(),
        az_deg: az.degrees(),
        alt_deg: alt.degrees(),
        distance_km: topo_dist_km,
        angular_radius_deg,
        apparent_magnitude: mag.clamp(-13.0, -1.5) as f32,
        sun_icrs_dir,
        lunar_phase: Some(lunar_phase),
    }
}

/// Computes full topocentric and ICRS ephemeris for `body` at `(location, epoch)`.
pub fn solar_system_ephemeris(
    body: SolarSystemBody,
    location: GeodeticPosition,
    epoch: Epoch,
) -> SolarSystemEphemeris {
    match body {
        SolarSystemBody::Sun => solar_ephemeris(location, epoch),
        SolarSystemBody::Moon => lunar_ephemeris(location, epoch),
    }
}

/// Computes the Sun's geometric altitude in degrees above the observer's horizon.
pub fn solar_altitude_deg(location: GeodeticPosition, epoch: Epoch) -> f64 {
    solar_topocentric_enu(location, epoch)
        .z
        .clamp(-1.0, 1.0)
        .asin()
        .to_degrees()
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

    #[test]
    fn solar_topocentric_enu_unit_and_equinox_cardinals() {
        let mid_lat = GeodeticPosition::from_degrees(45.0, 0.0, 0.0);

        let noon = Epoch::from_gregorian_utc(2026, 3, 20, 12, 7, 0, 0);
        let enu_noon = solar_topocentric_enu(mid_lat, noon);
        assert!((enu_noon.length() - 1.0).abs() < 1e-12);
        assert!(enu_noon.x.abs() < 0.05, "expected due south at noon, x={}", enu_noon.x);
        assert!(enu_noon.y < -0.65, "expected south (-Y) at noon, y={}", enu_noon.y);
        assert!(enu_noon.z > 0.65, "expected ~45° altitude at noon, z={}", enu_noon.z);
        assert!((enu_noon.z.asin().to_degrees() - solar_altitude_deg(mid_lat, noon)).abs() < 1e-10);

        let sunrise = Epoch::from_gregorian_utc(2026, 3, 20, 6, 7, 0, 0);
        let enu_sunrise = solar_topocentric_enu(mid_lat, sunrise);
        assert!((enu_sunrise.length() - 1.0).abs() < 1e-12);
        assert!(enu_sunrise.x > 0.95, "expected east (+X) at sunrise, x={}", enu_sunrise.x);
        assert!(enu_sunrise.z.abs() < 0.08, "expected near horizon at sunrise, z={}", enu_sunrise.z);

        let sunset = Epoch::from_gregorian_utc(2026, 3, 20, 18, 7, 0, 0);
        let enu_sunset = solar_topocentric_enu(mid_lat, sunset);
        assert!((enu_sunset.length() - 1.0).abs() < 1e-12);
        assert!(enu_sunset.x < -0.95, "expected west (-X) at sunset, x={}", enu_sunset.x);
        assert!(enu_sunset.z.abs() < 0.08, "expected near horizon at sunset, z={}", enu_sunset.z);
    }

    #[test]
    fn solar_and_lunar_ephemeris_consistency_and_lunar_phases() {
        let paris = GeodeticPosition::default();
        let epoch = Epoch::from_gregorian_utc(2026, 3, 20, 12, 0, 0, 0);
        let obs_ctx = ObservationContext::new(paris, epoch);

        for body in SolarSystemBody::ALL {
            let eph = body.ephemeris(paris, epoch);
            assert_eq!(eph.body, body);
            assert!((eph.icrs_dir.length() - 1.0).abs() < 1e-12);
            assert!((eph.topo_enu.length() - 1.0).abs() < 1e-12);
            // Transforming icrs_dir through ObservationContext must reconstruct topo_enu.
            let roundtrip_enu = obs_ctx.equatorial_to_topocentric_dir(eph.icrs_dir);
            assert!(
                roundtrip_enu.distance(eph.topo_enu) < 1e-12,
                "ICRS <-> ENU mismatch for {body:?}: {roundtrip_enu:?} vs {:?}",
                eph.topo_enu
            );
            assert!(eph.angular_radius_deg > 0.22 && eph.angular_radius_deg < 0.30);
        }

        // Check lunar phase progression over a synodic month (~29.53 days)
        let base_epoch = Epoch::from_gregorian_utc(2026, 1, 18, 19, 52, 0, 0); // near New Moon
        let mut saw_crescent = false;
        let mut saw_full = false;
        let mut min_illum = 1.0_f64;
        let mut max_illum = 0.0_f64;

        for day in 0..30 {
            let ep = Epoch::from_unix_seconds(base_epoch.to_unix_seconds() + f64::from(day) * 86_400.0);
            let moon = lunar_ephemeris(paris, ep);
            let phase = moon.lunar_phase.expect("Moon has phase");
            min_illum = min_illum.min(phase.illuminated_fraction);
            max_illum = max_illum.max(phase.illuminated_fraction);
            if phase.phase_name() == "Waxing Crescent" {
                saw_crescent = true;
            }
            if phase.phase_name() == "Full Moon" {
                saw_full = true;
            }
            assert!(moon.distance_km > 350_000.0 && moon.distance_km < 415_000.0);
        }

        assert!(min_illum < 0.05, "expected near-0 illumination at New Moon, got {min_illum}");
        assert!(max_illum > 0.95, "expected near-1 illumination at Full Moon, got {max_illum}");
        assert!(saw_crescent);
        assert!(saw_full);
    }

    #[test]
    fn lunar_libration_and_north_pole_within_physical_bounds() {
        let paris = GeodeticPosition::default();
        let base = Epoch::from_gregorian_utc(2026, 1, 1, 0, 0, 0, 0);
        let mut max_abs_lon = 0.0_f64;
        let mut max_abs_lat = 0.0_f64;

        for day in 0..30 {
            let ep = Epoch::from_unix_seconds(base.to_unix_seconds() + f64::from(day) * 86_400.0);
            let moon = lunar_ephemeris(paris, ep);
            let phase = moon.lunar_phase.expect("Moon has phase");
            assert!(
                phase.libration_lon_deg.abs() < 10.0,
                "libration_lon_deg out of range: {}",
                phase.libration_lon_deg
            );
            assert!(
                phase.libration_lat_deg.abs() < 8.0,
                "libration_lat_deg out of range: {}",
                phase.libration_lat_deg
            );
            assert!((phase.north_pole_icrs.length() - 1.0).abs() < 1e-12);
            // Moon's axis is within ~25° of celestial North (+Z ICRS), so dot > 0.88.
            assert!(
                phase.north_pole_icrs.z > 0.88,
                "unexpected lunar north_pole_icrs.z: {}",
                phase.north_pole_icrs.z
            );
            max_abs_lon = max_abs_lon.max(phase.libration_lon_deg.abs());
            max_abs_lat = max_abs_lat.max(phase.libration_lat_deg.abs());
        }

        assert!(max_abs_lon > 1.0);
        assert!(max_abs_lat > 1.0);
    }
}
