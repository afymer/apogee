use std::time::Duration;

use protocol::camera::CameraStatus;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use crate::{
    Driver, Initialized, Uninitialized,
    handle::CameraHandle,
    types::{CameraCommand, CameraFileRef, CaptureError, DeleteError, DownloadError},
};

struct ConnectionConfig {
    max_soft_retries: u32,
    max_total_retries: u32,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_soft_retries: 3,
            max_total_retries: 6,
        }
    }
}

pub struct CameraActor {
    rx: mpsc::Receiver<CameraCommand>,
    status_tx: watch::Sender<CameraStatus>,
    connection_config: ConnectionConfig,
}

#[derive(thiserror::Error, Debug)]
pub enum ConnectionError {
    #[error("app is quitting")]
    AppQuitting,
    #[error("hardware did not respond after multiple trials")]
    HardwareUnresponsive,
}

impl CameraActor {
    pub fn spawn() -> CameraHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (status_tx, status_rx) = watch::channel(CameraStatus::Disconnected);

        let actor = CameraActor {
            rx: cmd_rx,
            status_tx,
            connection_config: ConnectionConfig::default(),
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        CameraHandle::new(cmd_tx, status_rx)
    }

    async fn run(mut self) {
        loop {
            let mut driver = match self.connect().await {
                Ok(driver) => driver,
                Err(ConnectionError::AppQuitting) => {
                    info!("App is quitting, closing camera driver");
                    return;
                }
                Err(ConnectionError::HardwareUnresponsive) => {
                    error!("Hardware is not responding, requesting intervention");
                    self.set_status(CameraStatus::Fatal(
                        "Hardware did not respond. Please check USB connection.".into(),
                    ));
                    if self
                        .sleep_and_drain_commands(Duration::from_secs(5))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            };

            self.set_status(CameraStatus::Ready);

            let failure_reason = self.serve_requests(&mut driver).await;
            driver.close();

            if failure_reason.is_none() {
                return;
            }
            warn!("Camera disconnected: {:?}. Recovering", failure_reason);
        }
    }

    async fn serve_requests(&mut self, driver: &mut Driver<Initialized>) -> Option<String> {
        let mut keep_alive_interval = tokio::time::interval(Duration::from_secs(25));
        keep_alive_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keep_alive_interval.tick().await;

        loop {
            tokio::select! {
                cmd = self.rx.recv() => {
                    match cmd {
                        Some(CameraCommand::Capture {
                            exposure_secs,
                            reply,
                        }) => {
                            let capture_timeout = Duration::from_secs((exposure_secs as u64) + 20).max(Duration::from_secs(25));
                            let capture_res = tokio::time::timeout(
                                capture_timeout,
                                Self::execute_capture(driver, exposure_secs),
                            )
                            .await;

                            match capture_res {
                                Ok(Ok(captured)) => {
                                    let _ = reply.send(Ok(captured));
                                    keep_alive_interval.reset();
                                }
                                Ok(Err(err)) => {
                                    error!("Capture execution failed: {err:?}");
                                    let is_fatal = matches!(err, CaptureError::Fatal(_));
                                    let _ = reply.send(Err(err));
                                    if is_fatal {
                                        return Some("Fatal hardware error during capture".into());
                                    }
                                }
                                Err(_) => {
                                    error!("Capture timed out after {capture_timeout:?}!");
                                    let _ = reply.send(Err(CaptureError::Fatal(
                                        format!("Camera capture timed out after {capture_timeout:?}")
                                    )));
                                    return Some("Camera capture timed out (hardware unresponsive)".into());
                                }
                            }
                        }
                        Some(CameraCommand::Download {
                            folder,
                            filename,
                            reply,
                        }) => {
                            let res = Self::execute_download(driver, &folder, &filename).await;
                            let _ = reply.send(res);
                        }
                        Some(CameraCommand::DeleteFile {
                            folder,
                            filename,
                            reply,
                        }) => {
                            let camera = driver.camera();
                            let delete_timeout = Duration::from_secs(10);
                            let res = tokio::time::timeout(
                                delete_timeout,
                                camera.fs().delete_file(&folder, &filename),
                            )
                            .await;

                            match res {
                                Ok(Ok(())) => {
                                    info!("Successfully deleted {folder}/{filename} from camera");
                                    let _ = reply.send(Ok(()));
                                }
                                Ok(Err(err)) => {
                                    warn!("Failed to delete {folder}/{filename} from camera: {err}");
                                    let _ = reply.send(Err(DeleteError::Failed(err.to_string())));
                                }
                                Err(_) => {
                                    warn!("Timed out deleting {folder}/{filename} from camera");
                                    let _ = reply.send(Err(DeleteError::Failed(
                                        "Delete file timed out".into(),
                                    )));
                                }
                            }
                        }
                        None => {
                            info!("Command channel closed, shutting down camera actor");
                            return None;
                        }
                    }
                }
                _ = keep_alive_interval.tick() => {
                    info!("Running camera keep-alive ping...");
                    match tokio::time::timeout(Duration::from_secs(5), driver.keep_alive()).await {
                        Ok(Ok(())) => {
                            // Camera responded to PTP command; connection is healthy
                        }
                        Ok(Err(err)) => {
                            warn!("Camera keep-alive failed: {err}");
                            return Some(format!("Camera keep-alive failed: {err}"));
                        }
                        Err(_) => {
                            warn!("Camera keep-alive timed out after 5s");
                            return Some("Camera keep-alive timed out (hardware unresponsive)".into());
                        }
                    }
                }
            }
        }
    }

    async fn execute_capture(
        driver: &mut Driver<Initialized>,
        exposure_secs: u32,
    ) -> Result<CameraFileRef, CaptureError> {
        let camera = driver.camera();
        info!("Triggering camera capture (exposure_secs={exposure_secs})...");

        let capture_timeout =
            Duration::from_secs((exposure_secs as u64) + 15).max(Duration::from_secs(20));
        let file_path = tokio::time::timeout(capture_timeout, camera.capture_image())
            .await
            .map_err(|_| CaptureError::Fatal("Capture shutter release timed out".into()))?
            .map_err(|err| {
                error!("Camera capture_image error: {err}");
                CaptureError::Fatal(err.to_string())
            })?;

        let folder = file_path.folder().to_string();
        let name = file_path.name().to_string();
        info!("Captured file on camera storage: {folder}/{name}");

        Ok(CameraFileRef {
            folder,
            filename: name,
        })
    }

    async fn execute_download(
        driver: &Driver<Initialized>,
        folder: &str,
        filename: &str,
    ) -> Result<Vec<u8>, DownloadError> {
        let camera = driver.camera();
        info!("Downloading {folder}/{filename} from camera...");

        let download_timeout = Duration::from_secs(30);
        let camera_file = tokio::time::timeout(download_timeout, camera.fs().download(folder, filename))
            .await
            .map_err(|_| DownloadError::Failed("Downloading file from camera timed out".into()))?
            .map_err(|err| {
                error!("Camera download error: {err}");
                DownloadError::Failed(err.to_string())
            })?;

        let data = tokio::time::timeout(download_timeout, camera_file.get_data(camera))
            .await
            .map_err(|_| DownloadError::Failed("Reading file data from camera timed out".into()))?
            .map_err(|err| {
                error!("Camera get_data error: {err}");
                DownloadError::Failed(err.to_string())
            })?;

        info!("Downloaded {} bytes for file {filename}", data.len());
        Ok(data.into_vec())
    }

    async fn connect(&mut self) -> Result<Driver<Initialized>, ConnectionError> {
        for attempt in 1..=self.connection_config.max_total_retries {
            let is_hard_reset_attempt = attempt == (self.connection_config.max_soft_retries + 1);

            self.set_status(CameraStatus::Recovering {
                attempt,
                reason: if attempt <= self.connection_config.max_soft_retries {
                    "Detecting camera on USB bus".into()
                } else {
                    "Resetting USB port and re-detecting".into()
                },
            });

            if is_hard_reset_attempt {
                warn!(
                    "Soft retries failed {} times. Triggering hardware USB power reset...",
                    self.connection_config.max_soft_retries
                );
                self.reset_usb_hardware().await;
            }

            if let Some(driver) = self.try_detect_once().await {
                return Ok(driver);
            }
            warn!(
                "Connection attempt {}/{} failed.",
                attempt, self.connection_config.max_total_retries
            );

            self.sleep_and_drain_commands(Duration::from_secs(3))
                .await?;
        }

        self.set_status(CameraStatus::Fatal(
            "Hardware did not respond after multiple retries".into(),
        ));
        Err(ConnectionError::HardwareUnresponsive)
    }

    async fn sleep_and_drain_commands(
        &mut self,
        duration: Duration,
    ) -> Result<(), ConnectionError> {
        let sleep = tokio::time::sleep(duration);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => return Ok(()),
                cmd = self.rx.recv() => {
                    match cmd {
                        Some(CameraCommand::Capture { reply, .. }) => {
                            reply.send(Err(CaptureError::Interrupted)).ok();
                        }
                        Some(CameraCommand::Download { reply, .. }) => {
                            reply.send(Err(DownloadError::ActorDisconnected)).ok();
                        }
                        Some(CameraCommand::DeleteFile { reply, .. }) => {
                            reply.send(Err(DeleteError::ActorDisconnected)).ok();
                        }
                        None => return Err(ConnectionError::AppQuitting),
                    }
                }
            }
        }
    }

    async fn reset_usb_hardware(&mut self) {
        info!("Resetting USB port (stub: hardware reset triggered)");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    async fn try_detect_once(&mut self) -> Option<Driver<Initialized>> {
        let uninit = Driver::<Uninitialized>::default();
        let ctx = gphoto2::Context::new().ok()?;

        let driver = tokio::time::timeout(Duration::from_secs(5), uninit.initialize(&ctx))
            .await
            .ok()? // No timeout
            .ok()?; // initialization succeeded

        // Attempt to disable auto power-off if supported by camera model
        if let Ok(autopoweroff) = driver
            .camera()
            .config_key::<gphoto2::widget::RadioWidget>("autopoweroff")
            .await
        {
            let _ = autopoweroff.set_choice("0");
            let _ = driver.camera().set_config(&autopoweroff).await;
            info!("Camera autopoweroff config updated to 0 (Disabled)");
        }

        Some(driver)
    }

    fn set_status(&self, status: CameraStatus) {
        self.status_tx.send(status).ok();
    }
}
