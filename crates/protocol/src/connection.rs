use serde::{Deserialize, Serialize};

use crate::camera::CameraStatus;
use crate::message::DaemonStatus;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    pub connection: ConnectionState,
    pub camera: CameraStatus,
    pub daemon: DaemonStatus,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            camera: CameraStatus::Disconnected,
            daemon: DaemonStatus::Idle,
        }
    }
}
