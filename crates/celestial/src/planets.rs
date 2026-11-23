//! Low-precision ephemerides for the eight planets and Jupiter's Galilean satellites.
//!
//! Planet positions use the JPL "Keplerian elements for approximate positions of the major
//! planets" (valid 1800 AD - 2050 AD, arc-minute class accuracy), corrected for light-time and
//! diurnal parallax, and rotated into the same ICRS/ENU conventions as the Sun and Moon.
//!
//! The Galilean satellites are placed on circular orbits in Jupiter's equatorial plane, using the
//! mean synodic longitudes of Meeus, *Astronomical Algorithms* ch. 44 (low-accuracy method).

use std::f64::consts::{PI, TAU};

use glam::{DMat3, DVec3};
use hifitime::Epoch;

use crate::coords::{
    cartesian_to_canonical_spherical, geodetic::GeodeticPosition, topocentric_enu_to_spherical,
};
use crate::observer::ObservationContext;
use crate::solar::{SolarSystemBody, SolarSystemEphemeris, lunar_geocentric_of_date};

const AU_KM: f64 = 149_597_870.7;
/// Light travel time for one astronomical unit, in days (499.004784 s).
const LIGHT_DAYS_PER_AU: f64 = 499.004_784 / 86_400.0;
const J2000_OBLIQUITY_DEG: f64 = 23.439_291_1;
const EARTH_MOON_MASS_RATIO: f64 = 81.300_57;
const EARTH_REQ_KM: f64 = 6_378.137;
const EARTH_FLATTENING: f64 = 1.0 / 298.257_223_563;

/// Mean Keplerian elements `[value at J2000, rate per Julian century]`.
struct Elements {
    a_au: [f64; 2],
    e: [f64; 2],
    incl_deg: [f64; 2],
    mean_lon_deg: [f64; 2],
    peri_lon_deg: [f64; 2],
    node_deg: [f64; 2],
}

impl Elements {
    fn at(pair: [f64; 2], t: f64) -> f64 {
        pair[0] + pair[1] * t
    }
}

const MERCURY: Elements = Elements {
    a_au: [0.387_099_27, 0.000_000_37],
    e: [0.205_635_93, 0.000_019_06],
    incl_deg: [7.004_979_02, -0.005_947_49],
    mean_lon_deg: [252.250_323_50, 149_472.674_111_75],
    peri_lon_deg: [77.457_796_28, 0.160_476_89],
    node_deg: [48.330_765_93, -0.125_340_81],
};
const VENUS: Elements = Elements {
    a_au: [0.723_335_66, 0.000_003_90],
    e: [0.006_776_72, -0.000_041_07],
    incl_deg: [3.394_676_05, -0.000_788_90],
    mean_lon_deg: [181.979_099_50, 58_517.815_387_29],
    peri_lon_deg: [131.602_467_18, 0.002_683_29],
    node_deg: [76.679_842_55, -0.277_694_18],
};
const EARTH_MOON_BARYCENTER: Elements = Elements {
    a_au: [1.000_002_61, 0.000_005_62],
    e: [0.016_711_23, -0.000_043_92],
    incl_deg: [-0.000_015_31, -0.012_946_68],
    mean_lon_deg: [100.464_571_66, 35_999.372_449_81],
    peri_lon_deg: [102.937_681_93, 0.323_273_64],
    node_deg: [0.0, 0.0],
};
const MARS: Elements = Elements {
    a_au: [1.523_710_34, 0.000_018_47],
    e: [0.093_394_10, 0.000_078_82],
    incl_deg: [1.849_691_42, -0.008_131_31],
    mean_lon_deg: [-4.553_432_05, 19_140.302_684_99],
    peri_lon_deg: [-23.943_629_59, 0.444_410_88],
    node_deg: [49.559_538_91, -0.292_573_43],
};
const JUPITER: Elements = Elements {
    a_au: [5.202_887_00, -0.000_116_07],
    e: [0.048_386_24, -0.000_132_53],
    incl_deg: [1.304_396_95, -0.001_837_14],
    mean_lon_deg: [34.396_440_51, 3_034.746_127_75],
    peri_lon_deg: [14.728_479_83, 0.212_526_68],
    node_deg: [100.473_909_09, 0.204_691_06],
};
const SATURN: Elements = Elements {
    a_au: [9.536_675_94, -0.001_250_60],
    e: [0.053_861_79, -0.000_509_91],
    incl_deg: [2.485_991_87, 0.001_936_09],
    mean_lon_deg: [49.954_244_23, 1_222.493_622_01],
    peri_lon_deg: [92.598_878_31, -0.418_972_16],
    node_deg: [113.662_424_48, -0.288_677_94],
};
const URANUS: Elements = Elements {
    a_au: [19.189_164_64, -0.001_961_76],
    e: [0.047_257_44, -0.000_043_97],
    incl_deg: [0.772_637_83, -0.002_429_39],
    mean_lon_deg: [313.238_104_51, 428.482_027_85],
    peri_lon_deg: [170.954_276_30, 0.408_052_81],
    node_deg: [74.016_925_03, 0.042_405_89],
};
const NEPTUNE: Elements = Elements {
    a_au: [30.069_922_76, 0.000_262_91],
    e: [0.008_590_48, 0.000_051_05],
    incl_deg: [1.770_043_47, 0.000_353_72],
    mean_lon_deg: [-55.120_029_69, 218.459_453_25],
    peri_lon_deg: [44.964_762_27, -0.322_414_64],
    node_deg: [131.784_225_74, -0.005_086_64],
};

