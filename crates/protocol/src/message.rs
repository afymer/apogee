use serde::{Deserialize, Serialize};

use crate::camera::CameraStatus;
use crate::capture::Frame;
use crate::mount::{MountCommand, MountStatus, MountTelemetry};

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
    Abort,
    Mount(MountCommand),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum DaemonMessage {
    Status(DaemonStatus),
    CameraStatus(CameraStatus),
    MountStatus(MountStatus),
    MountTelemetry(MountTelemetry),
    MountPorts(Vec<String>),
    MountError { message: String },
    SequenceProgress { completed: u32, total: u32 },
    PreviewAvailable { frame_id: u64, image_bytes: Vec<u8> },
    FrameReady { frame_id: u64, url: String },
    CaptureError { frame_id: u64, message: String },
}
