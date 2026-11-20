use serde::{Deserialize, Serialize};

use crate::store::Persistable;

/// Current schema version for the persisted [`AppSettings`] document.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Default normalized strength for the day/twilight sky atmosphere shader.
pub const DEFAULT_ATMOSPHERE_STRENGTH: f32 = 1.0;

/// Default normalized opacity for the below-horizon ground hemisphere mask.
pub const DEFAULT_GROUND_OPACITY: f32 = 1.0;

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

/// Planetarium viewport visual and rendering settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanetariumSettings {
    /// Day and twilight sky atmosphere scattering settings.
    #[serde(default)]
    pub atmosphere: AtmosphereSettings,
    /// Below-horizon ground semi-sphere mask settings.
    #[serde(default)]
    pub ground: GroundSettings,
}

/// Debugging and viewport diagnostic settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSettings {
    /// Whether to display the green HEALPix HiPS grid debug lines in the planetarium viewport.
    #[serde(default)]
    pub show_hips_grid: bool,
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            planetarium: PlanetariumSettings::default(),
            debug: DebugSettings::default(),
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
    }
}
