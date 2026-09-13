use glam::{DMat3, DVec3};
use hifitime::Epoch;

use crate::coords::{
    angle::Angle, canonical_spherical_to_cartesian, geodetic::GeodeticPosition,
    topocentric_spherical_to_enu,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservationContext {
    pub location: GeodeticPosition,
    pub epoch: Epoch,
}

impl Default for ObservationContext {
    fn default() -> Self {
        Self {
            location: GeodeticPosition::default(),
            // Placeholder, TODO
            epoch: Epoch::from_gregorian_utc_hms(2026, 9, 17, 21, 0, 0),
        }
    }
}

impl ObservationContext {
    pub fn new(location: GeodeticPosition, epoch: Epoch) -> Self {
        Self { location, epoch }
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
        DMat3::from_cols(
            DVec3::new(-sin_theta, -sin_phi * cos_theta, cos_phi * cos_theta),
            DVec3::new(cos_theta, -sin_phi * sin_theta, cos_phi * sin_theta),
            DVec3::new(0.0, cos_phi, sin_phi),
        )
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

        let alt = Angle::from_radians(v_enu.z.clamp(-1.0, 1.0).asin());
        let az = Angle::from_radians_normalized(v_enu.x.atan2(v_enu.y));
        (az, alt)
    }

    /// Converts Az/Alt to RA/Dec
    pub fn topocentric_to_equatorial_coords(&self, az: Angle, alt: Angle) -> (Angle, Angle) {
        let v_enu = topocentric_spherical_to_enu(az, alt);
        let v_eq = self.topocentric_to_equatorial_dir(v_enu);

        let dec = Angle::from_radians(v_eq.z.clamp(-1.0, 1.0).asin());
        let ra = Angle::from_radians_normalized(v_eq.y.atan2(v_eq.x));
        (ra, dec)
    }
}