/// Physical and orbital description of a Galilean satellite.
struct GalileanSatellite {
    /// Mean synodic longitude at J2000 and its rate in degrees per day (Meeus ch. 44).
    u0_deg: f64,
    u_rate_deg_per_day: f64,
    orbit_radius_km: f64,
    /// Mean opposition visual magnitude.
    v_opposition: f64,
}

const IO: GalileanSatellite = GalileanSatellite {
    u0_deg: 163.806_9,
    u_rate_deg_per_day: 203.405_864_6,
    orbit_radius_km: 421_800.0,
    v_opposition: 5.0,
};
const EUROPA: GalileanSatellite = GalileanSatellite {
    u0_deg: 358.414_0,
    u_rate_deg_per_day: 101.291_633_5,
    orbit_radius_km: 671_100.0,
    v_opposition: 5.3,
};
const GANYMEDE: GalileanSatellite = GalileanSatellite {
    u0_deg: 5.717_6,
    u_rate_deg_per_day: 50.234_518_0,
    orbit_radius_km: 1_070_400.0,
    v_opposition: 4.6,
};
const CALLISTO: GalileanSatellite = GalileanSatellite {
    u0_deg: 224.809_2,
    u_rate_deg_per_day: 21.487_980_0,
    orbit_radius_km: 1_882_700.0,
    v_opposition: 5.7,
};

/// J2000 right ascension and declination of Jupiter's north rotation pole (IAU).
const JUPITER_POLE_RA_DEG: f64 = 268.056_595;
const JUPITER_POLE_DEC_DEG: f64 = 64.495_303;
/// J2000 right ascension and declination of Saturn's north rotation pole (IAU).
const SATURN_POLE_RA_DEG: f64 = 40.589;
const SATURN_POLE_DEC_DEG: f64 = 83.537;

/// IAU rotation model of a planet: pole direction (J2000) and prime-meridian angle `W` in degrees
/// as `w0_deg + rate_deg_per_day * d`, with `d` in days since J2000 (TDB).
struct Rotation {
    pole_ra_deg: f64,
    pole_dec_deg: f64,
    w0_deg: f64,
    rate_deg_per_day: f64,
}

fn rotation_of(body: SolarSystemBody) -> Option<Rotation> {
    let (pole_ra_deg, pole_dec_deg, w0_deg, rate_deg_per_day) = match body {
        SolarSystemBody::Mercury => (281.0103, 61.4155, 329.5988, 6.138_510_8),
        SolarSystemBody::Venus => (272.76, 67.16, 160.20, -1.481_368_8),
        SolarSystemBody::Mars => (317.681, 52.886, 176.630, 350.891_982_26),
        SolarSystemBody::Jupiter => (
            JUPITER_POLE_RA_DEG,
            JUPITER_POLE_DEC_DEG,
            284.95,
            870.536_642_0,
        ),
        SolarSystemBody::Saturn => (
            SATURN_POLE_RA_DEG,
            SATURN_POLE_DEC_DEG,
            38.90,
            810.793_902_4,
        ),
        SolarSystemBody::Uranus => (257.311, -15.175, 203.81, -501.160_092_8),
        SolarSystemBody::Neptune => (299.36, 43.46, 249.978, 541.139_775_7),
        _ => return None,
    };
    Some(Rotation {
        pole_ra_deg,
        pole_dec_deg,
        w0_deg,
        rate_deg_per_day,
    })
}

