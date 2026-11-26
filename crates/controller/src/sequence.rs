use std::time::Duration;

use gphoto::handle::CameraHandle;
use protocol::{
    Frame, FrameType, ImagingSequence, MountCommand, MountStatus, MountTelemetry, PierSide,
    SequencePhase, SequenceStatus,
};
use synscan::MountHandle;
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

use crate::{ControllerEvent, execute_capture_with_cancel};

/// Returns `true` when the mount is on the West pier side (OTA West, originally looking East)
/// and has tracked past the meridian into the western sky beyond `limits.meridian_limit_deg`.
///
/// In topocentric ENU azimuth (`0 = N, 90 = E, 180 = S, 270 = W`), the western half of the sky
/// is `az_deg in (180, 360)`. An object past the southern meridian by `meridian_limit_deg` has
/// `az_deg > 180 + meridian_limit_deg` (and `< 360 - meridian_limit_deg` for northern circumpolar).
#[must_use]
pub fn needs_meridian_flip(telemetry: &MountTelemetry) -> bool {
    if !telemetry.limits.auto_meridian_flip || telemetry.is_parked {
        return false;
    }
    if telemetry.pier_side != PierSide::West {
        return false;
    }
    let az = telemetry.az_deg.rem_euclid(360.0);
    let limit = telemetry.limits.meridian_limit_deg.clamp(0.0, 45.0);
    az > 180.0 + limit && az < 360.0 - limit
}

async fn wait_for_pause_or_cancel(
    pause_rx: &mut watch::Receiver<bool>,
    cancel_rx: &mut watch::Receiver<bool>,
    status: &mut SequenceStatus,
    emit: &impl Fn(&SequenceStatus),
) -> bool {
    if *cancel_rx.borrow() {
        return true;
    }
    if !*pause_rx.borrow() {
        return false;
    }

    status.phase = SequencePhase::Paused;
    status.message = Some("Sequence paused".to_string());
    emit(status);

    while *pause_rx.borrow() {
        tokio::select! {
            res = cancel_rx.changed() => {
                if res.is_err() || *cancel_rx.borrow() {
                    return true;
                }
            }
            res = pause_rx.changed() => {
                if res.is_err() {
                    return true;
                }
            }
        }
    }

    if *cancel_rx.borrow() {
        return true;
    }

    status.phase = SequencePhase::Running;
    status.message = None;
    emit(status);
    false
}

async fn coordinate_meridian_flip(
    mount: &MountHandle,
    cancel_rx: &mut watch::Receiver<bool>,
    status: &mut SequenceStatus,
    emit: &impl Fn(&SequenceStatus),
) -> Result<(), String> {
    if *mount.status().borrow() != MountStatus::Ready {
        return Ok(());
    }

    let telemetry_opt = mount.telemetry().borrow().clone();
    let Some(telemetry) = telemetry_opt else {
        return Ok(());
    };

    if !needs_meridian_flip(&telemetry) {
        return Ok(());
    }

    info!(
        "Sequence {}: triggering auto meridian flip (az={:.1} deg, pier={:?}, limit={:.1} deg)",
        status.sequence_id,
        telemetry.az_deg,
        telemetry.pier_side,
        telemetry.limits.meridian_limit_deg
    );

    status.phase = SequencePhase::WaitingForMeridianFlip;
    status.message = Some("Performing automatic meridian flip...".to_string());
    emit(status);

    if let Err(err) = mount.send_command(MountCommand::MeridianFlip).await {
        return Err(format!("Failed to trigger meridian flip: {err}"));
    }

    // Give the mount actor a brief moment to enter slewing state, then wait until it settles.
    let mut telem_rx = mount.telemetry();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut saw_slewing = false;
    let settle_grace = tokio::time::Instant::now() + Duration::from_secs(3);

    loop {
        if *cancel_rx.borrow() {
            return Err("Cancelled during meridian flip".to_string());
        }

        if let Some(current) = telem_rx.borrow().as_ref() {
            if current.is_slewing {
                saw_slewing = true;
            } else if saw_slewing
                || current.pier_side == PierSide::East
                || tokio::time::Instant::now() >= settle_grace
            {
                break;
            }
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Meridian flip timed out after 180s".to_string());
        }

        let poll_wait = remaining.min(Duration::from_millis(250));
        tokio::select! {
            res = cancel_rx.changed() => {
                if res.is_err() || *cancel_rx.borrow() {
                    return Err("Cancelled during meridian flip".to_string());
                }
            }
            _ = telem_rx.changed() => {}
            () = tokio::time::sleep(poll_wait) => {}
        }
    }

    info!("Sequence {}: meridian flip settled", status.sequence_id);
    status.phase = SequencePhase::Running;
    status.message = None;
    emit(status);
    Ok(())
}

