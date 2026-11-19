use std::time::Duration;

use tokio::sync::{oneshot, watch};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Exposure interrupted (hardware reset in progress)")]
    Interrupted,
    #[error("Actor task crashed or channel closed")]
    ActorDisconnected,
    #[error("Fatal hardware failure: {0}")]
    Fatal(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Actor task crashed or channel closed")]
    ActorDisconnected,
    #[error("Failed to download file from camera: {0}")]
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("Actor task crashed or channel closed")]
    ActorDisconnected,
    #[error("Failed to delete file from camera: {0}")]
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraFileRef {
    pub folder: String,
    pub filename: String,
}

pub enum CameraCommand {
    Capture {
        exposure: Duration,
        iso: Option<u32>,
        cancel: watch::Receiver<bool>,
        reply: oneshot::Sender<Result<CameraFileRef, CaptureError>>,
    },
    Download {
        folder: String,
        filename: String,
        reply: oneshot::Sender<Result<Vec<u8>, DownloadError>>,
    },
    DeleteFile {
        folder: String,
        filename: String,
        reply: oneshot::Sender<Result<(), DeleteError>>,
    },
}
