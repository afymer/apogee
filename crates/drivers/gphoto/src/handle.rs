use protocol::camera::{CameraCapabilities, CameraStatus};
use tokio::sync::{mpsc, oneshot, watch};

use crate::types::{CameraCommand, CameraFileRef, CaptureError, DeleteError, DownloadError};

#[derive(Clone)]
pub struct CameraHandle {
    cmd_tx: mpsc::Sender<CameraCommand>,
    status_rx: watch::Receiver<CameraStatus>,
    capabilities_rx: watch::Receiver<Option<CameraCapabilities>>,
}

impl CameraHandle {
    pub fn new(
        cmd_tx: mpsc::Sender<CameraCommand>,
        status_rx: watch::Receiver<CameraStatus>,
    ) -> Self {
        let (_caps_tx, capabilities_rx) = watch::channel(None);
        Self::with_capabilities(cmd_tx, status_rx, capabilities_rx)
    }

    pub fn with_capabilities(
        cmd_tx: mpsc::Sender<CameraCommand>,
        status_rx: watch::Receiver<CameraStatus>,
        capabilities_rx: watch::Receiver<Option<CameraCapabilities>>,
    ) -> Self {
        Self {
            cmd_tx,
            status_rx,
            capabilities_rx,
        }
    }

    pub async fn capture(
        &self,
        exposure: std::time::Duration,
        iso: Option<u32>,
    ) -> Result<CameraFileRef, CaptureError> {
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        self.capture_with_cancel(exposure, iso, cancel_rx).await
    }

    pub async fn capture_with_cancel(
        &self,
        exposure: std::time::Duration,
        iso: Option<u32>,
        cancel: watch::Receiver<bool>,
    ) -> Result<CameraFileRef, CaptureError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CameraCommand::Capture {
            exposure,
            iso,
            cancel,
            reply: reply_tx,
        };

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| CaptureError::ActorDisconnected)?;

        let timeout_duration = exposure
            .saturating_add(std::time::Duration::from_secs(35))
            .max(std::time::Duration::from_secs(40));

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

        let timeout_duration = std::time::Duration::from_secs(65);
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

        let timeout_duration = std::time::Duration::from_secs(15);
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

    pub fn capabilities(&self) -> watch::Receiver<Option<CameraCapabilities>> {
        self.capabilities_rx.clone()
    }
}
