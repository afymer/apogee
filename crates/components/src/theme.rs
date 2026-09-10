use gpui::{Hsla, hsla, rgb};
use theme::{Appearance, Theme, ThemeColors};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderColors {
    pub track_background: Hsla,
    pub track_background_hover: Hsla,
    pub active_track: Hsla,
    pub active_track_idle: Hsla,
    pub knob_background: Hsla,
    pub knob_background_active: Hsla,
    pub knob_background_hover: Hsla,
    pub knob_border: Hsla,
    pub knob_border_active: Hsla,
    pub knob_border_hover: Hsla,
}

impl SliderColors {
    pub fn knob_border(&self, is_active: bool) -> Hsla {
        if is_active {
            self.knob_border_active
        } else {
            self.knob_border
        }
    }

    pub fn knob_background(&self, is_active: bool) -> Hsla {
        if is_active {
            self.knob_background_active
        } else {
            self.knob_background
        }
    }
}

impl Default for SliderColors {
    fn default() -> Self {
        SliderColors {
            track_background: hsla(0.0, 0.0, 1.0, 0.12),
            track_background_hover: hsla(0.0, 0.0, 1.0, 0.18),
            active_track: hsla(0.0, 0.0, 1.0, 0.50),
            active_track_idle: hsla(0.0, 0.0, 1.0, 0.85),
            knob_background: rgb(0xffffff).into(),
            knob_background_active: rgb(0xffffff).into(),
            knob_background_hover: rgb(0xffffff).into(),
            knob_border: hsla(0.0, 0.0, 0.0, 0.25),
            knob_border_active: hsla(0.0, 0.0, 1.0, 0.60),
            knob_border_hover: hsla(0.0, 0.0, 1.0, 0.45),
        }
    }
}

pub trait SliderThemeExt {
    fn slider_colors(&self) -> SliderColors;
}

impl SliderThemeExt for ThemeColors {
    fn slider_colors(&self) -> SliderColors {
        SliderColors {
            track_background: hsla(0.0, 0.0, 1.0, 0.12),
            track_background_hover: hsla(0.0, 0.0, 1.0, 0.18),
            active_track: hsla(0.0, 0.0, 1.0, 0.50),
            active_track_idle: hsla(0.0, 0.0, 1.0, 0.85),
            knob_background: rgb(0xffffff).into(),
            knob_background_active: rgb(0xffffff).into(),
            knob_background_hover: rgb(0xffffff).into(),
            knob_border: hsla(0.0, 0.0, 0.0, 0.25),
            knob_border_active: hsla(0.0, 0.0, 1.0, 0.60),
            knob_border_hover: hsla(0.0, 0.0, 1.0, 0.45),
        }
    }
}

impl SliderThemeExt for Theme {
    fn slider_colors(&self) -> SliderColors {
        match self.appearance() {
            Appearance::Light => SliderColors {
                track_background: hsla(0.0, 0.0, 0.0, 0.08),
                track_background_hover: hsla(0.0, 0.0, 0.0, 0.14),
                active_track: hsla(0.0, 0.0, 0.0, 0.45),
                active_track_idle: hsla(0.0, 0.0, 0.0, 0.85),
                knob_background: hsla(0.0, 0.0, 0.0, 0.90),
                knob_background_active: hsla(0.0, 0.0, 0.0, 1.0),
                knob_background_hover: hsla(0.0, 0.0, 0.0, 0.95),
                knob_border: hsla(0.0, 0.0, 1.0, 0.30),
                knob_border_active: hsla(0.0, 0.0, 1.0, 0.60),
                knob_border_hover: hsla(0.0, 0.0, 1.0, 0.45),
            },
            Appearance::Dark => self.colors().slider_colors(),
        }
    }
}
