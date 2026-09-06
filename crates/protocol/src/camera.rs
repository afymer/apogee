use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CameraStatus {
    Disconnected,
    Ready,
    Recovering { attempt: u32, reason: String },
    Fatal(String),
}
