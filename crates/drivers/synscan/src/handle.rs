use protocol::{MountCommand, MountStatus, MountTelemetry};
use tokio::sync::{broadcast, mpsc, watch};

use crate::types::{MountError, MountEvent};

#[derive(Clone)]
pub struct MountHandle {
    cmd_tx: mpsc::Sender<MountCommand>,
    status_rx: watch::Receiver<MountStatus>,
    telemetry_rx: watch::Receiver<Option<MountTelemetry>>,
    event_tx: broadcast::Sender<MountEvent>,
}

impl MountHandle {
    pub fn new(
        cmd_tx: mpsc::Sender<MountCommand>,
        status_rx: watch::Receiver<MountStatus>,
        telemetry_rx: watch::Receiver<Option<MountTelemetry>>,
        event_tx: broadcast::Sender<MountEvent>,
    ) -> Self {
        Self {
            cmd_tx,
            status_rx,
            telemetry_rx,
            event_tx,
        }
    }

    /// Creates an inert/disconnected mount handle for unit tests that only exercise camera logic.
    pub fn disconnected_stub() -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let (_status_tx, status_rx) = watch::channel(MountStatus::Disconnected);
        let (_telemetry_tx, telemetry_rx) = watch::channel(None);
        let (event_tx, _) = broadcast::channel(16);
        Self {
            cmd_tx,
            status_rx,
            telemetry_rx,
            event_tx,
        }
    }

    pub async fn send_command(&self, cmd: MountCommand) -> Result<(), MountError> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| MountError::ActorDisconnected)
    }

    pub fn try_send_command(&self, cmd: MountCommand) -> Result<(), MountError> {
        self.cmd_tx.try_send(cmd).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                MountError::Io("Mount command queue is full".into())
            }
            mpsc::error::TrySendError::Closed(_) => MountError::ActorDisconnected,
        })
    }

    pub fn status(&self) -> watch::Receiver<MountStatus> {
        self.status_rx.clone()
    }

    pub fn telemetry(&self) -> watch::Receiver<Option<MountTelemetry>> {
        self.telemetry_rx.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<MountEvent> {
        self.event_tx.subscribe()
    }
}
