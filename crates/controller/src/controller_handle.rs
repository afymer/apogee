use protocol::{CameraStatus, DaemonStatus, Frame};
use tokio::sync::{broadcast, mpsc, watch};

use crate::ControllerEvent;

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error("Controller task crashed or channel closed")]
    Disconnected,
}

#[derive(Debug)]
pub enum ControllerCommand {
    StartCapture(Frame),
    Abort,
}

#[derive(Clone)]
pub struct ControllerHandle {
    cmd_tx: mpsc::Sender<ControllerCommand>,
    daemon_status_rx: watch::Receiver<DaemonStatus>,
    camera_status_rx: watch::Receiver<CameraStatus>,
    event_tx: broadcast::Sender<ControllerEvent>,
}

impl ControllerHandle {
    pub(crate) fn new(
        cmd_tx: mpsc::Sender<ControllerCommand>,
        daemon_status_rx: watch::Receiver<DaemonStatus>,
        camera_status_rx: watch::Receiver<CameraStatus>,
        event_tx: broadcast::Sender<ControllerEvent>,
    ) -> Self {
        Self {
            cmd_tx,
            daemon_status_rx,
            camera_status_rx,
            event_tx,
        }
    }

    pub async fn start_capture(&self, frame: Frame) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::StartCapture(frame))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn abort(&self) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::Abort)
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub fn status(&self) -> watch::Receiver<DaemonStatus> {
        self.daemon_status_rx.clone()
    }

    pub fn camera_status(&self) -> watch::Receiver<CameraStatus> {
        self.camera_status_rx.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ControllerEvent> {
        self.event_tx.subscribe()
    }
}
