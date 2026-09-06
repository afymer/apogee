use protocol::camera::CameraStatus;
use tokio::sync::{mpsc, oneshot, watch};

use crate::types::{CameraCommand, CaptureError};

#[derive(Clone)]
pub struct CameraHandle {
    cmd_tx: mpsc::Sender<CameraCommand>,
    status_rx: watch::Receiver<CameraStatus>,
}

impl CameraHandle {
    pub fn new(
        cmd_tx: mpsc::Sender<CameraCommand>,
        status_rx: watch::Receiver<CameraStatus>,
    ) -> Self {
        Self { cmd_tx, status_rx }
    }

    pub async fn capture(&self, exposure_secs: u32) -> Result<(), CaptureError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CameraCommand::Capture {
            exposure_secs,
            reply: reply_tx,
        };

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| CaptureError::ActorDisconnected)?;

        match reply_rx.await {
            Ok(result) => result,
            Err(_) => Err(CaptureError::Interrupted),
        }
    }

    pub async fn wait_ready(&self) -> Result<(), CaptureError> {
        let mut rx = self.status_rx.clone();
        rx.wait_for(|status| matches!(status, CameraStatus::Ready))
            .await
            .map_err(|_| CaptureError::ActorDisconnected)?;
        Ok(())
    }
}
