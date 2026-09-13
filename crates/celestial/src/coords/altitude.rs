#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Altitude(f64);

impl Default for Altitude {
    fn default() -> Self {
        Self(0.0)
    }
}

impl Altitude {
    pub const fn from_wgs84(wgs84: f64) -> Self {
        Self(wgs84)
    }

    pub const fn wgs84(&self) -> f64 {
        self.0
    }
}
