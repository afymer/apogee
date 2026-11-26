pub mod capture;
pub mod controller_handle;
pub mod controller_job;
pub mod sequence;

#[cfg(test)]
mod tests;

use gphoto::handle::CameraHandle;
use protocol::{
    DaemonStatus, Frame, ImagingSequence, NightPlan, NightPlanRunRequest, SequencePhase,
    SequenceStatus,
};
use synscan::MountHandle;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, info, warn};

pub use crate::capture::{
    CapturedFrame, delete_camera_file, download_captured_file, execute_capture,
    execute_capture_with_cancel, should_delete_from_camera, should_download,
    trigger_camera_exposure, trigger_camera_exposure_with_cancel,
};
pub use crate::controller_handle::{ControllerCommand, ControllerError, ControllerHandle};
pub use crate::controller_job::ControllerJob;
pub use crate::sequence::{execute_night_plan, execute_sequence, needs_meridian_flip};

#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Status(DaemonStatus),
    FrameCaptured(capture::CapturedFrame),
    CaptureError { frame_id: u64, error: String },
    SequenceStatus(SequenceStatus),
}

enum InternalMessage {
    CaptureFinished {
        job_id: u64,
        frame_id: u64,
        result: Result<capture::CapturedFrame, capture::CaptureExecutionError>,
    },
    SequenceFinished {
        job_id: u64,
    },
}

pub struct Controller {
    camera: CameraHandle,
    mount: MountHandle,
    cmd_rx: mpsc::Receiver<ControllerCommand>,
    internal_tx: mpsc::Sender<InternalMessage>,
    internal_rx: mpsc::Receiver<InternalMessage>,
    status_tx: watch::Sender<DaemonStatus>,
    sequence_status_tx: watch::Sender<Option<SequenceStatus>>,
    event_tx: broadcast::Sender<ControllerEvent>,
    current_job: Option<ControllerJob>,
    active_job_id: u64,
    current_cancel_tx: Option<watch::Sender<bool>>,
    current_pause_tx: Option<watch::Sender<bool>>,
    current_plan_tx: Option<watch::Sender<Option<NightPlan>>>,
    current_abort_handle: Option<tokio::task::AbortHandle>,
}

impl Controller {
    pub fn spawn(camera: CameraHandle) -> ControllerHandle {
        Self::spawn_with_mount(camera, MountHandle::disconnected_stub())
    }

    pub fn spawn_with_mount(camera: CameraHandle, mount: MountHandle) -> ControllerHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (internal_tx, internal_rx) = mpsc::channel(32);
        let (status_tx, status_rx) = watch::channel(DaemonStatus::Idle);
        let (sequence_status_tx, sequence_status_rx) = watch::channel(None);
        let (event_tx, _) = broadcast::channel(128);

        let camera_status_rx = camera.status();
        let camera_capabilities_rx = camera.capabilities();

        let handle = ControllerHandle::new(
            cmd_tx,
            status_rx,
            camera_status_rx,
            camera_capabilities_rx,
            sequence_status_rx,
            mount.clone(),
            event_tx.clone(),
        );

        let controller = Self {
            camera,
            mount,
            cmd_rx,
            internal_tx,
            internal_rx,
            status_tx,
            sequence_status_tx,
            event_tx,
            current_job: None,
            active_job_id: 0,
            current_cancel_tx: None,
            current_pause_tx: None,
            current_plan_tx: None,
            current_abort_handle: None,
        };

        tokio::spawn(async move { controller.run().await });

