use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gphoto::handle::CameraHandle;
use protocol::{
    Frame, FrameType, ImagingSequence, MountCommand, MountStatus, MountTelemetry, NightPlan,
    NightPlanRunRequest, PierSide, PlannedTarget, SequencePhase, SequenceStatus, StageKind,
};
use synscan::MountHandle;
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

use crate::{ControllerEvent, execute_capture_with_cancel};

/// Returns `true` when the mount is on the West pier side (OTA West, originally looking East)
/// and has tracked past the meridian into the western sky beyond `limits.meridian_limit_deg`.
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

    let prev_phase = status.phase;
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

    status.phase = if prev_phase == SequencePhase::Paused {
        SequencePhase::Running
    } else {
        prev_phase
    };
    status.message = None;
    emit(status);
    false
}

async fn wait_for_mount_slew_settle(
    mount: &MountHandle,
    cancel_rx: &mut watch::Receiver<bool>,
    timeout_secs: u64,
    expect_parked: bool,
    timeout_label: &str,
) -> Result<(), String> {
    let mut telem_rx = mount.telemetry();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut saw_slewing = false;
    let settle_grace = tokio::time::Instant::now() + Duration::from_millis(1500);

    loop {
        if *cancel_rx.borrow() {
            return Err(format!("Cancelled during {timeout_label}"));
        }

        if let Some(current) = telem_rx.borrow().as_ref() {
            if current.is_slewing {
                saw_slewing = true;
            } else if expect_parked {
                if current.is_parked {
                    break;
                }
            } else if saw_slewing || tokio::time::Instant::now() >= settle_grace {
                break;
            }
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("{timeout_label} timed out after {timeout_secs}s"));
        }

        let poll_wait = remaining.min(Duration::from_millis(200));
        tokio::select! {
            res = cancel_rx.changed() => {
                if res.is_err() || *cancel_rx.borrow() {
                    return Err(format!("Cancelled during {timeout_label}"));
                }
            }
            _ = telem_rx.changed() => {}
            () = tokio::time::sleep(poll_wait) => {}
        }
    }

    Ok(())
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

    wait_for_mount_slew_settle(mount, cancel_rx, 180, false, "meridian flip").await?;

    info!("Sequence {}: meridian flip settled", status.sequence_id);
    status.phase = SequencePhase::Running;
    status.message = None;
    emit(status);
    Ok(())
}

async fn slew_mount_to_target(
    mount: &MountHandle,
    target: &PlannedTarget,
    cancel_rx: &mut watch::Receiver<bool>,
    status: &mut SequenceStatus,
    emit: &impl Fn(&SequenceStatus),
) -> Result<(), String> {
    if *mount.status().borrow() != MountStatus::Ready {
        return Ok(());
    }

    let is_parked = mount
        .telemetry()
        .borrow()
        .as_ref()
        .is_some_and(|t| t.is_parked);

    status.phase = SequencePhase::SlewingToTarget;
    status.message = Some(format!("Slewing to {}...", target.name));
    emit(status);

    if is_parked {
        if let Err(err) = mount.send_command(MountCommand::Unpark).await {
            return Err(format!("Failed to unpark mount: {err}"));
        }
    }

    if let Err(err) = mount
        .send_command(MountCommand::SlewToEquatorial {
            ra_deg: target.ra_deg,
            dec_deg: target.dec_deg,
        })
        .await
    {
        return Err(format!("Failed to slew to {}: {err}", target.name));
    }

    wait_for_mount_slew_settle(mount, cancel_rx, 180, false, "target slew").await?;
    status.phase = SequencePhase::Running;
    status.message = None;
    emit(status);
    Ok(())
}

async fn park_mount_at_end(
    mount: &MountHandle,
    cancel_rx: &mut watch::Receiver<bool>,
    status: &mut SequenceStatus,
    emit: &impl Fn(&SequenceStatus),
) -> Result<(), String> {
    if *mount.status().borrow() != MountStatus::Ready {
        return Ok(());
    }

    status.phase = SequencePhase::Parking;
    status.message = Some("Parking mount...".to_string());
    emit(status);

    if let Err(err) = mount.send_command(MountCommand::Park).await {
        return Err(format!("Failed to park mount: {err}"));
    }

    wait_for_mount_slew_settle(mount, cancel_rx, 120, true, "mount park").await
}

