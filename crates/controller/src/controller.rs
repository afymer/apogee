pub mod capture;
pub mod controller_handle;
pub mod controller_job;

#[cfg(test)]
mod tests;

use gphoto::handle::CameraHandle;
use protocol::{DaemonStatus, Frame};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};

pub use crate::capture::{
    CapturedFrame, delete_camera_file, download_captured_file, execute_capture,
    should_delete_from_camera, should_download, trigger_camera_exposure,
};
pub use crate::controller_handle::{ControllerCommand, ControllerError, ControllerHandle};
pub use crate::controller_job::ControllerJob;

#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Status(DaemonStatus),
    FrameCaptured(capture::CapturedFrame),
    CaptureError { frame_id: u64, error: String },
}

enum InternalMessage {
    CaptureFinished {
        frame_id: u64,
        result: Result<capture::CapturedFrame, capture::CaptureExecutionError>,
    },
}

pub struct Controller {
    camera: CameraHandle,
    cmd_rx: mpsc::Receiver<ControllerCommand>,
    internal_tx: mpsc::Sender<InternalMessage>,
    internal_rx: mpsc::Receiver<InternalMessage>,
    status_tx: watch::Sender<DaemonStatus>,
    event_tx: broadcast::Sender<ControllerEvent>,
    #[allow(dead_code)]
    current_job: Option<ControllerJob>,
    current_abort_handle: Option<tokio::task::AbortHandle>,
}

impl Controller {
    pub fn spawn(camera: CameraHandle) -> ControllerHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (internal_tx, internal_rx) = mpsc::channel(32);
        let (status_tx, status_rx) = watch::channel(DaemonStatus::Idle);
        let (event_tx, _) = broadcast::channel(128);

        let camera_status_rx = camera.status();

        let handle = ControllerHandle::new(cmd_tx, status_rx, camera_status_rx, event_tx.clone());

        let controller = Self {
            camera,
            cmd_rx,
            internal_tx,
            internal_rx,
            status_tx,
            event_tx,
            current_job: None,
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
                        Some(ControllerCommand::Abort) => {
                            self.handle_abort();
                        }
                        None => {
                            info!("Controller command channel closed, shutting down controller");
                            if let Some(abort) = self.current_abort_handle.take() {
                                abort.abort();
                            }
                            break;
                        }
                    }
                }
                internal_msg = self.internal_rx.recv() => {
                    if let Some(InternalMessage::CaptureFinished { frame_id, result }) = internal_msg {
                        self.handle_capture_finished(frame_id, result);
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

        self.set_status(DaemonStatus::Capturing);
        self.current_job = Some(ControllerJob::Capture(frame.clone()));

        let camera = self.camera.clone();
        let internal_tx = self.internal_tx.clone();
        let frame_id = frame.id;
        let capture_frame = frame.clone();

        let task = tokio::spawn(async move {
            let res = execute_capture(&camera, &capture_frame).await;
            let _ = internal_tx
                .send(InternalMessage::CaptureFinished {
                    frame_id,
                    result: res,
                })
                .await;
        });

        self.current_abort_handle = Some(task.abort_handle());
    }

    fn handle_abort(&mut self) {
        warn!("Controller received Abort command");
        if let Some(abort) = self.current_abort_handle.take() {
            abort.abort();
            info!("Aborted running capture task");
        }
        self.current_job = None;
        self.set_status(DaemonStatus::Idle);
    }

    fn handle_capture_finished(
        &mut self,
        frame_id: u64,
        result: Result<capture::CapturedFrame, capture::CaptureExecutionError>,
    ) {
        self.current_abort_handle = None;
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
