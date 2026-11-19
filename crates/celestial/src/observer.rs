use glam::{DMat3, DVec3};
use hifitime::Epoch;

use crate::coords::{
    angle::Angle, canonical_spherical_to_cartesian, cartesian_to_canonical_spherical,
    geodetic::GeodeticPosition, topocentric_enu_to_spherical, topocentric_spherical_to_enu,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservationContext {
    pub location: GeodeticPosition,
    pub epoch: Epoch,
}

impl Default for ObservationContext {
    fn default() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        Self {
            location: GeodeticPosition::default(),
            epoch: Epoch::now().unwrap_or_else(|_| Epoch::from_unix_seconds(secs)),
        }
    }
}

impl ObservationContext {
    pub fn new(location: GeodeticPosition, epoch: Epoch) -> Self {
        Self { location, epoch }
    }

    pub fn update_epoch_now(&mut self) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.epoch = Epoch::now().unwrap_or_else(|_| Epoch::from_unix_seconds(secs));
    }

    pub fn set_epoch_unix_seconds(&mut self, unix_seconds: f64) {
        self.epoch = Epoch::from_unix_seconds(unix_seconds);
    }

    pub fn epoch_unix_seconds(&self) -> f64 {
        self.epoch.to_unix_seconds()
    }

    pub fn local_sidereal_time_hms(&self) -> (u8, u8, u8) {
        let total_secs = (self.local_sidereal_time().hours() * 3600.0).round() as u64 % 86_400;
        let h = (total_secs / 3600) as u8;
        let m = ((total_secs % 3600) / 60) as u8;
        let s = (total_secs % 60) as u8;
        (h, m, s)
    }

    pub fn format_lst_hms(&self) -> String {
        let (h, m, s) = self.local_sidereal_time_hms();
        format!("{h:02}h {m:02}m {s:02}s")
    }

    /// IAU 1976 / Lieske equatorial precession matrix from J2000/ICRS to mean equator & equinox of date.
    pub fn precession_matrix(&self) -> DMat3 {
        let t = (self.epoch.to_jde_tt_days() - 2451545.0) / 36525.0;
        let t2 = t * t;
        let t3 = t2 * t;
        let arcsec_to_rad = std::f64::consts::PI / (180.0 * 3600.0);
        let zeta = (2306.2181 * t + 0.30188 * t2 + 0.017998 * t3) * arcsec_to_rad;
        let z = (2306.2181 * t + 1.09468 * t2 + 0.018203 * t3) * arcsec_to_rad;
        let theta = (2004.3109 * t - 0.42665 * t2 - 0.041833 * t3) * arcsec_to_rad;
        DMat3::from_rotation_z(z) * DMat3::from_rotation_y(-theta) * DMat3::from_rotation_z(zeta)
    }

    /// IAU 1982/2000 formula
    pub fn gmst(&self) -> Angle {
        let jd = self.epoch.to_jde_utc_days();
        let d = jd - 2451545.0;
        let t = d / 36525.0;
        let gmst_deg =
            280.46061837 + 360.98564736629 * d + 0.000387933 * t * t - (t * t * t) / 38710000.0;
        Angle::from_degrees_normalized(gmst_deg)
    }

    pub fn local_sidereal_time(&self) -> Angle {
        (self.gmst() + self.location.longitude).normalize()
    }

    pub fn equatorial_to_topocentric_matrix(&self) -> DMat3 {
        let theta = self.local_sidereal_time();
        let phi = self.location.latitude;

        let (sin_theta, cos_theta) = theta.sin_cos();
        let (sin_phi, cos_phi) = phi.sin_cos();

        // Row 0 (East):  [-sin_theta, cos_theta, 0]
        // Row 1 (North): [-sin_phi * cos_theta, -sin_phi * sin_theta, cos_phi]
        // Row 2 (Up):    [cos_phi * cos_theta, cos_phi * sin_theta, sin_phi]
        let enu_of_date = DMat3::from_cols(
            DVec3::new(-sin_theta, -sin_phi * cos_theta, cos_phi * cos_theta),
            DVec3::new(cos_theta, -sin_phi * sin_theta, cos_phi * sin_theta),
            DVec3::new(0.0, cos_phi, sin_phi),
        );
        enu_of_date * self.precession_matrix()
    }

    /// Rotation matrix from ENU to Equatorial ICRS
    pub fn topocentric_to_equatorial_matrix(&self) -> DMat3 {
        self.equatorial_to_topocentric_matrix().transpose()
    }

    /// Transforms an equatorial unit vector to a topocentric ENU unit vector
    pub fn equatorial_to_topocentric_dir(&self, v_eq: DVec3) -> DVec3 {
        self.equatorial_to_topocentric_matrix().mul_vec3(v_eq)
    }

    /// Transforms a topocentric ENU unit vector to an equatorial unit vector
    pub fn topocentric_to_equatorial_dir(&self, v_enu: DVec3) -> DVec3 {
        self.topocentric_to_equatorial_matrix().mul_vec3(v_enu)
    }

    /// Converts RA/Dec to Az/Alt
    pub fn equatorial_to_topocentric_coords(&self, ra: Angle, dec: Angle) -> (Angle, Angle) {
        let v_eq = canonical_spherical_to_cartesian(ra, dec);
        let v_enu = self.equatorial_to_topocentric_dir(v_eq);
        topocentric_enu_to_spherical(v_enu)
    }

    /// Converts Az/Alt to RA/Dec
    pub fn topocentric_to_equatorial_coords(&self, az: Angle, alt: Angle) -> (Angle, Angle) {
        let v_enu = topocentric_spherical_to_enu(az, alt);
        let v_eq = self.topocentric_to_equatorial_dir(v_enu);
        cartesian_to_canonical_spherical(v_eq)
    }
}
