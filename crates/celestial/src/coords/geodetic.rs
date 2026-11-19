use crate::coords::{altitude::Altitude, angle::Angle};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeodeticPosition {
    pub longitude: Angle,
    pub latitude: Angle,
    pub altitude_m: Altitude,
}

impl GeodeticPosition {
    pub fn from_degrees(latitude_deg: f64, longitude_deg: f64, elevation_m: f64) -> Self {
        Self {
            longitude: Angle::from_degrees(longitude_deg),
            latitude: Angle::from_degrees(latitude_deg),
            altitude_m: Altitude::from_wgs84(elevation_m),
        }
    }

    pub fn latitude_deg(&self) -> f64 {
        self.latitude.degrees()
    }

    pub fn longitude_deg(&self) -> f64 {
        self.longitude.degrees()
    }

    pub fn elevation_m(&self) -> f64 {
        self.altitude_m.wgs84()
    }
}

impl Default for GeodeticPosition {
    fn default() -> Self {
        Self {
            longitude: Angle::from_degrees(2.3364),
            latitude: Angle::from_degrees(48.8364),
            altitude_m: Altitude::from_wgs84(67.0),
        }
    }
}
