use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::store::Persistable;

/// Current schema version for the persisted [`AppSettings`] document.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Default normalized strength for the day/twilight sky atmosphere shader.
pub const DEFAULT_ATMOSPHERE_STRENGTH: f32 = 1.0;

/// Default normalized opacity for the below-horizon ground hemisphere mask.
pub const DEFAULT_GROUND_OPACITY: f32 = 1.0;

/// Default normalized opacity for the custom 360° horizon and background layer.
pub const DEFAULT_HORIZON_OPACITY: f32 = 1.0;

/// Default normalized opacity for the HiPS sky survey layer.
pub const DEFAULT_HIPS_SURVEY_OPACITY: f32 = 1.0;

/// Default normalized opacity for the catalog stars layer.
pub const DEFAULT_STARS_OPACITY: f32 = 1.0;

/// Default normalized opacity for the deep-sky (Messier / NGC) catalog layer.
pub const DEFAULT_DSO_OPACITY: f32 = 1.0;

/// Default normalized opacity for the solar-system bodies layer (Sun and Moon).
pub const DEFAULT_SOLAR_SYSTEM_OPACITY: f32 = 1.0;

/// Default normalized opacity for coordinate grid overlays (Alt/Az and Equatorial).
pub const DEFAULT_GRID_OPACITY: f32 = 1.0;

/// Default normalized opacity for selected-object trajectory curves and event markers.
pub const DEFAULT_TRAJECTORIES_OPACITY: f32 = 1.0;

fn default_schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

fn default_true() -> bool {
    true
}

fn default_atmosphere_strength() -> f32 {
    DEFAULT_ATMOSPHERE_STRENGTH
}

fn default_ground_opacity() -> f32 {
    DEFAULT_GROUND_OPACITY
}

fn default_horizon_opacity() -> f32 {
    DEFAULT_HORIZON_OPACITY
}

fn default_hips_survey_opacity() -> f32 {
    DEFAULT_HIPS_SURVEY_OPACITY
}

fn default_stars_opacity() -> f32 {
    DEFAULT_STARS_OPACITY
}

fn default_dso_opacity() -> f32 {
    DEFAULT_DSO_OPACITY
}

fn default_solar_system_opacity() -> f32 {
    DEFAULT_SOLAR_SYSTEM_OPACITY
}

fn default_grid_opacity() -> f32 {
    DEFAULT_GRID_OPACITY
}

fn default_trajectories_opacity() -> f32 {
    DEFAULT_TRAJECTORIES_OPACITY
}

fn sanitize_unit_interval(value: &mut f32, default: f32) {
    if value.is_finite() {
        *value = value.clamp(0.0, 1.0);
    } else {
        *value = default;
    }
}

fn effective_unit_interval(enabled: bool, value: f32, default: f32) -> f32 {
    if !enabled {
        0.0
    } else if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
    }
}

/// Atmospheric sky rendering settings for daylight and twilight in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AtmosphereSettings {
    /// Whether day/twilight atmosphere scattering is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized atmosphere intensity multiplier in `[0.0, 1.0]`.
    #[serde(default = "default_atmosphere_strength")]
    pub strength: f32,
}

impl Default for AtmosphereSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: DEFAULT_ATMOSPHERE_STRENGTH,
        }
    }
}

impl AtmosphereSettings {
    /// Clamps finite strength into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.strength, DEFAULT_ATMOSPHERE_STRENGTH);
    }

    /// Returns the effective strength in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_strength(&self) -> f32 {
        effective_unit_interval(self.enabled, self.strength, DEFAULT_ATMOSPHERE_STRENGTH)
    }
}

/// Below-horizon ground semi-sphere mask settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundSettings {
    /// Whether the below-horizon ground mask is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized ground mask opacity in `[0.0, 1.0]`.
    #[serde(default = "default_ground_opacity")]
    pub opacity: f32,
}

impl Default for GroundSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_GROUND_OPACITY,
        }
    }
}

impl GroundSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_GROUND_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_GROUND_OPACITY)
    }
}

/// Custom 360° horizon line & panoramic background settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonLayerSettings {
    /// Whether the custom horizon & background panorama layer is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized custom horizon layer opacity in `[0.0, 1.0]`.
    #[serde(default = "default_horizon_opacity")]
    pub opacity: f32,
    /// Whether to draw the extracted horizon contour line over a 360° panoramic image.
    #[serde(default)]
    pub show_line: bool,
}

impl Default for HorizonLayerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_HORIZON_OPACITY,
            show_line: false,
        }
    }
}

impl HorizonLayerSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_HORIZON_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_HORIZON_OPACITY)
    }
}

/// HiPS background survey imagery settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurveySettings {
    /// Whether HiPS survey tile fetching and rendering is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized HiPS survey layer opacity in `[0.0, 1.0]`.
    #[serde(default = "default_hips_survey_opacity")]
    pub opacity: f32,
}

impl Default for SurveySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_HIPS_SURVEY_OPACITY,
        }
    }
}

impl SurveySettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_HIPS_SURVEY_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_HIPS_SURVEY_OPACITY)
    }
}

/// Catalog point-star rendering settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StarsSettings {
    /// Whether catalog star points are displayed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized star layer opacity in `[0.0, 1.0]`.
    #[serde(default = "default_stars_opacity")]
    pub opacity: f32,
}

impl Default for StarsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_STARS_OPACITY,
        }
    }
}

impl StarsSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_STARS_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_STARS_OPACITY)
    }
}

/// Deep-sky (Messier and NGC) catalog rendering settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DsoSettings {
    /// Whether deep-sky catalog markers and labels are displayed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized deep-sky layer opacity in `[0.0, 1.0]`.
    #[serde(default = "default_dso_opacity")]
    pub opacity: f32,
    /// Whether Messier objects (M1–M110) are included in the deep-sky overlay.
    #[serde(default = "default_true")]
    pub show_messier: bool,
    /// Whether NGC objects are included in the deep-sky overlay.
    #[serde(default = "default_true")]
    pub show_ngc: bool,
}

