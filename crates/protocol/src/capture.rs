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

impl FrameType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Flat => "Flat",
            Self::Bias => "Bias",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Light, Self::Dark, Self::Flat, Self::Bias]
    }
}

fn default_true() -> bool {
    true
}

/// A single step in a multi-frame imaging sequence (e.g. 20x 60s Light frames).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceStep {
    pub frame_type: FrameType,
    pub count: u32,
    pub params: CaptureParams,
    #[serde(default)]
    pub delay_between_frames: Duration,
}

/// Multi-frame imaging sequence executed by the daemon controller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImagingSequence {
    pub id: u64,
    pub target_name: String,
    pub steps: Vec<SequenceStep>,
    pub save_config: SaveLocationConfig,
    /// When true, coordinates automatic meridian flips with the mount between Light frames.
    #[serde(default = "default_true")]
    pub auto_meridian_flip: bool,
}

impl ImagingSequence {
    pub fn total_frames(&self) -> u32 {
        self.steps
            .iter()
            .fold(0u32, |acc, step| acc.saturating_add(step.count))
    }
}

/// Execution phase of an imaging sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SequencePhase {
    #[default]
    Idle,
    Running,
    Paused,
    WaitingForMeridianFlip,
    Completed,
    Aborted,
    Failed,
}

impl SequencePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::WaitingForMeridianFlip => "Meridian Flip",
            Self::Completed => "Completed",
            Self::Aborted => "Aborted",
            Self::Failed => "Failed",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running | Self::Paused | Self::WaitingForMeridianFlip
        )
    }
}

/// Live progress snapshot of the active or most recent imaging sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceStatus {
    pub sequence_id: u64,
    pub target_name: String,
    pub phase: SequencePhase,
    pub current_step: u32,
    pub total_steps: u32,
    pub current_step_completed: u32,
    pub current_step_total: u32,
    pub completed: u32,
    pub total: u32,
    pub current_frame_type: Option<FrameType>,
    pub current_frame_id: Option<u64>,
    pub message: Option<String>,
}
