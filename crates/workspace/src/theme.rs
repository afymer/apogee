use gpui::{Hsla, rgb};
use theme::{Appearance, Theme, ThemeColors};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderColors {
    pub track_background: Hsla,
    pub active_track: Hsla,
    pub knob_background: Hsla,
    pub knob_background_active: Hsla,
    pub knob_background_hover: Hsla,
    pub knob_border: Hsla,
    pub knob_border_active: Hsla,
    pub knob_border_hover: Hsla,
}

impl SliderColors {
    pub const ACCENT: u32 = 0x38bdf8;
    pub const ACCENT_ACTIVE: u32 = 0x0284c7;
    pub const ACCENT_HOVER: u32 = 0x0369a1;
    pub const ACCENT_HOVER_BORDER: u32 = 0x7dd3fc;
    pub const KNOB_BG: u32 = 0x1e293b;
    pub const KNOB_BORDER: u32 = 0xffffff;

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
            track_background: rgb(0x0f172a).into(),
            active_track: rgb(Self::ACCENT).into(),
            knob_background: rgb(Self::KNOB_BG).into(),
            knob_background_active: rgb(Self::ACCENT_ACTIVE).into(),
            knob_background_hover: rgb(Self::ACCENT_HOVER).into(),
            knob_border: rgb(Self::KNOB_BORDER).into(),
            knob_border_active: rgb(Self::ACCENT).into(),
            knob_border_hover: rgb(Self::ACCENT_HOVER_BORDER).into(),
        }
    }
}

pub trait SliderThemeExt {
    fn slider_colors(&self) -> SliderColors;
}

impl SliderThemeExt for ThemeColors {
    fn slider_colors(&self) -> SliderColors {
        SliderColors {
            track_background: self.element_background,
            active_track: rgb(SliderColors::ACCENT).into(),
            knob_background: rgb(SliderColors::KNOB_BG).into(),
            knob_background_active: rgb(SliderColors::ACCENT_ACTIVE).into(),
            knob_background_hover: rgb(SliderColors::ACCENT_HOVER).into(),
            knob_border: rgb(SliderColors::KNOB_BORDER).into(),
            knob_border_active: rgb(SliderColors::ACCENT).into(),
            knob_border_hover: rgb(SliderColors::ACCENT_HOVER_BORDER).into(),
        }
    }
}

impl SliderThemeExt for Theme {
    fn slider_colors(&self) -> SliderColors {
        match self.appearance() {
            Appearance::Light => SliderColors {
                track_background: self.colors().element_background,
                active_track: rgb(0x0284c7).into(),
                knob_background: rgb(0xf8fafc).into(),
                knob_background_active: rgb(0x0284c7).into(),
                knob_background_hover: rgb(0xe2e8f0).into(),
                knob_border: rgb(0x64748b).into(),
                knob_border_active: rgb(0x0284c7).into(),
                knob_border_hover: rgb(0x0ea5e9).into(),
            },
            Appearance::Dark => self.colors().slider_colors(),
        }
    }
}
