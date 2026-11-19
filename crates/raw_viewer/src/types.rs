use glam::Vec2;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DisplayRange {
    pub min: f32,
    pub max: f32,
}

impl DisplayRange {
    pub const DEFAULT: Self = Self { min: 0.0, max: 1.0 };

    pub fn new(min: f32, max: f32) -> Self {
        let min = if min.is_finite() { min } else { 0.0 }.clamp(0.0, 0.9999);
        let max = if max.is_finite() { max } else { 1.0 }.clamp(min + 0.0001, 1.0);
        Self { min, max }
    }

    pub fn as_array(self) -> [f32; 2] {
        [self.min, self.max]
    }
}

impl Default for DisplayRange {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ViewTransform {
    pub pan: Vec2,
    pub zoom: f32,
}

impl ViewTransform {
    pub const MIN_ZOOM: f32 = 0.05;
    pub const MAX_ZOOM: f32 = 100.0;

    pub const DEFAULT: Self = Self {
        pan: Vec2::ZERO,
        zoom: 1.0,
    };

    pub fn new(pan: Vec2, zoom: f32) -> Self {
        let zoom = if zoom.is_finite() { zoom } else { 1.0 };
        Self {
            pan,
            zoom: zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM),
        }
    }
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self::DEFAULT
    }
}
