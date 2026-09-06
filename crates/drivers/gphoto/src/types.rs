use tokio::sync::oneshot;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Exposure interrupted (hardware reset in progress)")]
    Interrupted,
    #[error("Actor task crashed or channel closed")]
    ActorDisconnected,
    #[error("Fatal hardware failure: {0}")]
    Fatal(String),
}

pub enum CameraCommand {
    Capture {
        exposure_secs: u32,
        reply: oneshot::Sender<Result<(), CaptureError>>,
    },
}