        handle
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(ControllerCommand::StartCapture(frame)) => {
                            self.handle_start_capture(&frame);
                        }
                        Some(ControllerCommand::StartSequence(sequence)) => {
                            self.handle_start_sequence(sequence);
                        }
                        Some(ControllerCommand::StartNightPlan(req)) => {
                            self.handle_start_night_plan(req);
                        }
                        Some(ControllerCommand::UpdateNightPlan(plan)) => {
                            self.handle_update_night_plan(plan);
                        }
                        Some(ControllerCommand::PauseSequence) => {
                            self.handle_pause_sequence();
                        }
                        Some(ControllerCommand::ResumeSequence) => {
                            self.handle_resume_sequence();
                        }
                        Some(ControllerCommand::Abort) => {
                            self.handle_abort();
                        }
                        Some(ControllerCommand::Mount(mount_cmd)) => {
                            if let Err(err) = self.mount.try_send_command(mount_cmd) {
                                error!("Failed to forward mount command: {err}");
                            }
                        }
                        None => {
                            info!("Controller command channel closed, shutting down controller");
                            if let Some(cancel_tx) = self.current_cancel_tx.take() {
                                let _ = cancel_tx.send(true);
                            }
                            if let Some(abort) = self.current_abort_handle.take() {
                                abort.abort();
                            }
                            break;
                        }
                    }
                }
                internal_msg = self.internal_rx.recv() => {
                    match internal_msg {
                        Some(InternalMessage::CaptureFinished { job_id, frame_id, result }) => {
                            self.handle_capture_finished(job_id, frame_id, result);
                        }
                        Some(InternalMessage::SequenceFinished { job_id }) => {
                            self.handle_sequence_finished(job_id);
                        }
                        None => {}
                    }
                }
            }
        }
    }

    fn handle_start_capture(&mut self, frame: &Frame) {
        info!(
            "Controller received StartCapture for frame {} ({:?}, {:.1}s)",
            frame.id,
            frame.frame_type,
            frame.params.exposure.as_secs_f32()
        );

        if *self.status_tx.borrow() == DaemonStatus::Capturing {
            info!(
                "Rejecting capture for frame {}: capture is already in progress",
                frame.id
            );
            let _ = self.event_tx.send(ControllerEvent::CaptureError {
                frame_id: frame.id,
                error: "Capture is already in progress".into(),
            });
            return;
        }

        self.active_job_id = self.active_job_id.wrapping_add(1);
        let job_id = self.active_job_id;
        let (cancel_tx, cancel_rx) = watch::channel(false);

        self.set_status(DaemonStatus::Capturing);
        self.current_job = Some(ControllerJob::Capture(frame.clone()));
        self.current_cancel_tx = Some(cancel_tx);
        self.current_pause_tx = None;
        self.current_plan_tx = None;

        let camera = self.camera.clone();
        let internal_tx = self.internal_tx.clone();
        let frame_id = frame.id;
        let capture_frame = frame.clone();

        let task = tokio::spawn(async move {
            let res = execute_capture_with_cancel(&camera, &capture_frame, cancel_rx).await;
            let _ = internal_tx
                .send(InternalMessage::CaptureFinished {
                    job_id,
                    frame_id,
                    result: res,
                })
                .await;
        });

        self.current_abort_handle = Some(task.abort_handle());
    }

    fn handle_start_sequence(&mut self, sequence: ImagingSequence) {
        let total = sequence.total_frames();
        info!(
            "Controller received StartSequence {} (target='{}', {} steps, {} frames)",
            sequence.id,
            sequence.target_name,
            sequence.steps.len(),
            total
        );

        if *self.status_tx.borrow() == DaemonStatus::Capturing {
            info!(
                "Rejecting sequence {}: capture or sequence is already in progress",
                sequence.id
            );
            let _ = self.event_tx.send(ControllerEvent::CaptureError {
                frame_id: sequence.id,
                error: "Capture is already in progress".into(),
            });
            return;
        }

        self.active_job_id = self.active_job_id.wrapping_add(1);
        let job_id = self.active_job_id;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (pause_tx, pause_rx) = watch::channel(false);

        self.set_status(DaemonStatus::Capturing);
        self.current_job = Some(ControllerJob::Sequence(sequence.clone()));
        self.current_cancel_tx = Some(cancel_tx);
        self.current_pause_tx = Some(pause_tx);
        self.current_plan_tx = None;

        let camera = self.camera.clone();
        let mount = self.mount.clone();
        let status_tx = self.sequence_status_tx.clone();
        let event_tx = self.event_tx.clone();
        let internal_tx = self.internal_tx.clone();

        let task = tokio::spawn(async move {
            let _ = execute_sequence(
                camera, mount, sequence, cancel_rx, pause_rx, status_tx, event_tx,
            )
            .await;
            let _ = internal_tx
                .send(InternalMessage::SequenceFinished { job_id })
                .await;
        });

        self.current_abort_handle = Some(task.abort_handle());
    }

    fn handle_start_night_plan(&mut self, req: NightPlanRunRequest) {
        info!(
            "Controller received StartNightPlan {} (id='{}', {} targets, immediate={})",
            req.run_id,
            req.plan.id,
            req.plan.targets.len(),
            req.start_immediately
        );

        if *self.status_tx.borrow() == DaemonStatus::Capturing {
            info!(
                "Rejecting NightPlan {}: capture or sequence is already in progress",
                req.run_id
            );
            let _ = self.event_tx.send(ControllerEvent::CaptureError {
                frame_id: req.run_id,
                error: "Capture is already in progress".into(),
            });
            return;
        }

        self.active_job_id = self.active_job_id.wrapping_add(1);
        let job_id = self.active_job_id;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (pause_tx, pause_rx) = watch::channel(false);
        let (plan_tx, plan_rx) = watch::channel(None);

        self.set_status(DaemonStatus::Capturing);
        self.current_job = Some(ControllerJob::NightPlan(req.clone()));
        self.current_cancel_tx = Some(cancel_tx);
        self.current_pause_tx = Some(pause_tx);
        self.current_plan_tx = Some(plan_tx);

        let camera = self.camera.clone();
        let mount = self.mount.clone();
        let status_tx = self.sequence_status_tx.clone();
        let event_tx = self.event_tx.clone();
        let internal_tx = self.internal_tx.clone();

        let task = tokio::spawn(async move {
            let _ = execute_night_plan(
                camera, mount, req, cancel_rx, pause_rx, plan_rx, status_tx, event_tx,
            )
            .await;
            let _ = internal_tx
                .send(InternalMessage::SequenceFinished { job_id })
                .await;
        });

        self.current_abort_handle = Some(task.abort_handle());
    }

    fn handle_update_night_plan(&mut self, plan: NightPlan) {
        let is_night_plan = matches!(self.current_job, Some(ControllerJob::NightPlan(_)));
        let is_paused = self
            .current_pause_tx
            .as_ref()
            .is_some_and(|tx| *tx.borrow());
        if !is_night_plan || !is_paused {
            warn!("Ignoring UpdateNightPlan: active NightPlan must be paused to apply edits");
            return;
        }
        if let Some(plan_tx) = &self.current_plan_tx {
            info!("Applying paused NightPlan update (id='{}')", plan.id);
            let _ = plan_tx.send(Some(plan));
        }
    }

    fn handle_pause_sequence(&mut self) {
        if let Some(pause_tx) = &self.current_pause_tx {
            info!("Pausing active imaging sequence");
            let _ = pause_tx.send(true);
        }
    }

    fn handle_resume_sequence(&mut self) {
        if let Some(pause_tx) = &self.current_pause_tx {
            info!("Resuming active imaging sequence");
            let _ = pause_tx.send(false);
        }
    }

    fn handle_abort(&mut self) {
        warn!("Controller received Abort command");
        self.active_job_id = self.active_job_id.wrapping_add(1);
        let was_sequence = matches!(
            self.current_job,
            Some(ControllerJob::Sequence(_) | ControllerJob::NightPlan(_))
        );
        if let Some(cancel_tx) = self.current_cancel_tx.take() {
            let _ = cancel_tx.send(true);
        }
        self.current_pause_tx = None;
        self.current_plan_tx = None;
        if let Some(abort) = self.current_abort_handle.take() {
            abort.abort();
            info!("Aborted running capture task");
        }
        self.current_job = None;
        if was_sequence {
            let current = self.sequence_status_tx.borrow().clone();
            if let Some(mut st) = current {
                if st.phase.is_active() {
                    st.phase = SequencePhase::Aborted;
                    st.current_frame_id = None;
                    st.waiting_until_unix_secs = None;
                    st.message = Some("Sequence aborted".to_string());
                    let _ = self.sequence_status_tx.send(Some(st.clone()));
                    let _ = self.event_tx.send(ControllerEvent::SequenceStatus(st));
                }
            }
        }
        self.set_status(DaemonStatus::Idle);
    }

    fn handle_sequence_finished(&mut self, job_id: u64) {
        if job_id != self.active_job_id || self.current_job.is_none() {
            debug!(
                "Ignoring stale sequence completion (job {job_id}, active {})",
                self.active_job_id
            );
            return;
        }
        self.current_abort_handle = None;
        self.current_cancel_tx = None;
        self.current_pause_tx = None;
        self.current_plan_tx = None;
        self.current_job = None;
        self.set_status(DaemonStatus::Idle);
    }

    fn handle_capture_finished(
        &mut self,
        job_id: u64,
        frame_id: u64,
        result: Result<capture::CapturedFrame, capture::CaptureExecutionError>,
    ) {
        if job_id != self.active_job_id || self.current_job.is_none() {
            debug!(
                "Ignoring stale capture completion for frame {frame_id} (job {job_id}, active {})",
                self.active_job_id
            );
            return;
        }

        self.current_abort_handle = None;
        self.current_cancel_tx = None;
        self.current_pause_tx = None;
        self.current_plan_tx = None;
        self.current_job = None;

        match result {
            Ok(captured_frame) => {
                info!("Capture completed successfully for frame {frame_id}");
                let _ = self
                    .event_tx
                    .send(ControllerEvent::FrameCaptured(captured_frame));
            }
            Err(err) => {
                error!("Capture failed for frame {frame_id}: {err}");
                let _ = self.event_tx.send(ControllerEvent::CaptureError {
                    frame_id,
                    error: err.to_string(),
                });
            }
        }

        self.set_status(DaemonStatus::Idle);
    }

    fn set_status(&mut self, status: DaemonStatus) {
        if *self.status_tx.borrow() != status {
            let _ = self.status_tx.send(status.clone());
            let _ = self.event_tx.send(ControllerEvent::Status(status));
        }
    }
}