/// Orientation of a planet's body-fixed frame as seen by the observer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetOrientation {
    /// Unit ICRS direction of the north rotation pole.
    pub north_pole_icrs: DVec3,
    /// Planetocentric longitude (east positive, `[-180, 180]`) of the sub-observer point.
    pub sub_observer_lon_deg: f64,
    /// Planetocentric latitude of the sub-observer point. For ringed planets this is the
    /// ring-plane opening angle `B` (positive when the observer is north of the ring plane).
    pub sub_observer_lat_deg: f64,
}

/// Computes the sub-observer point and pole of `body` for an observer looking along `icrs_dir`
/// at a body `distance_km` away (used to correct the rotation phase for light-time).
pub fn planet_orientation(
    body: SolarSystemBody,
    epoch: Epoch,
    icrs_dir: DVec3,
    distance_km: f64,
) -> Option<PlanetOrientation> {
    let rot = rotation_of(body)?;
    let light_days = distance_km / 299_792.458 / 86_400.0;
    let days = epoch.to_jde_tt_days() - 2_451_545.0 - light_days;
    let w_deg = (rot.w0_deg + rot.rate_deg_per_day * days).rem_euclid(360.0);

    let body_to_icrs = DMat3::from_rotation_z((rot.pole_ra_deg + 90.0).to_radians())
        * DMat3::from_rotation_x((90.0 - rot.pole_dec_deg).to_radians())
        * DMat3::from_rotation_z(w_deg.to_radians());
    let to_observer = body_to_icrs.transpose() * (-icrs_dir);
    Some(PlanetOrientation {
        north_pole_icrs: unit_from_ra_dec_deg(rot.pole_ra_deg, rot.pole_dec_deg),
        sub_observer_lon_deg: to_observer.y.atan2(to_observer.x).to_degrees(),
        sub_observer_lat_deg: to_observer.z.clamp(-1.0, 1.0).asin().to_degrees(),
    })
}

fn unit_from_ra_dec_deg(ra_deg: f64, dec_deg: f64) -> DVec3 {
    let (sin_ra, cos_ra) = ra_deg.to_radians().sin_cos();
    let (sin_dec, cos_dec) = dec_deg.to_radians().sin_cos();
    DVec3::new(cos_dec * cos_ra, cos_dec * sin_ra, sin_dec)
}

fn ecliptic_to_equatorial(v: DVec3) -> DVec3 {
    let (sin_e, cos_e) = J2000_OBLIQUITY_DEG.to_radians().sin_cos();
    DVec3::new(v.x, cos_e * v.y - sin_e * v.z, sin_e * v.y + cos_e * v.z)
}

fn kepler_eccentric_anomaly(mean_anomaly_rad: f64, e: f64) -> f64 {
    let m = (mean_anomaly_rad + PI).rem_euclid(TAU) - PI;
    let mut big_e = if e < 0.8 { m } else { PI.copysign(m) };
    for _ in 0..16 {
        let delta = (big_e - e * big_e.sin() - m) / (1.0 - e * big_e.cos());
        big_e -= delta;
        if delta.abs() < 1e-12 {
            break;
        }
    }
    big_e
}

/// Heliocentric J2000 ecliptic position in AU and the mean longitude in degrees.
#[allow(clippy::many_single_char_names)]
fn heliocentric_ecliptic_au(el: &Elements, t: f64) -> (DVec3, f64) {
    let a = Elements::at(el.a_au, t);
    let e = Elements::at(el.e, t);
    let incl = Elements::at(el.incl_deg, t).to_radians();
    let mean_lon = Elements::at(el.mean_lon_deg, t);
    let peri_lon = Elements::at(el.peri_lon_deg, t);
    let node = Elements::at(el.node_deg, t).to_radians();
    let arg_peri = peri_lon.to_radians() - node;
    let mean_anomaly = (mean_lon - peri_lon).to_radians();

    let big_e = kepler_eccentric_anomaly(mean_anomaly, e);
    let x_orb = a * (big_e.cos() - e);
    let y_orb = a * (1.0 - e * e).sqrt() * big_e.sin();

    let (sin_w, cos_w) = arg_peri.sin_cos();
    let (sin_n, cos_n) = node.sin_cos();
    let (sin_i, cos_i) = incl.sin_cos();

    let x = (cos_w * cos_n - sin_w * sin_n * cos_i) * x_orb
        + (-sin_w * cos_n - cos_w * sin_n * cos_i) * y_orb;
    let y = (cos_w * sin_n + sin_w * cos_n * cos_i) * x_orb
        + (-sin_w * sin_n + cos_w * cos_n * cos_i) * y_orb;
    let z = (sin_w * sin_i) * x_orb + (cos_w * sin_i) * y_orb;
    (DVec3::new(x, y, z), mean_lon.rem_euclid(360.0))
}