async fn execute_non_capture_stage(
    step: &protocol::SequenceStep,
    pause_rx: &mut watch::Receiver<bool>,
    cancel_rx: &mut watch::Receiver<bool>,
    status: &mut SequenceStatus,
    emit: &impl Fn(&SequenceStatus),
) -> Result<(), ()> {
    if wait_for_pause_or_cancel(pause_rx, cancel_rx, status, emit).await {
        return Err(());
    }
    status.current_frame_type = None;
    match step.kind {
        StageKind::Capture => Ok(()),
        StageKind::Autofocus => {
            info!(
                "Sequence {}: Autofocus stage (not yet connected, continuing)",
                status.sequence_id
            );
            status.message = Some("Autofocus (not yet connected, continuing)".to_string());
            emit(status);
            Ok(())
        }
        StageKind::PlateSolve => {
            info!(
                "Sequence {}: Plate Solve stage (not yet connected, continuing)",
                status.sequence_id
            );
            status.message = Some("Plate Solve (not yet connected, continuing)".to_string());
            emit(status);
            Ok(())
        }
        StageKind::Wait => {
            if !step.delay_between_frames.is_zero() {
                status.message = Some(format!(
                    "Waiting {}s...",
                    step.delay_between_frames.as_secs()
                ));
                emit(status);
                tokio::select! {
                    res = cancel_rx.changed() => {
                        if res.is_err() || *cancel_rx.borrow() {
                            return Err(());
                        }
                    }
                    () = tokio::time::sleep(step.delay_between_frames) => {}
                }
            }
            Ok(())
        }
    }
}

/// Executes a single-target multi-frame imaging sequence, emitting progress and frame events.
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
        plan_id: None,
        current_target_id: None,
        current_target_index: 1,
        total_targets: 1,
        waiting_until_unix_secs: None,
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
        if step.kind != StageKind::Capture {
            if execute_non_capture_stage(step, &mut pause_rx, &mut cancel_rx, &mut status, &emit)
                .await
                .is_err()
            {
                status.phase = SequencePhase::Aborted;
                status.current_frame_id = None;
                status.message = Some("Sequence aborted".to_string());
                emit(&status);
                return status;
            }
            continue;
        }
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

fn take_plan_update(plan_update_rx: &mut watch::Receiver<Option<NightPlan>>) -> Option<NightPlan> {
    if plan_update_rx.has_changed().unwrap_or(false) {
        return plan_update_rx.borrow_and_update().clone();
    }
    None
}

fn compute_plan_totals(
    plan: &NightPlan,
    completed_target_ids: &HashSet<String>,
    completed_so_far: u32,
) -> (u32, u32) {
    let active_targets: Vec<&PlannedTarget> = plan
        .targets
        .iter()
        .filter(|t| t.enabled && t.total_frames() > 0)
        .collect();
    let total_targets = active_targets.len() as u32;
    let remaining_frames: u32 = active_targets
        .iter()
        .filter(|t| !completed_target_ids.contains(&t.id))
        .map(|t| t.total_frames())
        .sum();
    (total_targets, completed_so_far.saturating_add(remaining_frames))
}

