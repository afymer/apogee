use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraStatus {
    Disconnected,
    Ready,
    Recovering { attempt: u32, reason: String },
    Fatal(String),
}
