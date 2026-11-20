use serde::{Deserialize, Serialize};

use crate::store::Persistable;

/// Current schema version for the persisted [`AppSettings`] document.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    SETTINGS_SCHEMA_VERSION
}

/// Debugging and viewport diagnostic settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugSettings {
    /// Whether to display the green HEALPix HiPS grid debug lines in the planetarium viewport.
    #[serde(default)]
    pub show_hips_grid: bool,
}

/// Persisted application settings document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub debug: DebugSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
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
    }
}
