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

/// Functional kind of a stage inside a target's sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StageKind {
    #[default]
    Capture,
    Autofocus,
    PlateSolve,
    Wait,
}

impl StageKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
            Self::Autofocus => "Autofocus",
            Self::PlateSolve => "Plate Solve",
            Self::Wait => "Wait",
        }
    }

    pub fn is_capture(self) -> bool {
        matches!(self, Self::Capture)
    }
}

/// A single step in a multi-frame imaging sequence (e.g. 20x 60s Light frames).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceStep {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub kind: StageKind,
    pub frame_type: FrameType,
    pub count: u32,
    pub params: CaptureParams,
    #[serde(default)]
    pub delay_between_frames: Duration,
}

impl SequenceStep {
    pub fn capture(
        frame_type: FrameType,
        count: u32,
        exposure: Duration,
        iso: Option<u32>,
        delay_between_frames: Duration,
    ) -> Self {
        Self {
            enabled: true,
            kind: StageKind::Capture,
            frame_type,
            count,
            params: CaptureParams { exposure, iso },
            delay_between_frames,
        }
    }

    pub fn autofocus() -> Self {
        Self {
            enabled: true,
            kind: StageKind::Autofocus,
            frame_type: FrameType::Light,
            count: 0,
            params: CaptureParams {
                exposure: Duration::from_secs(3),
                iso: Some(1600),
            },
            delay_between_frames: Duration::ZERO,
        }
    }

    pub fn plate_solve() -> Self {
        Self {
            enabled: true,
            kind: StageKind::PlateSolve,
            frame_type: FrameType::Light,
            count: 0,
            params: CaptureParams {
                exposure: Duration::from_secs(5),
                iso: Some(1600),
            },
            delay_between_frames: Duration::ZERO,
        }
    }

    pub fn wait(duration: Duration) -> Self {
        Self {
            enabled: true,
            kind: StageKind::Wait,
            frame_type: FrameType::Light,
            count: 0,
            params: CaptureParams {
                exposure: Duration::from_secs(1),
                iso: None,
            },
            delay_between_frames: duration,
        }
    }

    pub fn stage_label(&self) -> &'static str {
        match self.kind {
            StageKind::Capture => self.frame_type.label(),
            StageKind::Autofocus => "Autofocus",
            StageKind::PlateSolve => "Plate Solve",
            StageKind::Wait => "Wait",
        }
    }

    pub fn frame_count(&self) -> u32 {
        if self.enabled && self.kind.is_capture() {
            self.count
        } else {
            0
        }
    }

    /// Total expected execution time for this step (zero when disabled or non-timed).
    pub fn estimated_duration(&self) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }
        match self.kind {
            StageKind::Capture => {
                if self.count == 0 {
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
            StageKind::Autofocus | StageKind::PlateSolve => Duration::ZERO,
            StageKind::Wait => self.delay_between_frames,
        }
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
            .fold(0u32, |acc, step| acc.saturating_add(step.frame_count()))
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
