use gpui::{App, Font, Pixels, font, px};
use theme::{ThemeSettingsProvider, UiDensity, set_theme_settings_provider};

pub struct DefaultThemeSettingsProvider {
    pub ui_font: Font,
    pub buffer_font: Font,
    pub ui_font_size: Pixels,
    pub buffer_font_size: Pixels,
    pub ui_density: UiDensity,
}

impl Default for DefaultThemeSettingsProvider {
    fn default() -> Self {
        Self {
            ui_font: font(".ZedSans"),
            buffer_font: font(".ZedMono"),
            ui_font_size: px(14.0),
            buffer_font_size: px(14.0),
            ui_density: UiDensity::Default,
        }
    }
}

impl ThemeSettingsProvider for DefaultThemeSettingsProvider {
    fn ui_font<'a>(&'a self, _cx: &'a App) -> &'a Font {
        &self.ui_font
    }

    fn buffer_font<'a>(&'a self, _cx: &'a App) -> &'a Font {
        &self.buffer_font
    }

    fn ui_font_size(&self, _cx: &App) -> Pixels {
        self.ui_font_size
    }

    fn buffer_font_size(&self, _cx: &App) -> Pixels {
        self.buffer_font_size
    }

    fn ui_density(&self, _cx: &App) -> UiDensity {
        self.ui_density
    }
}

pub fn init(cx: &mut App) {
    set_theme_settings_provider(Box::new(DefaultThemeSettingsProvider::default()), cx);
}
