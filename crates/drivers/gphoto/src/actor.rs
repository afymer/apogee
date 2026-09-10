use std::time::Duration;

use protocol::camera::CameraStatus;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

use crate::{
    Driver, Initialized, Uninitialized,
    handle::CameraHandle,
    types::{CameraCommand, CaptureError},
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

            if failure_reason.is_none() {
                return;
            }
            warn!("Camera disconnected: {:?}. Recovering", failure_reason);
        }
    }

    async fn serve_requests(&mut self, driver: &mut Driver<Initialized>) -> Option<String> {
        loop {
            tokio::select! {
                cmd = self.rx.recv() => {
                    match cmd {
                        Some(CameraCommand::Capture {
                            exposure_secs: _,
                            reply: _,
                        }) => {
                            todo!()
                        }
                        None => {
                            info!("Command channel closed, shutting down camera actor");
                            return None;
                        }
                    }
                }
                event_res = driver.wait_event(Duration::from_millis(500)) => {
                    match event_res {
                        Ok(gphoto2::camera::CameraEvent::Timeout) => {
                            // Camera is still connected and responsive
                        }
                        Ok(event) => {
                            info!("Camera event: {:?}", event);
                        }
                        Err(err) => {
                            warn!("Camera health check failed: {err}");
                            return Some(format!("Camera communication error: {err}"));
                        }
                    }
                }
            }
        }
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

        tokio::time::timeout(Duration::from_secs(5), uninit.initialize(&ctx))
            .await
            .ok()? // No timeout
            .ok() // initialization succeeded
    }

    fn set_status(&self, status: CameraStatus) {
        self.status_tx.send(status).ok();
    }
}
