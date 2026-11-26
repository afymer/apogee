//! Shared quick-toggle sky & grid layer icons, state queries, and pill styling
//! used by both the Planetarium (`Explore`) and Mount (`3D Attitude`) viewports.

use gpui::{Context, Hsla, rgb};
use theme::ActiveTheme;

use crate::SettingsModel;

pub const ICON_LAYER_ATMOSPHERE: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 16.5Q12 13.5 21 16.5" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M7.5 15.2A4.5 4.5 0 0 1 16.5 15.2" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M12 5.2V7.8M6.2 7.8L8 9.6M17.8 7.8L16 9.6" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M6.5 19.5H17.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
</svg>"##;

pub const ICON_LAYER_GROUND: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 10.5H21.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M4 10.5A8 8 0 0 0 20 10.5" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5.6 14.8H18.4" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <path d="M12 10.5V18.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
</svg>"##;

pub const ICON_LAYER_HORIZON: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 17.5L7.5 10.5L11.8 15L16.2 8.5L21.5 17.5" stroke="black" stroke-width="1.55" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M2.5 19.5H21.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <circle cx="8" cy="6.2" r="1.2" fill="black"/>
</svg>"##;

pub const ICON_LAYER_HIPS: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" stroke="black" stroke-width="1.5"/>
  <circle cx="12" cy="12" r="1.4" fill="black"/>
  <path d="M12 10.6C13.8 8.2 17 8.8 17.5 11.2M12 13.4C10.2 15.8 7 15.2 6.5 12.8" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <circle cx="7" cy="8" r="0.9" fill="black"/>
  <circle cx="17" cy="16.2" r="0.9" fill="black"/>
</svg>"##;

pub const ICON_LAYER_STARS: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M11 4.5L12.5 9.5L17.5 11L12.5 12.5L11 17.5L9.5 12.5L4.5 11L9.5 9.5L11 4.5Z" stroke="black" stroke-width="1.45" stroke-linejoin="round"/>
  <circle cx="18.2" cy="6.2" r="1.15" fill="black"/>
  <circle cx="17.5" cy="17.8" r="1.05" fill="black"/>
  <circle cx="6" cy="17.5" r="0.95" fill="black"/>
</svg>"##;

pub const ICON_LAYER_DSO: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <ellipse cx="12" cy="12" rx="8.5" ry="3.8" transform="rotate(-32 12 12)" stroke="black" stroke-width="1.5"/>
  <circle cx="12" cy="12" r="1.6" fill="black"/>
  <circle cx="6.2" cy="6.4" r="0.95" fill="black"/>
  <circle cx="17.8" cy="17.6" r="0.95" fill="black"/>
</svg>"##;

pub const ICON_LAYER_SOLAR_SYSTEM: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="4.2" stroke="black" stroke-width="1.5"/>
  <path d="M12 3.2V5.3M12 18.7V20.8M3.2 12H5.3M18.7 12H20.8M5.8 5.8L7.3 7.3M16.7 16.7L18.2 18.2M18.2 5.8L16.7 7.3M7.3 16.7L5.8 18.2" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

pub const ICON_GRID_ALTAZ: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M5 8.5H19M4 15.5H20" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M12 3.5V20.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M9.5 4.2Q6.2 12 9.5 19.8M14.5 4.2Q17.8 12 14.5 19.8" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

pub const ICON_GRID_EQUATORIAL: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M16 4.5L8 19.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M7.3 5.5L20 12.3M4 11.7L16.7 18.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M12.3 3.3Q6.9 9.3 5 17.1M19 6.9Q17.1 14.7 11.7 20.7" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

pub const ICON_LAYER_SKY_LIMITS: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M12 3.8L18.5 6.4V11.6C18.5 15.8 15.7 19.2 12 20.5C8.3 19.2 5.5 15.8 5.5 11.6V6.4L12 3.8Z" stroke="black" stroke-width="1.5" stroke-linejoin="round"/>
  <path d="M8.5 13.5Q12 11.2 15.5 13.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <circle cx="12" cy="9.2" r="1.3" fill="black"/>
</svg>"##;

pub const ICON_LAYER_TRAJECTORIES: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3.5 17.5C6.5 8 17.5 8 20.5 17.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <circle cx="12" cy="10.4" r="2.1" fill="black"/>
  <circle cx="5.5" cy="14" r="1.2" fill="black"/>
  <circle cx="18.5" cy="14" r="1.2" fill="black"/>
</svg>"##;

#[derive(Clone, Copy)]
pub struct LayerPillStyle {
    pub pill_bg: Hsla,
    pub pill_border: Hsla,
    pub active_bg: Hsla,
    pub hover_bg: Hsla,
    pub active_icon: Hsla,
    pub disabled_icon: Hsla,
    pub popover_ring: Hsla,
}