/// Executes a multi-frame imaging sequence, emitting progress and frame events.
pub async fn execute_sequence(
    camera: CameraHandle,
    mount: MountHandle,
    sequence: ImagingSequence,
    mut cancel_rx: watch::Receiver<bool>,
    mut pause_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<Option<SequenceStatus>>,
    event_tx: broadcast::Sender<ControllerEvent>,
) -> SequenceStatus {
    let total = sequence.total_frames();
    let enabled_steps: Vec<&protocol::SequenceStep> =
        sequence.steps.iter().filter(|step| step.enabled).collect();
    let total_steps = enabled_steps.len() as u32;

    let emit = |st: &SequenceStatus| {
        let _ = status_tx.send(Some(st.clone()));
        let _ = event_tx.send(ControllerEvent::SequenceStatus(st.clone()));
    };

    let mut status = SequenceStatus {
        sequence_id: sequence.id,
        target_name: sequence.target_name.clone(),
        phase: SequencePhase::Running,
        current_step: 0,
        total_steps,
        current_step_completed: 0,
        current_step_total: 0,
        completed: 0,
        total,
        current_frame_type: None,
        current_frame_id: None,
        message: None,
    };

    if total == 0 {
        status.phase = SequencePhase::Completed;
        status.message = Some("Sequence has no frames to capture".to_string());
        emit(&status);
        return status;
    }

    emit(&status);

    let mut completed: u32 = 0;

    for (step_idx, step) in enabled_steps.into_iter().enumerate() {
        status.current_step = step_idx as u32 + 1;
        status.current_step_completed = 0;
        status.current_step_total = step.count;
        status.current_frame_type = Some(step.frame_type);

        for frame_in_step in 0..step.count {
            if wait_for_pause_or_cancel(&mut pause_rx, &mut cancel_rx, &mut status, &emit).await {
                status.phase = SequencePhase::Aborted;
                status.current_frame_id = None;
                status.message = Some("Sequence aborted".to_string());
                emit(&status);
                return status;
            }

            if sequence.auto_meridian_flip && step.frame_type == FrameType::Light {
                if let Err(err) =
                    coordinate_meridian_flip(&mount, &mut cancel_rx, &mut status, &emit).await
                {
                    if *cancel_rx.borrow() {
                        status.phase = SequencePhase::Aborted;
                        status.current_frame_id = None;
                        status.message = Some("Sequence aborted".to_string());
                    } else {
                        warn!("Sequence {}: {err}", sequence.id);
                        status.phase = SequencePhase::Failed;
                        status.current_frame_id = None;
                        status.message = Some(err);
                    }
                    emit(&status);
                    return status;
                }
            }

            let frame_id = sequence
                .id
                .wrapping_mul(100_000)
                .wrapping_add(u64::from(completed) + 1);
            status.current_frame_id = Some(frame_id);
            emit(&status);

            let frame = Frame {
                id: frame_id,
                target_name: sequence.target_name.clone(),
                frame_type: step.frame_type,
                params: step.params.clone(),
                save_config: sequence.save_config.clone(),
            };

            match execute_capture_with_cancel(&camera, &frame, cancel_rx.clone()).await {
                Ok(captured) => {
                    completed = completed.saturating_add(1);
                    status.completed = completed;
                    status.current_step_completed = frame_in_step + 1;
                    status.current_frame_id = None;
                    let _ = event_tx.send(ControllerEvent::FrameCaptured(captured));
                    emit(&status);
                }
                Err(err) => {
                    if *cancel_rx.borrow() {
                        status.phase = SequencePhase::Aborted;
                        status.current_frame_id = None;
                        status.message = Some("Sequence aborted".to_string());
                    } else {
                        error!("Sequence {} frame {frame_id} failed: {err}", sequence.id);
                        let _ = event_tx.send(ControllerEvent::CaptureError {
                            frame_id,
                            error: err.to_string(),
                        });
                        status.phase = SequencePhase::Failed;
                        status.current_frame_id = None;
                        status.message = Some(err.to_string());
                    }
                    emit(&status);
                    return status;
                }
            }

            if !step.delay_between_frames.is_zero() && frame_in_step + 1 < step.count {
                tokio::select! {
                    res = cancel_rx.changed() => {
                        if res.is_err() || *cancel_rx.borrow() {
                            status.phase = SequencePhase::Aborted;
                            status.current_frame_id = None;
                            status.message = Some("Sequence aborted".to_string());
                            emit(&status);
                            return status;
                        }
                    }
                    () = tokio::time::sleep(step.delay_between_frames) => {}
                }
            }
        }
    }

    status.phase = SequencePhase::Completed;
    status.current_frame_id = None;
    status.message = Some(format!("Completed {completed}/{total} frames"));
    emit(&status);
    status
}
