use std::time::Duration;

use protocol::camera::{CameraCapabilities, CameraStatus, DEFAULT_ISO_CHOICES};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

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
    capabilities_tx: watch::Sender<Option<CameraCapabilities>>,
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
        let (capabilities_tx, capabilities_rx) = watch::channel(None);

        let actor = CameraActor {
            rx: cmd_rx,
            status_tx,
            capabilities_tx,
            connection_config: ConnectionConfig::default(),
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        CameraHandle::with_capabilities(cmd_tx, status_rx, capabilities_rx)
    }

    /// Spawns a simulated camera actor that stays `CameraStatus::Ready` without polling USB.
    pub fn spawn_simulated() -> CameraHandle {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(32);
        let (status_tx, status_rx) = watch::channel(CameraStatus::Ready);
        let sim_caps = CameraCapabilities {
            model: Some("Simulated DSLR".to_string()),
            iso_choices: DEFAULT_ISO_CHOICES.to_vec(),
            supports_auto_iso: true,
        };
        let (capabilities_tx, capabilities_rx) = watch::channel(Some(sim_caps));

        tokio::spawn(async move {
            info!("Simulated camera actor ready (no USB polling)");
            let mut seq: u64 = 0;
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    CameraCommand::Capture {
                        exposure,
                        iso: _,
                        mut cancel,
                        reply,
                    } => {
                        let sleep = tokio::time::sleep(exposure.min(Duration::from_secs(10)));
                        tokio::pin!(sleep);
                        let res = if *cancel.borrow() {
                            Err(CaptureError::Interrupted)
                        } else {
                            tokio::select! {
                                () = &mut sleep => {
                                    seq += 1;
                                    Ok(CameraFileRef {
                                        folder: "/sim".to_string(),
                                        filename: format!("sim_frame_{seq:04}.raw"),
                                    })
                                }
                                () = wait_for_cancel(&mut cancel) => Err(CaptureError::Interrupted),
                            }
                        };
                        let _ = reply.send(res);
                    }
                    CameraCommand::Download { reply, .. } => {
                        let _ = reply.send(Ok(Vec::new()));
                    }
                    CameraCommand::DeleteFile { reply, .. } => {
                        let _ = reply.send(Ok(()));
                    }
                }
            }
            drop(capabilities_tx);
            drop(status_tx);
        });

        CameraHandle::with_capabilities(cmd_tx, status_rx, capabilities_rx)
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
                    self.set_capabilities(None);
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

            let caps = Self::query_capabilities(&driver).await;
            self.set_capabilities(caps);
            self.set_status(CameraStatus::Ready);

            let failure_reason = self.serve_requests(&mut driver).await;
            driver.close().await;
            self.set_capabilities(None);

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
                        Some(CameraCommand::Capture { exposure, iso, mut cancel, reply }) => {
                            if *cancel.borrow() {
                                let _ = reply.send(Err(CaptureError::Interrupted));
                                continue;
                            }
                            let capture_timeout = exposure
                                .saturating_add(Duration::from_secs(25))
                                .max(Duration::from_secs(30));
                            let capture_res = tokio::time::timeout(
                                capture_timeout,
                                Self::execute_capture(driver, exposure, iso, &mut cancel),
                            )
                            .await;

                            match capture_res {
                                Ok(CaptureOutcome::Completed(Ok(captured))) => {
                                    let _ = reply.send(Ok(captured));
                                    keep_alive_interval.reset();
                                }
                                Ok(CaptureOutcome::Completed(Err(err))) => {
                                    error!("Capture execution failed: {err:?}");
                                    let is_fatal = matches!(err, CaptureError::Fatal(_));
                                    let _ = reply.send(Err(err));
                                    if is_fatal {
                                        return Some("Fatal hardware error during capture".into());
                                    }
                                }
                                Ok(CaptureOutcome::Cancelled { needs_reconnect }) => {
                                    let _ = reply.send(Err(CaptureError::Interrupted));
                                    if needs_reconnect {
                                        return Some("Capture aborted; reconnecting camera".into());
                                    }
                                    keep_alive_interval.reset();
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
                            match Self::execute_download(driver, &folder, &filename).await {
                                Ok(data) => {
                                    let _ = reply.send(Ok(data));
                                    keep_alive_interval.reset();
                                }
                                Err((err, timed_out)) => {
                                    let _ = reply.send(Err(err));
                                    if timed_out {
                                        return Some("Camera download timed out".into());
                                    }
                                }
                            }
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
                                    keep_alive_interval.reset();
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
                                    return Some("Camera delete timed out".into());
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
        exposure: Duration,
        iso: Option<u32>,
        cancel: &mut watch::Receiver<bool>,
    ) -> CaptureOutcome {
        let camera = driver.camera();
        info!("Triggering camera capture (exposure={exposure:?}, iso={iso:?})...");

        if let Some(target_iso) = iso {
            match tokio::time::timeout(
                Duration::from_secs(5),
                camera.config_key::<gphoto2::widget::RadioWidget>("iso"),
            )
            .await
            {
                Ok(Ok(iso_widget)) => {
                    let choices: Vec<String> = iso_widget.choices_iter().collect();
                    if let Some(choice) =
                        select_closest_iso_choice(choices.iter().map(String::as_str), target_iso)
                    {
                        if let Err(err) = iso_widget.set_choice(choice) {
                            warn!("Failed to set ISO choice '{choice}': {err}");
                        } else {
                            match tokio::time::timeout(
                                Duration::from_secs(5),
                                camera.set_config(&iso_widget),
                            )
                            .await
                            {
                                Ok(Ok(())) => info!("Camera ISO config updated to '{choice}'"),
                                Ok(Err(err)) => {
                                    warn!("Failed to apply ISO config '{choice}': {err}");
                                }
                                Err(_) => warn!("Timed out applying ISO config '{choice}'"),
                            }
                        }
                    } else {
                        warn!("No numeric ISO choice matched requested ISO {target_iso}");
                    }
                }
                Ok(Err(err)) => warn!("Failed to read camera ISO config: {err}"),
                Err(_) => warn!("Timed out reading camera ISO config"),
            }
        }

        match tokio::time::timeout(
            Duration::from_secs(5),
            camera.config_key::<gphoto2::widget::RadioWidget>("shutterspeed"),
        )
        .await
        {
            Ok(Ok(shutter)) => {
                let choices: Vec<String> = shutter.choices_iter().collect();
                let max_finite = choices
                    .iter()
                    .filter_map(|c| parse_shutter_choice_secs(c))
                    .reduce(f64::max);

                if let Some(max_s) = max_finite
                    && exposure.as_secs_f64() > max_s * 1.15
                {
                    let bulb_choice = choices
                        .iter()
                        .map(String::as_str)
                        .find(|c| c.trim().eq_ignore_ascii_case("bulb"));

                    let bulb_toggle = match tokio::time::timeout(
                        Duration::from_secs(5),
                        camera.config_key::<gphoto2::widget::ToggleWidget>("bulb"),
                    )
                    .await
                    {
                        Ok(Ok(widget)) => Some(widget),
                        Ok(Err(err)) => {
                            warn!("Camera bulb toggle widget unavailable: {err}");
                            None
                        }
                        Err(_) => {
                            warn!("Timed out reading camera bulb toggle widget");
                            None
                        }
                    };

                    let (Some(b_choice), Some(bulb_widget)) = (bulb_choice, bulb_toggle) else {
                        return CaptureOutcome::Completed(Err(CaptureError::Fatal(format!(
                            "Requested exposure {exposure:?} exceeds max fixed shutter speed ({max_s:.1}s) and camera bulb mode is unavailable"
                        ))));
                    };

                    if let Err(err) = shutter.set_choice(b_choice) {
                        return CaptureOutcome::Completed(Err(CaptureError::Fatal(format!(
                            "Failed to select bulb shutterspeed '{b_choice}': {err}"
                        ))));
                    }
                    match tokio::time::timeout(Duration::from_secs(5), camera.set_config(&shutter))
                        .await
                    {
                        Ok(Ok(())) => info!("Camera shutterspeed config updated to '{b_choice}'"),
                        Ok(Err(err)) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(format!(
                                "Failed to apply bulb shutterspeed '{b_choice}': {err}"
                            ))));
                        }
                        Err(_) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                "Timed out applying bulb shutterspeed".into(),
                            )));
                        }
                    }

                    bulb_widget.set_toggled(true);
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        camera.set_config(&bulb_widget),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(format!(
                                "Failed to open bulb shutter: {err}"
                            ))));
                        }
                        Err(_) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                "Timed out opening bulb shutter".into(),
                            )));
                        }
                    }

                    let was_cancelled = tokio::select! {
                        () = tokio::time::sleep(exposure) => false,
                        () = wait_for_cancel(cancel) => true,
                    };

                    bulb_widget.set_toggled(false);
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        camera.set_config(&bulb_widget),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(format!(
                                "Failed to close bulb shutter: {err}"
                            ))));
                        }
                        Err(_) => {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                "Timed out closing bulb shutter".into(),
                            )));
                        }
                    }

                    if was_cancelled {
                        return CaptureOutcome::Cancelled {
                            needs_reconnect: false,
                        };
                    }

                    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                    loop {
                        let remaining =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                "Timed out waiting for bulb capture file event".into(),
                            )));
                        }
                        let step = remaining.min(Duration::from_secs(2));
                        match tokio::time::timeout(
                            step + Duration::from_secs(2),
                            driver.wait_event(step),
                        )
                        .await
                        {
                            Ok(Ok(gphoto2::camera::CameraEvent::NewFile(file_path))) => {
                                let folder = file_path.folder().to_string();
                                let name = file_path.name().to_string();
                                info!("Captured bulb file on camera storage: {folder}/{name}");
                                return CaptureOutcome::Completed(Ok(CameraFileRef {
                                    folder,
                                    filename: name,
                                }));
                            }
                            Ok(Ok(_)) => continue,
                            Ok(Err(err)) => {
                                return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                    format!("Error waiting for bulb capture file event: {err}"),
                                )));
                            }
                            Err(_) => {
                                return CaptureOutcome::Completed(Err(CaptureError::Fatal(
                                    "Camera wait_event timed out during bulb capture".into(),
                                )));
                            }
                        }
                    }
                }

                if let Some(choice) =
                    select_closest_shutter_choice(choices.iter().map(String::as_str), exposure)
                {
                    if let Err(err) = shutter.set_choice(choice) {
                        warn!("Failed to select shutterspeed choice '{choice}': {err}");
                    } else {
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            camera.set_config(&shutter),
                        )
                        .await
                        {
                            Ok(Ok(())) => {
                                info!("Camera shutterspeed config updated to '{choice}'");
                            }
                            Ok(Err(err)) => {
                                warn!("Failed to apply shutterspeed config '{choice}': {err}");
                            }
                            Err(_) => {
                                warn!("Timed out applying shutterspeed config '{choice}'");
                            }
                        }
                    }
                }
            }
            Ok(Err(err)) => debug!("Camera shutterspeed config unavailable: {err}"),
            Err(_) => warn!("Timed out reading camera shutterspeed config"),
        }

        let capture_timeout = exposure
            .saturating_add(Duration::from_secs(15))
            .max(Duration::from_secs(20));

        tokio::select! {
            res = tokio::time::timeout(capture_timeout, camera.capture_image()) => {
                match res {
                    Ok(Ok(file_path)) => {
                        let folder = file_path.folder().to_string();
                        let name = file_path.name().to_string();
                        info!("Captured file on camera storage: {folder}/{name}");
                        CaptureOutcome::Completed(Ok(CameraFileRef {
                            folder,
                            filename: name,
                        }))
                    }
                    Ok(Err(err)) => {
                        error!("Camera capture_image error: {err}");
                        CaptureOutcome::Completed(Err(CaptureError::Fatal(err.to_string())))
                    }
                    Err(_) => CaptureOutcome::Completed(Err(CaptureError::Fatal(
                        "Capture shutter release timed out".into(),
                    ))),
                }
            }
            () = wait_for_cancel(cancel) => {
                CaptureOutcome::Cancelled { needs_reconnect: true }
            }
        }
    }

    async fn execute_download(
        driver: &Driver<Initialized>,
        folder: &str,
        filename: &str,
    ) -> Result<Vec<u8>, (DownloadError, bool)> {
        let camera = driver.camera();
        info!("Downloading {folder}/{filename} from camera...");

        let download_timeout = Duration::from_secs(30);
        let camera_file =
            tokio::time::timeout(download_timeout, camera.fs().download(folder, filename))
                .await
                .map_err(|_| {
                    (
                        DownloadError::Failed("Downloading file from camera timed out".into()),
                        true,
                    )
                })?
                .map_err(|err| {
                    error!("Camera download error: {err}");
                    (DownloadError::Failed(err.to_string()), false)
                })?;

        let data = tokio::time::timeout(download_timeout, camera_file.get_data(camera))
            .await
            .map_err(|_| {
                (
                    DownloadError::Failed("Reading file data from camera timed out".into()),
                    true,
                )
            })?
            .map_err(|err| {
                error!("Camera get_data error: {err}");
                (DownloadError::Failed(err.to_string()), false)
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
        let ctx = match gphoto2::Context::new() {
            Ok(ctx) => ctx,
            Err(err) => {
                warn!("Failed to create gphoto2 context: {err}");
                return None;
            }
        };

        let driver =
            match tokio::time::timeout(Duration::from_secs(5), uninit.initialize(&ctx)).await {
                Ok(Ok(driver)) => driver,
                Ok(Err(err)) => {
                    debug!("Camera autodetect failed: {err}");
                    return None;
                }
                Err(_) => {
                    warn!("Timed out waiting for camera autodetect");
                    return None;
                }
            };

        // Attempt to disable auto power-off if supported by camera model
        match tokio::time::timeout(
            Duration::from_secs(5),
            driver
                .camera()
                .config_key::<gphoto2::widget::RadioWidget>("autopoweroff"),
        )
        .await
        {
            Ok(Ok(autopoweroff)) => {
                if let Err(err) = autopoweroff.set_choice("0") {
                    warn!("Failed to select autopoweroff=0: {err}");
                } else {
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        driver.camera().set_config(&autopoweroff),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            info!("Camera autopoweroff config updated to 0 (Disabled)");
                        }
                        Ok(Err(err)) => {
                            warn!("Failed to apply autopoweroff=0 config: {err}");
                        }
                        Err(_) => warn!("Timed out applying autopoweroff=0 config"),
                    }
                }
            }
            Ok(Err(err)) => debug!("Camera autopoweroff config unavailable: {err}"),
            Err(_) => warn!("Timed out reading camera autopoweroff config"),
        }

        Some(driver)
    }

    async fn query_capabilities(driver: &Driver<Initialized>) -> Option<CameraCapabilities> {
        let abilities = driver.camera().abilities();
        let raw_model = abilities.model();
        let trimmed_model = raw_model.trim();
        let model = (!trimmed_model.is_empty()).then(|| trimmed_model.to_string());

        let (iso_choices, supports_auto_iso) = match tokio::time::timeout(
            Duration::from_secs(5),
            driver
                .camera()
                .config_key::<gphoto2::widget::RadioWidget>("iso"),
        )
        .await
        {
            Ok(Ok(iso_widget)) => {
                let raw_choices: Vec<String> = iso_widget.choices_iter().collect();
                let (choices, auto) =
                    parse_iso_capabilities(raw_choices.iter().map(String::as_str));
                info!(
                    "Queried camera capabilities (model={:?}, iso_choices={}, supports_auto={})",
                    model,
                    choices.len(),
                    auto
                );
                (choices, auto)
            }
            Ok(Err(err)) => {
                debug!("Camera ISO choices unavailable during capability query: {err}");
                (Vec::new(), true)
            }
            Err(_) => {
                warn!("Timed out querying camera ISO choices");
                (Vec::new(), true)
            }
        };

        (model.is_some() || !iso_choices.is_empty()).then_some(CameraCapabilities {
            model,
            iso_choices,
            supports_auto_iso,
        })
    }

    fn set_status(&self, status: CameraStatus) {
        self.status_tx.send(status).ok();
    }

    fn set_capabilities(&self, caps: Option<CameraCapabilities>) {
        self.capabilities_tx.send(caps).ok();
    }
}

