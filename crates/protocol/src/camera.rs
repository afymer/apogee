use serde::{Deserialize, Serialize};

pub const DEFAULT_ISO_CHOICES: &[u32] = &[100, 200, 400, 800, 1600, 3200, 6400, 12800];

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraCapabilities {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub iso_choices: Vec<u32>,
    #[serde(default = "default_true")]
    pub supports_auto_iso: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraStatus {
    Disconnected,
    Ready,
    Recovering { attempt: u32, reason: String },
    Fatal(String),
}
