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
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub frame_type: FrameType,
    pub count: u32,
    pub params: CaptureParams,
    #[serde(default)]
    pub delay_between_frames: Duration,
}

impl SequenceStep {
    /// Total expected execution time for this step (zero when disabled or count is 0).
    pub fn estimated_duration(&self) -> Duration {
        if !self.enabled || self.count == 0 {
            return Duration::ZERO;
        }
        self.params
            .exposure
            .saturating_mul(self.count)
            .saturating_add(
                self.delay_between_frames
                    .saturating_mul(self.count.saturating_sub(1)),
            )
    }
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
            .filter(|step| step.enabled)
            .fold(0u32, |acc, step| acc.saturating_add(step.count))
    }
}

/// Execution phase of an imaging sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SequencePhase {
    #[default]
    Idle,
    WaitingForStart,
    SlewingToTarget,
    Running,
    Paused,
    WaitingForMeridianFlip,
    Parking,
    Completed,
    Aborted,
    Failed,
}

impl SequencePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::WaitingForStart => "Waiting",
            Self::SlewingToTarget => "Slewing",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::WaitingForMeridianFlip => "Meridian Flip",
            Self::Parking => "Parking",
            Self::Completed => "Completed",
            Self::Aborted => "Aborted",
            Self::Failed => "Failed",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::WaitingForStart
                | Self::SlewingToTarget
                | Self::Running
                | Self::Paused
                | Self::WaitingForMeridianFlip
                | Self::Parking
        )
    }
}

/// Live progress snapshot of the active or most recent imaging sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceStatus {
    pub sequence_id: u64,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub current_target_id: Option<String>,
    #[serde(default)]
    pub current_target_index: u32,
    #[serde(default)]
    pub total_targets: u32,
    #[serde(default)]
    pub waiting_until_unix_secs: Option<i64>,
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