enum CaptureOutcome {
    Completed(Result<CameraFileRef, CaptureError>),
    Cancelled { needs_reconnect: bool },
}

async fn wait_for_cancel(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    if cancel.wait_for(|&c| c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn strip_iso_prefix(trimmed: &str) -> &str {
    if trimmed
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("iso"))
    {
        trimmed[3..].trim()
    } else {
        trimmed
    }
}

fn parse_iso_choice(choice: &str) -> Option<u32> {
    let trimmed = choice.trim();
    let numeric_part = strip_iso_prefix(trimmed);
    let val: u32 = numeric_part.parse().ok()?;
    (val > 0).then_some(val)
}

fn parse_iso_capabilities<'a>(choices: impl IntoIterator<Item = &'a str>) -> (Vec<u32>, bool) {
    let mut supports_auto = false;
    let mut values = Vec::new();
    for raw in choices {
        let trimmed = raw.trim();
        let after_iso = strip_iso_prefix(trimmed);
        if after_iso.eq_ignore_ascii_case("auto") {
            supports_auto = true;
            continue;
        }
        if let Some(val) = parse_iso_choice(trimmed) {
            values.push(val);
        }
    }
    values.sort_unstable();
    values.dedup();
    (values, supports_auto)
}

fn select_closest_iso_choice<'a>(
    choices: impl IntoIterator<Item = &'a str>,
    target_iso: u32,
) -> Option<&'a str> {
    let target = f64::from(target_iso);
    if target <= 0.0 {
        return None;
    }

    choices
        .into_iter()
        .filter_map(|choice| {
            let val = f64::from(parse_iso_choice(choice)?);
            let log_dist = (val / target).ln().abs();
            Some((choice, log_dist))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(choice, _)| choice)
}

