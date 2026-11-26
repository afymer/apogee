use serde::{Deserialize, Serialize};

use crate::camera::{CameraCapabilities, CameraStatus};
use crate::capture::{Frame, ImagingSequence, SequenceStatus};
use crate::mount::{MountCommand, MountStatus, MountTelemetry};
use crate::plan::{NightPlan, NightPlanRunRequest};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonStatus {
    Idle,
    Capturing,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ViewerMessage {
    Connect,
    StartCapture(Frame),
    StartSequence(ImagingSequence),
    StartNightPlan(NightPlanRunRequest),
    UpdateNightPlan(NightPlan),
    PauseSequence,
    ResumeSequence,
    Abort,
    Mount(MountCommand),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum DaemonMessage {
    Status(DaemonStatus),
    CameraStatus(CameraStatus),
    CameraCapabilities(Option<CameraCapabilities>),
    MountStatus(MountStatus),
    MountTelemetry(MountTelemetry),
    MountPorts(Vec<String>),
    MountError { message: String },
    SequenceProgress { completed: u32, total: u32 },
    SequenceStatus(SequenceStatus),
    PreviewAvailable { frame_id: u64, image_bytes: Vec<u8> },
    FrameReady { frame_id: u64, url: String },
    CaptureError { frame_id: u64, message: String },
}