fn elements_of(body: SolarSystemBody) -> Option<&'static Elements> {
    Some(match body {
        SolarSystemBody::Mercury => &MERCURY,
        SolarSystemBody::Venus => &VENUS,
        SolarSystemBody::Mars => &MARS,
        SolarSystemBody::Jupiter => &JUPITER,
        SolarSystemBody::Saturn => &SATURN,
        SolarSystemBody::Uranus => &URANUS,
        SolarSystemBody::Neptune => &NEPTUNE,
        _ => return None,
    })
}

fn equatorial_radius_km(body: SolarSystemBody) -> f64 {
    match body {
        SolarSystemBody::Mercury => 2_439.7,
        SolarSystemBody::Venus => 6_051.8,
        SolarSystemBody::Mars => 3_396.2,
        SolarSystemBody::Jupiter => 71_492.0,
        SolarSystemBody::Saturn => 60_268.0,
        SolarSystemBody::Uranus => 25_559.0,
        SolarSystemBody::Neptune => 24_764.0,
        SolarSystemBody::Io => 1_821.6,
        SolarSystemBody::Europa => 1_560.8,
        SolarSystemBody::Ganymede => 2_634.1,
        SolarSystemBody::Callisto => 2_410.3,
        SolarSystemBody::Sun => 695_700.0,
        SolarSystemBody::Moon => 1_737.4,
    }
}

fn galilean_of(body: SolarSystemBody) -> Option<&'static GalileanSatellite> {
    Some(match body {
        SolarSystemBody::Io => &IO,
        SolarSystemBody::Europa => &EUROPA,
        SolarSystemBody::Ganymede => &GANYMEDE,
        SolarSystemBody::Callisto => &CALLISTO,
        _ => return None,
    })
}

/// Observer position in km in the J2000 equatorial frame (geocentric, includes elevation).
fn observer_j2000_km(ctx: &ObservationContext) -> DVec3 {
    let lat_rad = ctx.location.latitude.radians();
    let (sin_lat, cos_lat) = lat_rad.sin_cos();
    let u = ((1.0 - EARTH_FLATTENING) * lat_rad.tan()).atan();
    let (sin_u, cos_u) = u.sin_cos();
    let elev_ratio = (ctx.location.elevation_m() * 1e-3) / EARTH_REQ_KM;
    let rho_sin_phi = (1.0 - EARTH_FLATTENING) * sin_u + elev_ratio * sin_lat;
    let rho_cos_phi = cos_u + elev_ratio * cos_lat;
    let (sin_lst, cos_lst) = ctx.local_sidereal_time().sin_cos();
    let of_date =
        DVec3::new(rho_cos_phi * cos_lst, rho_cos_phi * sin_lst, rho_sin_phi) * EARTH_REQ_KM;
    ctx.precession_matrix().transpose() * of_date
}

/// Heliocentric J2000 equatorial position of the Earth's center in AU.
fn earth_heliocentric_equatorial_au(ctx: &ObservationContext, t: f64) -> DVec3 {
    let (emb_ecl, _) = heliocentric_ecliptic_au(&EARTH_MOON_BARYCENTER, t);
    let emb = ecliptic_to_equatorial(emb_ecl);

    // The elements describe the Earth-Moon barycenter; shift to the geocenter.
    let (ra, dec, dist_km, _, _) = lunar_geocentric_of_date(ctx.epoch);
    let moon_of_date = unit_from_ra_dec_deg(ra.degrees(), dec.degrees()) * (dist_km / AU_KM);
    let moon_j2000 = ctx.precession_matrix().transpose() * moon_of_date;
    emb - moon_j2000 / (1.0 + EARTH_MOON_MASS_RATIO)
}

