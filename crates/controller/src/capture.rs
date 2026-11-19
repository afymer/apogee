use std::time::Duration;

use gphoto::handle::CameraHandle;
use gphoto::types::CameraFileRef;
use protocol::{Frame, SaveLocationConfig};
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub frame: Box<Frame>,
    pub file_ref: CameraFileRef,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureExecutionError {
    #[error("Camera shutter release failed: {0}")]
    Exposure(String),
    #[error("Downloading file from camera failed: {0}")]
    Download(String),
    #[error("Deleting file from camera failed: {0}")]
    Delete(String),
}

/// Determines whether the captured file should be downloaded based on configured destinations.
#[must_use]
pub fn should_download(save_config: &SaveLocationConfig) -> bool {
    save_config.server.is_some() || save_config.viewer.is_some()
}

/// Determines whether the file should be deleted from camera storage based on save configuration.
#[must_use]
pub fn should_delete_from_camera(save_config: &SaveLocationConfig) -> bool {
    match &save_config.camera {
        Some(cam_cfg) => cam_cfg.delete_after_download,
        None => true,
    }
}

/// Triggers camera shutter release for the specified exposure duration and optional ISO.
pub async fn trigger_camera_exposure(
    camera: &CameraHandle,
    exposure: Duration,
) -> Result<CameraFileRef, CaptureExecutionError> {
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    trigger_camera_exposure_with_cancel(camera, exposure, None, cancel_rx).await
}

/// Triggers camera shutter release with optional ISO and cancellation signal.
pub async fn trigger_camera_exposure_with_cancel(
    camera: &CameraHandle,
    exposure: Duration,
    iso: Option<u32>,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<CameraFileRef, CaptureExecutionError> {
    info!("Triggering camera exposure ({exposure:?}, iso={iso:?})...");

    let timeout_duration = exposure
        .saturating_add(Duration::from_secs(40))
        .max(Duration::from_secs(45));

    tokio::time::timeout(
        timeout_duration,
        camera.capture_with_cancel(exposure, iso, cancel),
    )
    .await
    .map_err(|_| {
        CaptureExecutionError::Exposure(format!("Exposure timed out after {timeout_duration:?}"))
    })?
    .map_err(|err| CaptureExecutionError::Exposure(err.to_string()))
}

/// Downloads a captured file from the camera's filesystem into memory.
pub async fn download_captured_file(
    camera: &CameraHandle,
    file_ref: &CameraFileRef,
) -> Result<Vec<u8>, CaptureExecutionError> {
    info!("Downloading {}/{} from camera...", file_ref.folder, file_ref.filename);

    camera
        .download(&file_ref.folder, &file_ref.filename)
        .await
        .map_err(|err| CaptureExecutionError::Download(err.to_string()))
}

/// Deletes a file from the camera's filesystem.
pub async fn delete_camera_file(
    camera: &CameraHandle,
    file_ref: &CameraFileRef,
) -> Result<(), CaptureExecutionError> {
    info!("Deleting {}/{} from camera...", file_ref.folder, file_ref.filename);

    camera
        .delete_file(&file_ref.folder, &file_ref.filename)
        .await
        .map_err(|err| CaptureExecutionError::Delete(err.to_string()))
}

/// Coordinates the complete capture workflow for a single frame.
pub async fn execute_capture(
    camera: &CameraHandle,
    frame: &Frame,
) -> Result<CapturedFrame, CaptureExecutionError> {
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    execute_capture_with_cancel(camera, frame, cancel_rx).await
}

/// Coordinates the complete capture workflow for a single frame with cancellation support.
pub async fn execute_capture_with_cancel(
    camera: &CameraHandle,
    frame: &Frame,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<CapturedFrame, CaptureExecutionError> {
    // 1. Trigger exposure on camera hardware
    let file_ref = trigger_camera_exposure_with_cancel(
        camera,
        frame.params.exposure,
        frame.params.iso,
        cancel,
    )
    .await?;

    // 2. Download from camera if server or viewer save location is requested
    let data = if should_download(&frame.save_config) {
        Some(download_captured_file(camera, &file_ref).await?)
    } else {
        info!(
            "Skipping download for frame {}: only camera storage requested",
            frame.id
        );
        None
    };

    // 3. Delete from camera only if downloaded and requested by configuration
    if data.is_some() && should_delete_from_camera(&frame.save_config) {
        if let Err(err) = delete_camera_file(camera, &file_ref).await {
            warn!(
                "Failed to delete {}/{} from camera: {err}",
                file_ref.folder, file_ref.filename
            );
        }
    } else {
        info!(
            "Preserving {}/{} on camera storage",
            file_ref.folder, file_ref.filename
        );
    }

    Ok(CapturedFrame {
        frame: Box::new(frame.clone()),
        file_ref,
        data,
    })
}