fn parse_shutter_choice_secs(choice: &str) -> Option<f64> {
    let trimmed = choice.trim();
    let stripped = trimmed.strip_suffix(['s', 'S']).unwrap_or(trimmed).trim();
    if stripped.is_empty()
        || stripped.eq_ignore_ascii_case("bulb")
        || stripped.eq_ignore_ascii_case("time")
    {
        return None;
    }

    let secs = if let Some((num, den)) = stripped.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d <= 0.0 {
            return None;
        }
        n / d
    } else {
        stripped.replace('"', ".").parse::<f64>().ok()?
    };

    (secs.is_finite() && secs > 0.0).then_some(secs)
}

fn select_closest_shutter_choice<'a>(
    choices: impl IntoIterator<Item = &'a str>,
    target: Duration,
) -> Option<&'a str> {
    let target_secs = target.as_secs_f64();
    if !target_secs.is_finite() || target_secs <= 0.0 {
        return None;
    }

    choices
        .into_iter()
        .filter_map(|choice| {
            let secs = parse_shutter_choice_secs(choice)?;
            let log_dist = (secs / target_secs).ln().abs();
            Some((choice, log_dist))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(choice, _)| choice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_choice_handles_prefixed_and_non_numeric_values() {
        assert_eq!(parse_iso_choice("100"), Some(100));
        assert_eq!(parse_iso_choice("  ISO 800 "), Some(800));
        assert_eq!(parse_iso_choice("iso1600"), Some(1600));
        assert_eq!(parse_iso_choice("Iso 3200"), Some(3200));
        assert_eq!(parse_iso_choice("Auto"), None);
        assert_eq!(parse_iso_choice("ISO Auto"), None);
        assert_eq!(parse_iso_choice("Hi 1"), None);
        assert_eq!(parse_iso_choice("0"), None);
        assert_eq!(parse_iso_choice("élan"), None);
    }

    #[test]
    fn parse_iso_capabilities_extracts_auto_sorts_and_dedups() {
        let (choices, auto) = parse_iso_capabilities([
            "Auto",
            "ISO 1600",
            "100",
            "Hi 1",
            "ISO 400",
            "400",
            "iso 800",
            "ISO 200",
        ]);
        assert!(auto);
        assert_eq!(choices, vec![100, 200, 400, 800, 1600]);

        let (no_auto_choices, no_auto) = parse_iso_capabilities(["100", "125", "160", "200"]);
        assert!(!no_auto);
        assert_eq!(no_auto_choices, vec![100, 125, 160, 200]);
    }

    #[test]
    fn select_closest_iso_choice_picks_nearest_log_stop() {
        let choices = ["Auto", "100", "ISO 200", "400", "ISO 800", "1600"];
        assert_eq!(select_closest_iso_choice(choices, 800), Some("ISO 800"));
        assert_eq!(select_closest_iso_choice(choices, 320), Some("400"));
        assert_eq!(select_closest_iso_choice(choices, 50), Some("100"));
    }
}