/// Geometry of a planet at a given instant, shared by planet and satellite ephemerides.
struct PlanetState {
    /// Heliocentric J2000 ecliptic position at the retarded time, in AU.
    helio_ecl_au: DVec3,
    /// Heliocentric J2000 equatorial position at the retarded time, in AU.
    helio_eq_au: DVec3,
    /// Heliocentric J2000 equatorial position of the Earth at the observation time, in AU.
    earth_helio_eq_au: DVec3,
    /// Topocentric J2000 equatorial position in km (light-time corrected).
    topo_km: DVec3,
    /// Retarded time offset in days.
    light_time_days: f64,
    /// Mean heliocentric longitude at the retarded time in degrees.
    mean_lon_deg: f64,
}

fn planet_state(
    el: &Elements,
    ctx: &ObservationContext,
    obs_j2000_km: DVec3,
    epoch: Epoch,
) -> PlanetState {
    let t = (epoch.to_jde_tt_days() - 2_451_545.0) / 36_525.0;
    let earth = earth_heliocentric_equatorial_au(ctx, t);

    let mut tau_days = 0.0;
    let mut helio_ecl_au = DVec3::ZERO;
    let mut mean_lon_deg = 0.0;
    let mut geo_au = DVec3::ZERO;
    for _ in 0..3 {
        let t_ret = t - tau_days / 36_525.0;
        (helio_ecl_au, mean_lon_deg) = heliocentric_ecliptic_au(el, t_ret);
        geo_au = ecliptic_to_equatorial(helio_ecl_au) - earth;
        tau_days = geo_au.length() * LIGHT_DAYS_PER_AU;
    }

    PlanetState {
        helio_ecl_au,
        helio_eq_au: ecliptic_to_equatorial(helio_ecl_au),
        earth_helio_eq_au: earth,
        topo_km: geo_au * AU_KM - obs_j2000_km,
        light_time_days: tau_days,
        mean_lon_deg,
    }
}

