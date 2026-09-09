use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameType {
    Light,
    Dark,
    Flat,
    Bias,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureParams {
    pub exposure: Duration,
    pub iso: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraSaveConfig {
    pub delete_after_download: bool,
    pub override_template: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerSaveConfig {
    pub override_template: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewerSaveConfig {
    pub override_template: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveLocationConfig {
    pub common_template: String,
    pub camera: Option<CameraSaveConfig>,
    pub server: Option<ServerSaveConfig>,
    pub viewer: Option<ViewerSaveConfig>,
}

impl SaveLocationConfig {
    pub fn save_everywhere_default(common_template: impl Into<String>) -> Self {
        Self {
            common_template: common_template.into(),
            camera: None,
            server: Some(ServerSaveConfig {
                override_template: None,
            }),
            viewer: Some(ViewerSaveConfig {
                override_template: None,
            }),
        }
    }
}

impl Default for SaveLocationConfig {
    fn default() -> Self {
        Self::save_everywhere_default("{target}_{frame_type}_{exposure_ms}ms_{timestamp}")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub id: u64,
    pub target_name: String,
    pub frame_type: FrameType,
    pub params: CaptureParams,
    pub save_config: SaveLocationConfig,
}
