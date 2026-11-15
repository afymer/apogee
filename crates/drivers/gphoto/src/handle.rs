use protocol::camera::CameraStatus;
use tokio::sync::{mpsc, oneshot, watch};

use crate::types::{CameraCommand, CameraFileRef, CaptureError, DeleteError, DownloadError};

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

    pub async fn capture(
        &self,
        exposure: std::time::Duration,
    ) -> Result<CameraFileRef, CaptureError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CameraCommand::Capture {
            exposure,
            reply: reply_tx,
        };

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| CaptureError::ActorDisconnected)?;

        let timeout_duration = exposure
            .saturating_add(std::time::Duration::from_secs(25))
            .max(std::time::Duration::from_secs(30));

        match tokio::time::timeout(timeout_duration, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CaptureError::Interrupted),
            Err(_) => Err(CaptureError::Fatal(format!(
                "Camera actor timed out replying to capture after {timeout_duration:?}"
            ))),
        }
    }

    pub async fn download(&self, folder: &str, filename: &str) -> Result<Vec<u8>, DownloadError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CameraCommand::Download {
            folder: folder.to_string(),
            filename: filename.to_string(),
            reply: reply_tx,
        };

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| DownloadError::ActorDisconnected)?;

        let timeout_duration = std::time::Duration::from_secs(35);
        match tokio::time::timeout(timeout_duration, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(DownloadError::ActorDisconnected),
            Err(_) => Err(DownloadError::Failed("Download timed out".into())),
        }
    }

    pub async fn delete_file(&self, folder: &str, filename: &str) -> Result<(), DeleteError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CameraCommand::DeleteFile {
            folder: folder.to_string(),
            filename: filename.to_string(),
            reply: reply_tx,
        };

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| DeleteError::ActorDisconnected)?;

        let timeout_duration = std::time::Duration::from_secs(10);
        match tokio::time::timeout(timeout_duration, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(DeleteError::ActorDisconnected),
            Err(_) => Err(DeleteError::Failed("Delete file timed out".into())),
        }
    }

    pub async fn wait_ready(&self) -> Result<(), CaptureError> {
        let mut rx = self.status_rx.clone();
        rx.wait_for(|status| matches!(status, CameraStatus::Ready))
            .await
            .map_err(|_| CaptureError::ActorDisconnected)?;
        Ok(())
    }

    pub fn status(&self) -> watch::Receiver<CameraStatus> {
        self.status_rx.clone()
    }
}
