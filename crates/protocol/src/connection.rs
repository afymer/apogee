use serde::{Deserialize, Serialize};

use crate::camera::{CameraCapabilities, CameraStatus};
use crate::capture::SequenceStatus;
use crate::message::DaemonStatus;
use crate::mount::{MountStatus, MountTelemetry};

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
        next_retry_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonState {
    pub connection: ConnectionState,
    pub camera: CameraStatus,
    #[serde(default)]
    pub camera_capabilities: Option<CameraCapabilities>,
    pub mount: MountStatus,
    pub mount_telemetry: Option<MountTelemetry>,
    pub available_mount_ports: Vec<String>,
    pub daemon: DaemonStatus,
    pub sequence: Option<SequenceStatus>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            camera: CameraStatus::Disconnected,
            camera_capabilities: None,
            mount: MountStatus::Disconnected,
            mount_telemetry: None,
            available_mount_ports: Vec::new(),
            daemon: DaemonStatus::Idle,
            sequence: None,
        }
    }
}
