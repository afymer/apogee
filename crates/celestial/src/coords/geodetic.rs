use crate::coords::{altitude::Altitude, angle::Angle};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeodeticPosition {
    pub longitude: Angle,
    pub latitude: Angle,
    pub altitude_m: Altitude,
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
