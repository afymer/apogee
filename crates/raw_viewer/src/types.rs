use glam::Vec2;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DisplayRange {
    pub min: f32,
    pub max: f32,
}

impl DisplayRange {
    pub const DEFAULT: Self = Self { min: 0.0, max: 1.0 };

    pub fn new(min: f32, max: f32) -> Self {
        let min = min.clamp(0.0, 1.0);
        let max = max.clamp(min + 0.0001, 1.0);
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
    pub const DEFAULT: Self = Self {
        pan: Vec2::ZERO,
        zoom: 1.0,
    };

    pub fn new(pan: Vec2, zoom: f32) -> Self {
        Self {
            pan,
            zoom: zoom.clamp(0.05, 100.0),
        }
    }
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self::DEFAULT
    }
}