/// Executes an autonomous multi-target `NightPlan`: waiting for target start times,
/// slewing the mount, running enabled steps with meridian-flip coordination, applying
/// pause-to-edit updates, and optionally parking the mount at completion.
#[allow(clippy::too_many_arguments)]
pub async fn execute_night_plan(
    camera: CameraHandle,
    mount: MountHandle,
    request: NightPlanRunRequest,
    mut cancel_rx: watch::Receiver<bool>,
    mut pause_rx: watch::Receiver<bool>,
    mut plan_update_rx: watch::Receiver<Option<NightPlan>>,
    status_tx: watch::Sender<Option<SequenceStatus>>,
    event_tx: broadcast::Sender<ControllerEvent>,
) -> SequenceStatus {
    let mut plan = request.plan;
    let mut completed_target_ids: HashSet<String> = HashSet::new();
    let mut completed_frames: u32 = 0;

    let emit = |st: &SequenceStatus| {
        let _ = status_tx.send(Some(st.clone()));
        let _ = event_tx.send(ControllerEvent::SequenceStatus(st.clone()));
    };

    let (mut total_targets, mut total_frames) =
        compute_plan_totals(&plan, &completed_target_ids, completed_frames);

    let mut status = SequenceStatus {
        sequence_id: request.run_id,
        plan_id: Some(plan.id.clone()),
        current_target_id: None,
        current_target_index: 0,
        total_targets,
        waiting_until_unix_secs: None,
        target_name: plan.name.clone(),
        phase: SequencePhase::Running,
        current_step: 0,
        total_steps: 0,
        current_step_completed: 0,
        current_step_total: 0,
        completed: 0,
        total: total_frames,
        current_frame_type: None,
        current_frame_id: None,
        message: None,
    };

    if total_frames == 0 {
        status.phase = SequencePhase::Completed;
        status.message = Some("Night plan has no enabled frames".to_string());
        emit(&status);
        return status;
    }

    emit(&status);

    let mut completed_targets_count: u32 = 0;

    loop {
        if let Some(updated) = take_plan_update(&mut plan_update_rx) {
            plan = updated;
            (total_targets, total_frames) =
                compute_plan_totals(&plan, &completed_target_ids, completed_frames);
            status.total_targets = total_targets;
            status.total = total_frames;
        }

        let next_target = plan
            .targets
            .iter()
            .find(|t| t.enabled && t.total_frames() > 0 && !completed_target_ids.contains(&t.id))
            .cloned();

        let Some(mut target) = next_target else {
            break;
        };

        completed_targets_count = completed_targets_count.saturating_add(1);
        status.current_target_id = Some(target.id.clone());
        status.current_target_index = completed_targets_count;
        status.target_name = target.name.clone();
        status.current_step = 0;
        status.total_steps = target.steps.iter().filter(|s| s.enabled).count() as u32;
        status.current_step_completed = 0;
        status.current_step_total = 0;

        if !request.start_immediately {
            loop {
                if wait_for_pause_or_cancel(&mut pause_rx, &mut cancel_rx, &mut status, &emit).await
                {
                    status.phase = SequencePhase::Aborted;
                    status.waiting_until_unix_secs = None;
                    status.current_frame_id = None;
                    status.message = Some("Night plan aborted".to_string());
                    emit(&status);
                    return status;
                }

                if let Some(updated) = take_plan_update(&mut plan_update_rx) {
                    plan = updated;
                    (total_targets, total_frames) =
                        compute_plan_totals(&plan, &completed_target_ids, completed_frames);
                    status.total_targets = total_targets;
                    status.total = total_frames;
                    if let Some(refreshed) = plan.targets.iter().find(|t| t.id == target.id) {
                        target = refreshed.clone();
                        status.target_name = target.name.clone();
                    }
                }

                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let wait_secs = target.start_unix_secs - now_secs;
                if wait_secs <= 0 {
                    status.waiting_until_unix_secs = None;
                    break;
                }

                if status.phase != SequencePhase::WaitingForStart
                    || status.waiting_until_unix_secs != Some(target.start_unix_secs)
                {
                    status.phase = SequencePhase::WaitingForStart;
                    status.waiting_until_unix_secs = Some(target.start_unix_secs);
                    status.message = Some(format!("Waiting to start {}", target.name));
                    emit(&status);
                }

                let sleep_dur = Duration::from_secs((wait_secs as u64).min(1));
                tokio::select! {
                    res = cancel_rx.changed() => {
                        if res.is_err() || *cancel_rx.borrow() {
                            status.phase = SequencePhase::Aborted;
                            status.waiting_until_unix_secs = None;
                            status.current_frame_id = None;
                            status.message = Some("Night plan aborted".to_string());
                            emit(&status);
                            return status;
                        }
                    }
                    _ = pause_rx.changed() => {}
                    _ = plan_update_rx.changed() => {}
                    () = tokio::time::sleep(sleep_dur) => {}
                }
            }
        }

        if target.slew_to_target
            && let Err(err) =
                slew_mount_to_target(&mount, &target, &mut cancel_rx, &mut status, &emit).await
        {
            status.waiting_until_unix_secs = None;
            status.current_frame_id = None;
            if *cancel_rx.borrow() {
                status.phase = SequencePhase::Aborted;
                status.message = Some("Night plan aborted".to_string());
            } else {
                status.phase = SequencePhase::Failed;
                status.message = Some(err);
            }
            emit(&status);
            return status;
        }

        status.phase = SequencePhase::Running;
        status.waiting_until_unix_secs = None;
        status.message = None;

        let mut step_idx: usize = 0;
        loop {
            if let Some(updated) = take_plan_update(&mut plan_update_rx) {
                plan = updated;
                (total_targets, total_frames) =
                    compute_plan_totals(&plan, &completed_target_ids, completed_frames);
                status.total_targets = total_targets;
                status.total = total_frames;
                if let Some(refreshed) = plan.targets.iter().find(|t| t.id == target.id) {
                    target = refreshed.clone();
                    status.target_name = target.name.clone();
                }
            }

            let enabled_steps: Vec<protocol::SequenceStep> =
                target.steps.iter().filter(|s| s.enabled).cloned().collect();
            status.total_steps = enabled_steps.len() as u32;
            if step_idx >= enabled_steps.len() {
                break;
            }

            let step = enabled_steps[step_idx].clone();
            status.current_step = step_idx as u32 + 1;
            status.current_step_completed = 0;
            status.current_step_total = step.count;
            if step.kind != StageKind::Capture {
                if execute_non_capture_stage(
                    &step,
                    &mut pause_rx,
                    &mut cancel_rx,
                    &mut status,
                    &emit,
                )
                .await
                .is_err()
                {
                    status.phase = SequencePhase::Aborted;
                    status.current_frame_id = None;
                    status.message = Some("Night plan aborted".to_string());
                    emit(&status);
                    return status;
                }
                step_idx += 1;
                continue;
            }
            status.current_frame_type = Some(step.frame_type);

            for frame_in_step in 0..step.count {
                if wait_for_pause_or_cancel(&mut pause_rx, &mut cancel_rx, &mut status, &emit).await
                {
                    status.phase = SequencePhase::Aborted;
                    status.current_frame_id = None;
                    status.message = Some("Night plan aborted".to_string());
                    emit(&status);
                    return status;
                }

                if let Some(updated) = take_plan_update(&mut plan_update_rx) {
                    plan = updated;
                    (total_targets, total_frames) =
                        compute_plan_totals(&plan, &completed_target_ids, completed_frames);
                    status.total_targets = total_targets;
                    status.total = total_frames;
                    if let Some(refreshed) = plan.targets.iter().find(|t| t.id == target.id) {
                        target = refreshed.clone();
                        status.target_name = target.name.clone();
                    }
                }

                if target.auto_meridian_flip && step.frame_type == FrameType::Light {
                    if let Err(err) =
                        coordinate_meridian_flip(&mount, &mut cancel_rx, &mut status, &emit).await
                    {
                        status.current_frame_id = None;
                        if *cancel_rx.borrow() {
                            status.phase = SequencePhase::Aborted;
                            status.message = Some("Night plan aborted".to_string());
                        } else {
                            status.phase = SequencePhase::Failed;
                            status.message = Some(err);
                        }
                        emit(&status);
                        return status;
                    }
                }

                let frame_id = request
                    .run_id
                    .wrapping_mul(100_000)
                    .wrapping_add(u64::from(completed_frames) + 1);
                status.current_frame_id = Some(frame_id);
                emit(&status);

                let frame = Frame {
                    id: frame_id,
                    target_name: target.name.clone(),
                    frame_type: step.frame_type,
                    params: step.params.clone(),
                    save_config: plan.save_config.clone(),
                };

                match execute_capture_with_cancel(&camera, &frame, cancel_rx.clone()).await {
                    Ok(captured) => {
                        completed_frames = completed_frames.saturating_add(1);
                        status.completed = completed_frames;
                        status.current_step_completed = frame_in_step + 1;
                        status.current_frame_id = None;
                        let _ = event_tx.send(ControllerEvent::FrameCaptured(captured));
                        emit(&status);
                    }
                    Err(err) => {
                        status.current_frame_id = None;
                        if *cancel_rx.borrow() {
                            status.phase = SequencePhase::Aborted;
                            status.message = Some("Night plan aborted".to_string());
                        } else {
                            error!("NightPlan frame {frame_id} failed: {err}");
                            let _ = event_tx.send(ControllerEvent::CaptureError {
                                frame_id,
                                error: err.to_string(),
                            });
                            status.phase = SequencePhase::Failed;
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
                                status.message = Some("Night plan aborted".to_string());
                                emit(&status);
                                return status;
                            }
                        }
                        () = tokio::time::sleep(step.delay_between_frames) => {}
                    }
                }
            }
            step_idx += 1;
        }

        completed_target_ids.insert(target.id.clone());
    }

    if plan.policy.park_at_end
        && let Err(err) = park_mount_at_end(&mount, &mut cancel_rx, &mut status, &emit).await
    {
        status.current_frame_id = None;
        if *cancel_rx.borrow() {
            status.phase = SequencePhase::Aborted;
            status.message = Some("Night plan aborted".to_string());
        } else {
            status.phase = SequencePhase::Failed;
            status.message = Some(err);
        }
        emit(&status);
        return status;
    }

    status.phase = SequencePhase::Completed;
    status.current_frame_id = None;
    status.waiting_until_unix_secs = None;
    status.message = Some(format!(
        "Completed {completed_frames}/{total_frames} frames across {completed_targets_count} target(s)"
    ));
    emit(&status);
    status
}