impl Default for DsoSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_DSO_OPACITY,
            show_messier: true,
            show_ngc: true,
        }
    }
}

impl DsoSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_DSO_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_DSO_OPACITY)
    }
}

/// Solar-system bodies (Sun and Moon) rendering settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolarSystemSettings {
    /// Whether solar-system objects (Sun, Moon) are displayed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized solar-system layer opacity in `[0.0, 1.0]`.
    #[serde(default = "default_solar_system_opacity")]
    pub opacity: f32,
}

impl Default for SolarSystemSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_SOLAR_SYSTEM_OPACITY,
        }
    }
}

impl SolarSystemSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_SOLAR_SYSTEM_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_SOLAR_SYSTEM_OPACITY)
    }
}

/// Coordinate grid overlay settings (horizontal Alt/Az and equatorial RA/Dec) in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridLayerSettings {
    /// Whether this coordinate grid is displayed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized grid opacity multiplier in `[0.0, 1.0]`.
    #[serde(default = "default_grid_opacity")]
    pub opacity: f32,
}

impl Default for GridLayerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_GRID_OPACITY,
        }
    }
}

impl GridLayerSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_GRID_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_GRID_OPACITY)
    }
}

/// Selected-object trajectory overlay settings in the planetarium.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrajectoriesSettings {
    /// Whether selected-object trajectory curves, ticks, and event markers are displayed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Normalized trajectory overlay opacity multiplier in `[0.0, 1.0]`.
    #[serde(default = "default_trajectories_opacity")]
    pub opacity: f32,
}

impl Default for TrajectoriesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: DEFAULT_TRAJECTORIES_OPACITY,
        }
    }
}

impl TrajectoriesSettings {
    /// Clamps finite opacity into `[0.0, 1.0]` and resets non-finite values to the default.
    pub fn sanitize(&mut self) {
        sanitize_unit_interval(&mut self.opacity, DEFAULT_TRAJECTORIES_OPACITY);
    }

    /// Returns the effective opacity in `[0.0, 1.0]`, or `0.0` when disabled.
    pub fn effective_opacity(&self) -> f32 {
        effective_unit_interval(self.enabled, self.opacity, DEFAULT_TRAJECTORIES_OPACITY)
    }
}

/// Planetarium viewport visual and rendering settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanetariumSettings {
    /// Day and twilight sky atmosphere scattering settings.
    #[serde(default)]
    pub atmosphere: AtmosphereSettings,
    /// Below-horizon ground semi-sphere mask settings.
    #[serde(default)]
    pub ground: GroundSettings,
    /// Custom 360° horizon & background panorama layer settings.
    #[serde(default)]
    pub horizon: HorizonLayerSettings,
    /// HiPS background survey imagery settings.
    #[serde(default)]
    pub hips_survey: SurveySettings,
    /// Catalog point-star rendering settings.
    #[serde(default)]
    pub stars: StarsSettings,
    /// Deep-sky (Messier / NGC) catalog rendering settings.
    #[serde(default)]
    pub deep_sky: DsoSettings,
    /// Solar-system bodies (Sun and Moon) rendering settings.
    #[serde(default)]
    pub solar_system: SolarSystemSettings,
    /// Horizontal (Azimuth / Altitude) coordinate grid settings.
    #[serde(default)]
    pub altaz_grid: GridLayerSettings,
    /// Equatorial (Right Ascension / Declination) coordinate grid settings.
    #[serde(default)]
    pub equatorial_grid: GridLayerSettings,
    /// Selected-object trajectory overlay settings.
    #[serde(default)]
    pub trajectories: TrajectoriesSettings,
}

/// Debugging and viewport diagnostic settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSettings {
    /// Whether to display the green HEALPix HiPS grid debug lines in the planetarium viewport.
    #[serde(default)]
    pub show_hips_grid: bool,
    /// Whether to show the frame time debug toggle icon in the status bar.
    #[serde(default)]
    pub show_frame_profiler: bool,
}

/// User keyboard shortcut overrides keyed by canonical action ID (e.g. `"apogee::OpenSettings"`).
///
/// - Absent entry: use the action's built-in default binding.
/// - `Some(keystrokes)`: bind the action to the serialized GPUI keystroke sequence (e.g. `"ctrl-,"`).
/// - `None`: explicitly unbound by the user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapSettings {
    #[serde(default)]
    pub overrides: BTreeMap<String, Option<String>>,
}

/// Persisted application settings document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub planetarium: PlanetariumSettings,
    #[serde(default)]
    pub debug: DebugSettings,
    #[serde(default)]
    pub keymap: KeymapSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            planetarium: PlanetariumSettings::default(),
            debug: DebugSettings::default(),
            keymap: KeymapSettings::default(),
        }
    }
}

impl Persistable for AppSettings {
    const DOCUMENT_KEY: &'static str = "settings";
    const SCHEMA_VERSION: u32 = SETTINGS_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn normalize_after_load(&mut self) {
        self.schema_version = SETTINGS_SCHEMA_VERSION;
        self.planetarium.atmosphere.sanitize();
        self.planetarium.ground.sanitize();
        self.planetarium.horizon.sanitize();
        self.planetarium.hips_survey.sanitize();
        self.planetarium.stars.sanitize();
        self.planetarium.deep_sky.sanitize();
        self.planetarium.solar_system.sanitize();
        self.planetarium.altaz_grid.sanitize();
        self.planetarium.equatorial_grid.sanitize();
        self.planetarium.trajectories.sanitize();
    }
}