/// Phase angle (Sun-body-observer) in degrees.
fn phase_angle_deg(helio_eq_au: DVec3, topo_km: DVec3) -> f64 {
    let cos_i = helio_eq_au
        .normalize_or_zero()
        .dot(topo_km.normalize_or_zero());
    cos_i.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Apparent visual magnitude from the Astronomical Almanac phase laws.
fn planet_magnitude(
    body: SolarSystemBody,
    r_au: f64,
    delta_au: f64,
    phase_deg: f64,
    ring_sin_b: f64,
) -> f64 {
    let dist_term = 5.0 * (r_au * delta_au).max(1e-3).log10();
    let i = phase_deg;
    let i100 = i / 100.0;
    dist_term
        + match body {
            SolarSystemBody::Mercury => {
                -0.42 + 3.80 * i100 - 2.73 * i100.powi(2) + 2.00 * i100.powi(3)
            }
            SolarSystemBody::Venus => {
                -4.40 + 0.0009 * i + 0.000_239 * i * i - 0.000_000_65 * i * i * i
            }
            SolarSystemBody::Mars => -1.52 + 1.6 * i100,
            SolarSystemBody::Jupiter => -9.40 + 0.5 * i100,
            SolarSystemBody::Saturn => -8.914 - 1.825 * ring_sin_b.abs() + 0.026 * i,
            SolarSystemBody::Uranus => -7.19,
            SolarSystemBody::Neptune => -6.87,
            _ => 0.0,
        }
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    body: SolarSystemBody,
    ctx: &ObservationContext,
    obs_j2000_km: DVec3,
    earth_helio_eq_au: DVec3,
    topo_km: DVec3,
    phase_deg: f64,
    magnitude: f64,
) -> SolarSystemEphemeris {
    let distance_km = topo_km.length().max(1.0);
    let icrs_dir = (topo_km / distance_km).normalize_or_zero();
    let topo_enu = ctx
        .equatorial_to_topocentric_dir(icrs_dir)
        .normalize_or_zero();
    let (ra, dec) = cartesian_to_canonical_spherical(icrs_dir);
    let (az, alt) = topocentric_enu_to_spherical(topo_enu);

    let sun_icrs_dir = (-earth_helio_eq_au * AU_KM - obs_j2000_km).normalize_or_zero();
    let elongation_deg = icrs_dir
        .dot(sun_icrs_dir)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    let angular_radius_deg = (equatorial_radius_km(body) / distance_km)
        .clamp(0.0, 1.0)
        .asin()
        .to_degrees();

    SolarSystemEphemeris {
        body,
        icrs_dir,
        topo_enu,
        ra_deg: ra.degrees(),
        dec_deg: dec.degrees(),
        az_deg: az.degrees(),
        alt_deg: alt.degrees(),
        distance_km,
        angular_radius_deg,
        apparent_magnitude: magnitude as f32,
        sun_icrs_dir,
        lunar_phase: None,
        illuminated_fraction: f64::midpoint(1.0, phase_deg.to_radians().cos()),
        elongation_deg,
    }
}

/// Computes the topocentric ephemeris of one of the seven non-terrestrial planets.
pub fn planet_ephemeris(
    body: SolarSystemBody,
    location: GeodeticPosition,
    epoch: Epoch,
) -> SolarSystemEphemeris {
    let el = elements_of(body).expect("planet_ephemeris requires a planet body");
    let ctx = ObservationContext::new(location, epoch);
    let obs = observer_j2000_km(&ctx);
    let state = planet_state(el, &ctx, obs, epoch);

    let phase = phase_angle_deg(state.helio_eq_au, state.topo_km);
    let ring_sin_b = if body == SolarSystemBody::Saturn {
        let pole = unit_from_ra_dec_deg(SATURN_POLE_RA_DEG, SATURN_POLE_DEC_DEG);
        pole.dot(-state.topo_km.normalize_or_zero())
    } else {
        0.0
    };
    let mag = planet_magnitude(
        body,
        state.helio_eq_au.length(),
        state.topo_km.length() / AU_KM,
        phase,
        ring_sin_b,
    );
    assemble(
        body,
        &ctx,
        obs,
        state.earth_helio_eq_au,
        state.topo_km,
        phase,
        mag,
    )
}

/// Computes the topocentric ephemeris of a Galilean satellite of Jupiter.
pub fn galilean_ephemeris(
    body: SolarSystemBody,
    location: GeodeticPosition,
    epoch: Epoch,
) -> SolarSystemEphemeris {
    let sat = galilean_of(body).expect("galilean_ephemeris requires a Galilean satellite");
    let ctx = ObservationContext::new(location, epoch);
    let obs = observer_j2000_km(&ctx);
    let jup = planet_state(&JUPITER, &ctx, obs, epoch);

    // Jupiter's equation of center (true minus mean heliocentric longitude).
    let true_lon_deg = jup.helio_ecl_au.y.atan2(jup.helio_ecl_au.x).to_degrees();
    let center_deg = (true_lon_deg - jup.mean_lon_deg + 180.0).rem_euclid(360.0) - 180.0;

    // Signed Sun-Jupiter-Earth angle, positive before opposition (Meeus psi).
    let ecl_north = ecliptic_to_equatorial(DVec3::Z);
    let to_sun = -jup.helio_eq_au;
    let to_earth = jup.earth_helio_eq_au - jup.helio_eq_au;
    let psi_deg = ecl_north
        .dot(to_sun.cross(to_earth))
        .atan2(to_sun.dot(to_earth))
        .to_degrees();

    let days = epoch.to_jde_tt_days() - 2_451_545.0 - jup.light_time_days;
    let u_deg =
        (sat.u0_deg + sat.u_rate_deg_per_day * days + psi_deg - center_deg).rem_euclid(360.0);
    let (sin_u, cos_u) = u_deg.to_radians().sin_cos();

    // `a` points from Jupiter toward the observer within Jupiter's equatorial plane;
    // `b = pole x a` is the direction of apparent westward motion in front of the disc.
    let pole = unit_from_ra_dec_deg(JUPITER_POLE_RA_DEG, JUPITER_POLE_DEC_DEG);
    let view = jup.topo_km.normalize_or_zero();
    let a = (-view + pole * view.dot(pole)).normalize_or_zero();
    let b = pole.cross(a);
    let topo_km = jup.topo_km + (a * cos_u + b * sin_u) * sat.orbit_radius_km;

    let phase = phase_angle_deg(jup.helio_eq_au, jup.topo_km);
    let r_au = jup.helio_eq_au.length();
    let delta_au = jup.topo_km.length() / AU_KM;
    let mag = sat.v_opposition + 5.0 * ((r_au * delta_au) / (5.2 * 4.2)).max(1e-3).log10();

    assemble(body, &ctx, obs, jup.earth_helio_eq_au, topo_km, phase, mag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paris() -> GeodeticPosition {
        GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0)
    }

    #[test]
    fn kepler_solver_satisfies_kepler_equation() {
        for &e in &[0.0, 0.2, 0.6, 0.95] {
            for k in 0..24 {
                let m = f64::from(k) * 0.27 - 3.0;
                let big_e = kepler_eccentric_anomaly(m, e);
                let wrapped = (m + PI).rem_euclid(TAU) - PI;
                assert!((big_e - e * big_e.sin() - wrapped).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn earth_is_about_one_au_from_the_sun() {
        let epoch = Epoch::from_gregorian_utc(2026, 7, 4, 0, 0, 0, 0);
        let ctx = ObservationContext::new(paris(), epoch);
        let t = (epoch.to_jde_tt_days() - 2_451_545.0) / 36_525.0;
        // Aphelion is in early July.
        let r = earth_heliocentric_equatorial_au(&ctx, t).length();
        assert!((r - 1.0167).abs() < 0.002, "r = {r}");
    }

    #[test]
    fn planets_have_sane_distances_sizes_and_magnitudes() {
        let epoch = Epoch::from_gregorian_utc(2026, 11, 23, 0, 0, 0, 0);
        let expected_au = [
            (SolarSystemBody::Mercury, 0.55, 1.45),
            (SolarSystemBody::Venus, 0.25, 1.75),
            (SolarSystemBody::Mars, 0.35, 2.7),
            (SolarSystemBody::Jupiter, 3.9, 6.5),
            (SolarSystemBody::Saturn, 7.9, 11.1),
            (SolarSystemBody::Uranus, 17.2, 21.2),
            (SolarSystemBody::Neptune, 28.8, 31.1),
        ];
        for (body, lo, hi) in expected_au {
            let eph = body.ephemeris(paris(), epoch);
            let au = eph.distance_km / AU_KM;
            assert!(au > lo && au < hi, "{body:?} at {au} AU");
            assert!((eph.icrs_dir.length() - 1.0).abs() < 1e-12);
            assert!((eph.topo_enu.length() - 1.0).abs() < 1e-12);
            assert!(eph.angular_radius_deg > 0.0 && eph.angular_radius_deg < 0.01);
            assert!(eph.apparent_magnitude.is_finite());
            assert!(eph.lunar_phase.is_none());
            assert!((0.0..=1.0).contains(&eph.illuminated_fraction));
        }
        let venus = SolarSystemBody::Venus.ephemeris(paris(), epoch);
        assert!(venus.apparent_magnitude < -3.0 && venus.apparent_magnitude > -5.0);
        let jupiter = SolarSystemBody::Jupiter.ephemeris(paris(), epoch);
        assert!(jupiter.apparent_magnitude < -1.5 && jupiter.apparent_magnitude > -3.2);
    }

    #[test]
    fn outer_planets_are_opposite_the_sun_at_opposition() {
        for (body, y, m, d) in [
            (SolarSystemBody::Mars, 2025, 1, 16),
            (SolarSystemBody::Saturn, 2025, 9, 21),
        ] {
            let eph = body.ephemeris(paris(), Epoch::from_gregorian_utc(y, m, d, 0, 0, 0, 0));
            assert!(
                eph.elongation_deg > 170.0,
                "{body:?}: {}",
                eph.elongation_deg
            );
            assert!(eph.illuminated_fraction > 0.99);
        }
        let mars = SolarSystemBody::Mars
            .ephemeris(paris(), Epoch::from_gregorian_utc(2025, 1, 16, 0, 0, 0, 0));
        // Mars stood in Gemini (RA ~ 8h, Dec ~ +25 deg) at its January 2025 opposition.
        assert!(
            (mars.ra_deg / 15.0 - 7.95).abs() < 0.2,
            "RA {}",
            mars.ra_deg / 15.0
        );
        assert!((mars.dec_deg - 25.1).abs() < 1.0, "Dec {}", mars.dec_deg);
    }

    #[test]
    fn saturn_ring_plane_crossing_in_march_2025_has_zero_opening() {
        let epoch = Epoch::from_gregorian_utc(2025, 3, 23, 12, 0, 0, 0);
        let eph = SolarSystemBody::Saturn.ephemeris(paris(), epoch);
        let o = eph.orientation(epoch).expect("Saturn has an orientation");
        assert!(
            o.sub_observer_lat_deg.abs() < 0.6,
            "B = {}",
            o.sub_observer_lat_deg
        );
        assert!((o.north_pole_icrs.length() - 1.0).abs() < 1e-12);

        // The rings open up again afterwards.
        let later = Epoch::from_gregorian_utc(2026, 11, 23, 0, 0, 0, 0);
        let eph = SolarSystemBody::Saturn.ephemeris(paris(), later);
        let o = eph.orientation(later).unwrap();
        assert!(o.sub_observer_lat_deg.abs() > 1.0 && o.sub_observer_lat_deg.abs() < 10.0);
    }

    #[test]
    fn sub_observer_longitude_advances_with_rotation() {
        let epoch = Epoch::from_gregorian_utc(2026, 11, 23, 0, 0, 0, 0);
        let later = Epoch::from_unix_seconds(epoch.to_unix_seconds() + 3600.0);
        let lon = |ep: Epoch| {
            let eph = SolarSystemBody::Jupiter.ephemeris(paris(), ep);
            eph.orientation(ep).unwrap().sub_observer_lon_deg
        };
        // Jupiter turns ~36 deg per hour; the sub-observer longitude decreases as it rotates east.
        let delta = (lon(later) - lon(epoch) + 540.0).rem_euclid(360.0) - 180.0;
        assert!((delta + 36.2).abs() < 1.0, "delta = {delta}");
        assert!(
            SolarSystemBody::Io
                .ephemeris(paris(), epoch)
                .orientation(epoch)
                .is_none()
        );
    }

    #[test]
    fn inner_planet_elongations_are_bounded() {
        let start = Epoch::from_gregorian_utc(2026, 1, 1, 0, 0, 0, 0).to_unix_seconds();
        let (mut max_mercury, mut max_venus) = (0.0_f64, 0.0_f64);
        for day in 0..400 {
            let ep = Epoch::from_unix_seconds(start + f64::from(day) * 86_400.0);
            max_mercury = max_mercury.max(
                SolarSystemBody::Mercury
                    .ephemeris(paris(), ep)
                    .elongation_deg,
            );
            max_venus = max_venus.max(SolarSystemBody::Venus.ephemeris(paris(), ep).elongation_deg);
        }
        assert!(
            (18.0..=28.5).contains(&max_mercury),
            "mercury {max_mercury}"
        );
        assert!((40.0..=47.5).contains(&max_venus), "venus {max_venus}");
    }

    #[test]
    fn galilean_moons_orbit_close_to_jupiter_with_correct_periods() {
        let epoch = Epoch::from_gregorian_utc(2026, 11, 23, 0, 0, 0, 0);
        let jupiter = SolarSystemBody::Jupiter.ephemeris(paris(), epoch);
        for (body, orbit_km) in [
            (SolarSystemBody::Io, 421_800.0),
            (SolarSystemBody::Europa, 671_100.0),
            (SolarSystemBody::Ganymede, 1_070_400.0),
            (SolarSystemBody::Callisto, 1_882_700.0),
        ] {
            let eph = body.ephemeris(paris(), epoch);
            let sep_km =
                eph.icrs_dir.dot(jupiter.icrs_dir).clamp(-1.0, 1.0).acos() * jupiter.distance_km;
            assert!(sep_km <= orbit_km * 1.01, "{body:?} separation {sep_km} km");
            assert!(eph.angular_radius_deg < jupiter.angular_radius_deg);
        }

        // Io completes one synodic revolution in ~1.769 days: the separation must repeat.
        let later = Epoch::from_unix_seconds(epoch.to_unix_seconds() + 1.769_860_5 * 86_400.0);
        let sep = |ep: Epoch| {
            let j = SolarSystemBody::Jupiter.ephemeris(paris(), ep);
            let io = SolarSystemBody::Io.ephemeris(paris(), ep);
            io.icrs_dir.dot(j.icrs_dir).clamp(-1.0, 1.0).acos() * j.distance_km
        };
        assert!((sep(epoch) - sep(later)).abs() < 25_000.0);
    }
}
