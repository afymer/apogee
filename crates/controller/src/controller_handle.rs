use protocol::{
    CameraStatus, DaemonStatus, Frame, ImagingSequence, MountCommand, MountStatus, MountTelemetry,
    NightPlan, NightPlanRunRequest, SequenceStatus,
};
use synscan::{MountEvent, MountHandle};
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
    StartSequence(ImagingSequence),
    StartNightPlan(NightPlanRunRequest),
    UpdateNightPlan(NightPlan),
    PauseSequence,
    ResumeSequence,
    Abort,
    Mount(MountCommand),
}

#[derive(Clone)]
pub struct ControllerHandle {
    cmd_tx: mpsc::Sender<ControllerCommand>,
    daemon_status_rx: watch::Receiver<DaemonStatus>,
    camera_status_rx: watch::Receiver<CameraStatus>,
    sequence_status_rx: watch::Receiver<Option<SequenceStatus>>,
    mount: MountHandle,
    event_tx: broadcast::Sender<ControllerEvent>,
}

impl ControllerHandle {
    pub(crate) fn new(
        cmd_tx: mpsc::Sender<ControllerCommand>,
        daemon_status_rx: watch::Receiver<DaemonStatus>,
        camera_status_rx: watch::Receiver<CameraStatus>,
        sequence_status_rx: watch::Receiver<Option<SequenceStatus>>,
        mount: MountHandle,
        event_tx: broadcast::Sender<ControllerEvent>,
    ) -> Self {
        Self {
            cmd_tx,
            daemon_status_rx,
            camera_status_rx,
            sequence_status_rx,
            mount,
            event_tx,
        }
    }

    pub async fn start_capture(&self, frame: Frame) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::StartCapture(frame))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn start_sequence(&self, sequence: ImagingSequence) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::StartSequence(sequence))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn start_night_plan(
        &self,
        request: NightPlanRunRequest,
    ) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::StartNightPlan(request))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn update_night_plan(&self, plan: NightPlan) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::UpdateNightPlan(plan))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn pause_sequence(&self) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::PauseSequence)
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn resume_sequence(&self) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::ResumeSequence)
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn abort(&self) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::Abort)
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub async fn send_mount_command(&self, cmd: MountCommand) -> Result<(), ControllerError> {
        self.cmd_tx
            .send(ControllerCommand::Mount(cmd))
            .await
            .map_err(|_| ControllerError::Disconnected)
    }

    pub fn status(&self) -> watch::Receiver<DaemonStatus> {
        self.daemon_status_rx.clone()
    }

    pub fn camera_status(&self) -> watch::Receiver<CameraStatus> {
        self.camera_status_rx.clone()
    }

    pub fn sequence_status(&self) -> watch::Receiver<Option<SequenceStatus>> {
        self.sequence_status_rx.clone()
    }

    pub fn mount_status(&self) -> watch::Receiver<MountStatus> {
        self.mount.status()
    }

    pub fn mount_telemetry(&self) -> watch::Receiver<Option<MountTelemetry>> {
        self.mount.telemetry()
    }

    pub fn subscribe_mount_events(&self) -> broadcast::Receiver<MountEvent> {
        self.mount.subscribe_events()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ControllerEvent> {
        self.event_tx.subscribe()
    }
}