impl LayerPillStyle {
    pub fn from_theme(cx: &gpui::App) -> Self {
        let colors = cx.theme().colors();
        Self {
            pill_bg: colors.elevated_surface_background.opacity(0.86),
            pill_border: colors.border.opacity(0.85),
            active_bg: colors.element_active.opacity(0.65),
            hover_bg: colors.element_hover.opacity(0.82),
            active_icon: colors.text,
            disabled_icon: colors.text_muted.opacity(0.45),
            popover_ring: colors.border_focused,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickLayer {
    Atmosphere,
    Ground,
    Horizon,
    HipsSurvey,
    Stars,
    DeepSky,
    SolarSystem,
    AltAzGrid,
    EquatorialGrid,
    Trajectories,
    SkyLimits,
}

impl QuickLayer {
    pub const SKY_LAYERS: [Self; 7] = [
        Self::Atmosphere,
        Self::Ground,
        Self::Horizon,
        Self::HipsSurvey,
        Self::Stars,
        Self::DeepSky,
        Self::SolarSystem,
    ];
    pub const OVERLAY_LAYERS: [Self; 3] =
        [Self::AltAzGrid, Self::EquatorialGrid, Self::Trajectories];
    pub const MOUNT_LAYERS: [Self; 1] = [Self::SkyLimits];

    pub fn key(self) -> &'static str {
        match self {
            Self::Atmosphere => "atmosphere",
            Self::Ground => "ground",
            Self::Horizon => "horizon",
            Self::HipsSurvey => "hips",
            Self::Stars => "stars",
            Self::DeepSky => "deep_sky",
            Self::SolarSystem => "solar_system",
            Self::AltAzGrid => "altaz_grid",
            Self::EquatorialGrid => "eq_grid",
            Self::Trajectories => "trajectories",
            Self::SkyLimits => "sky_limits",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Atmosphere => "Sky Atmosphere",
            Self::Ground => "Ground Floor",
            Self::Horizon => "Custom Horizon",
            Self::HipsSurvey => "HiPS Survey",
            Self::Stars => "Stars",
            Self::DeepSky => "Deep Sky (M / NGC)",
            Self::SolarSystem => "Solar System",
            Self::AltAzGrid => "Alt/Az Grid",
            Self::EquatorialGrid => "Equatorial Grid",
            Self::Trajectories => "Trajectories",
            Self::SkyLimits => "Mount Limits",
        }
    }

    pub fn icon(self) -> &'static [u8] {
        match self {
            Self::Atmosphere => ICON_LAYER_ATMOSPHERE,
            Self::Ground => ICON_LAYER_GROUND,
            Self::Horizon => ICON_LAYER_HORIZON,
            Self::HipsSurvey => ICON_LAYER_HIPS,
            Self::Stars => ICON_LAYER_STARS,
            Self::DeepSky => ICON_LAYER_DSO,
            Self::SolarSystem => ICON_LAYER_SOLAR_SYSTEM,
            Self::AltAzGrid => ICON_GRID_ALTAZ,
            Self::EquatorialGrid => ICON_GRID_EQUATORIAL,
            Self::Trajectories => ICON_LAYER_TRAJECTORIES,
            Self::SkyLimits => ICON_LAYER_SKY_LIMITS,
        }
    }

    pub fn active_icon_color(self, default: Hsla) -> Hsla {
        match self {
            Self::DeepSky => rgb(0xeb8278).into(),
            Self::AltAzGrid => rgb(0x66b5f5).into(),
            Self::EquatorialGrid => rgb(0xeb9b56).into(),
            Self::Trajectories => rgb(0x5fd4c8).into(),
            Self::SolarSystem => rgb(0xf7c957).into(),
            Self::SkyLimits => rgb(0xf59447).into(),
            Self::Atmosphere | Self::Ground | Self::Horizon | Self::HipsSurvey | Self::Stars => {
                default
            }
        }
    }

    pub fn is_enabled(self, model: &SettingsModel) -> bool {
        match self {
            Self::Atmosphere => model.atmosphere_enabled(),
            Self::Ground => model.ground_enabled(),
            Self::Horizon => model.horizon_enabled(),
            Self::HipsSurvey => model.hips_survey_enabled(),
            Self::Stars => model.stars_enabled(),
            Self::DeepSky => model.dso_enabled(),
            Self::SolarSystem => model.solar_system_enabled(),
            Self::AltAzGrid => model.altaz_grid_enabled(),
            Self::EquatorialGrid => model.equatorial_grid_enabled(),
            Self::Trajectories => model.trajectories_enabled(),
            Self::SkyLimits => true,
        }
    }

    pub fn stored_opacity(self, model: &SettingsModel) -> f32 {
        match self {
            Self::Atmosphere => model.atmosphere_strength(),
            Self::Ground => model.ground_opacity(),
            Self::Horizon => model.horizon_opacity(),
            Self::HipsSurvey => model.hips_survey_opacity(),
            Self::Stars => model.stars_opacity(),
            Self::DeepSky => model.dso_opacity(),
            Self::SolarSystem => model.solar_system_opacity(),
            Self::AltAzGrid => model.altaz_grid_opacity(),
            Self::EquatorialGrid => model.equatorial_grid_opacity(),
            Self::Trajectories => model.trajectories_opacity(),
            Self::SkyLimits => 1.0,
        }
    }

    pub fn is_visible(self, model: &SettingsModel) -> bool {
        self.is_enabled(model) && self.stored_opacity(model) > 1e-3
    }

    pub fn set_enabled(
        self,
        model: &mut SettingsModel,
        enabled: bool,
        cx: &mut Context<SettingsModel>,
    ) {
        match self {
            Self::Atmosphere => model.set_atmosphere_enabled(enabled, cx),
            Self::Ground => model.set_ground_enabled(enabled, cx),
            Self::Horizon => model.set_horizon_enabled(enabled, cx),
            Self::HipsSurvey => model.set_hips_survey_enabled(enabled, cx),
            Self::Stars => model.set_stars_enabled(enabled, cx),
            Self::DeepSky => model.set_dso_enabled(enabled, cx),
            Self::SolarSystem => model.set_solar_system_enabled(enabled, cx),
            Self::AltAzGrid => model.set_altaz_grid_enabled(enabled, cx),
            Self::EquatorialGrid => model.set_equatorial_grid_enabled(enabled, cx),
            Self::Trajectories => model.set_trajectories_enabled(enabled, cx),
            Self::SkyLimits => {}
        }
    }

    pub fn toggle(self, model: &mut SettingsModel, cx: &mut Context<SettingsModel>) {
        if self.is_visible(model) {
            self.set_enabled(model, false, cx);
        } else {
            if self.stored_opacity(model) <= 1e-3 {
                self.commit_raw_opacity(model, 1.0, cx);
            }
            self.set_enabled(model, true, cx);
        }
    }

    pub fn preview_opacity(
        self,
        model: &mut SettingsModel,
        value: f32,
        cx: &mut Context<SettingsModel>,
    ) {
        if !self.is_enabled(model) && value > 1e-3 {
            self.set_enabled(model, true, cx);
        }
        match self {
            Self::Atmosphere => model.preview_atmosphere_strength(value, cx),
            Self::Ground => model.preview_ground_opacity(value, cx),
            Self::Horizon => model.preview_horizon_opacity(value, cx),
            Self::HipsSurvey => model.preview_hips_survey_opacity(value, cx),
            Self::Stars => model.preview_stars_opacity(value, cx),
            Self::DeepSky => model.preview_dso_opacity(value, cx),
            Self::SolarSystem => model.preview_solar_system_opacity(value, cx),
            Self::AltAzGrid => model.preview_altaz_grid_opacity(value, cx),
            Self::EquatorialGrid => model.preview_equatorial_grid_opacity(value, cx),
            Self::Trajectories => model.preview_trajectories_opacity(value, cx),
            Self::SkyLimits => {}
        }
    }

    pub fn commit_opacity(
        self,
        model: &mut SettingsModel,
        value: f32,
        cx: &mut Context<SettingsModel>,
    ) {
        if !self.is_enabled(model) && value > 1e-3 {
            self.set_enabled(model, true, cx);
        }
        self.commit_raw_opacity(model, value, cx);
    }

    pub fn commit_raw_opacity(
        self,
        model: &mut SettingsModel,
        value: f32,
        cx: &mut Context<SettingsModel>,
    ) {
        match self {
            Self::Atmosphere => model.commit_atmosphere_strength(value, cx),
            Self::Ground => model.commit_ground_opacity(value, cx),
            Self::Horizon => model.commit_horizon_opacity(value, cx),
            Self::HipsSurvey => model.commit_hips_survey_opacity(value, cx),
            Self::Stars => model.commit_stars_opacity(value, cx),
            Self::DeepSky => model.commit_dso_opacity(value, cx),
            Self::SolarSystem => model.commit_solar_system_opacity(value, cx),
            Self::AltAzGrid => model.commit_altaz_grid_opacity(value, cx),
            Self::EquatorialGrid => model.commit_equatorial_grid_opacity(value, cx),
            Self::Trajectories => model.commit_trajectories_opacity(value, cx),
            Self::SkyLimits => {}
        }
    }
}
