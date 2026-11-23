use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::bail;
use gpui::{App, AppContext, Entity, Global};
use persistence::{DocumentStore, HardwareConfig};
use protocol::{
    CameraStatus, ConnectionState, DaemonMessage, DaemonState, DaemonStatus, MountCommand,
    MountPointing, MountStatus, MountTelemetry, ViewerMessage,
};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedSender, channel, unbounded_channel};
use tracing::{error, info, warn};

pub fn next_frame_id() -> u64 {
    static COUNTER: LazyLock<AtomicU64> = LazyLock::new(|| {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        AtomicU64::new(seed.max(1))
    });
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub frame_id: u64,
    pub data: std::sync::Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub enum ClientModelUpdate {
    State {
        state: Box<DaemonState>,
        /// True when this update originated from a `DaemonMessage::MountStatus` frame
        /// rather than a synthetic local connection-state transition.
        from_mount_status: bool,
    },
    Frame {
        seq: u64,
        frame: CapturedFrame,
    },
    Error {
        frame_id: u64,
        message: String,
    },
    MountError(String),
}

pub struct DaemonStateModel {
    pub state: DaemonState,
    pub latest_frame: Option<CapturedFrame>,
    pub last_error: Option<(u64, String)>,
    pub last_mount_error: Option<String>,
    pub last_known_mount_pointing: Option<MountPointing>,
    pub hardware_config: HardwareConfig,
    store: DocumentStore,
    last_pointing_saved_ms: u64,
    latest_frame_seq: u64,
}

impl DaemonStateModel {
    pub fn new(state: DaemonState, store: DocumentStore, hardware_config: HardwareConfig) -> Self {
        let last_known_mount_pointing = hardware_config.last_pointing;
        Self {
            state,
            latest_frame: None,
            last_error: None,
            last_mount_error: None,
            last_known_mount_pointing,
            hardware_config,
            store,
            last_pointing_saved_ms: 0,
            latest_frame_seq: 0,
        }
    }

    /// Returns `(pointing, is_live)`: live telemetry when the mount is connected, otherwise
    /// the last known pointing snapshot (even across disconnects and app restarts).
    pub fn effective_mount_pointing(&self) -> Option<(MountPointing, bool)> {
        let is_live = self.state.connection == ConnectionState::Connected
            && self.state.mount == MountStatus::Ready;
        if is_live {
            if let Some(telemetry) = &self.state.mount_telemetry {
                return Some((MountPointing::from(telemetry), true));
            }
        }
        self.last_known_mount_pointing.map(|p| (p, false))
    }

    pub fn update_hardware_config(
        &mut self,
        f: impl FnOnce(&mut HardwareConfig),
        cx: &mut gpui::Context<Self>,
    ) {
        f(&mut self.hardware_config);
        self.save_hardware_config_async();
        cx.notify();
    }

    fn save_hardware_config_async(&self) {
        let store = self.store.clone();
        let cfg = self.hardware_config.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(err) = store.save(&cfg) {
                warn!("Failed to persist hardware config: {err}");
            }
        });
    }

    fn record_pointing(&mut self, pointing: MountPointing) {
        self.last_known_mount_pointing = Some(pointing);
        // Persist at most every 5 seconds to avoid excessive disk writes during slews.
        let now_ms = pointing.updated_at_unix_ms;
        if now_ms > 0
            && (now_ms < self.last_pointing_saved_ms
                || now_ms.saturating_sub(self.last_pointing_saved_ms) >= 5_000)
        {
            self.last_pointing_saved_ms = now_ms;
            self.hardware_config.last_pointing = Some(pointing);
            self.save_hardware_config_async();
        }
    }
}

/// Convenience helper to dispatch a [`MountCommand`] to the daemon over the WebSocket.
pub fn send_mount_command(cmd: MountCommand, cx: &App) -> bool {
    let Some(sender) = cx.try_global::<DaemonCommandSender>() else {
        warn!("Mount command ignored: DaemonCommandSender unavailable");
        return false;
    };
    if sender.0.try_send(ViewerMessage::Mount(cmd)).is_err() {
        warn!("Failed to enqueue MountCommand to daemon");
        return false;
    }
    true
}

#[derive(Clone)]
pub struct GlobalDaemonState(pub Entity<DaemonStateModel>);

impl Global for GlobalDaemonState {}

#[derive(Clone)]
pub struct DaemonAddress {
    url: String,
    tx: tokio::sync::watch::Sender<String>,
}

impl DaemonAddress {
    pub fn new(url: String, tx: tokio::sync::watch::Sender<String>) -> Self {
        Self { url, tx }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn set_url(&mut self, new_url: String) {
        if self.url != new_url {
            self.url.clone_from(&new_url);
            let _ = self.tx.send(new_url);
        }
    }
}

impl Global for DaemonAddress {}

#[derive(Clone)]
pub struct DaemonCommandSender(pub Sender<ViewerMessage>);

impl Global for DaemonCommandSender {}

/// Returns `true` if the viewer was launched with `--sim` / `--simulate` or `APOGEE_SIM=1`.
pub fn is_sim_mode() -> bool {
    std::env::args().any(|a| a == "--sim" || a == "--simulate")
        || matches!(
            std::env::var("APOGEE_SIM").ok().as_deref(),
            Some("1" | "true" | "TRUE" | "yes")
        )
}

fn telemetry_meaningfully_equal(a: Option<&MountTelemetry>, b: Option<&MountTelemetry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.ra_deg == b.ra_deg
                && a.dec_deg == b.dec_deg
                && a.az_deg == b.az_deg
                && a.alt_deg == b.alt_deg
                && a.pier_side == b.pier_side
                && a.tracking == b.tracking
                && a.is_slewing == b.is_slewing
                && a.is_parked == b.is_parked
                && a.slew_rate == b.slew_rate
                && a.target_ra_deg == b.target_ra_deg
                && a.target_dec_deg == b.target_dec_deg
                && a.driver == b.driver
                && a.port == b.port
                && a.firmware_version == b.firmware_version
                && a.mount_model == b.mount_model
                && a.limits == b.limits
                && a.park_az_deg == b.park_az_deg
                && a.park_alt_deg == b.park_alt_deg
        }
        _ => false,
    }
}

fn daemon_state_meaningfully_equal(a: &DaemonState, b: &DaemonState) -> bool {
    a.connection == b.connection
        && a.camera == b.camera
        && a.mount == b.mount
        && a.available_mount_ports == b.available_mount_ports
        && a.daemon == b.daemon
        && telemetry_meaningfully_equal(a.mount_telemetry.as_ref(), b.mount_telemetry.as_ref())
}

pub fn init(cx: &mut App) {
    let store = DocumentStore::open_default();
    let hardware_config: HardwareConfig = store.load_or_default();
    let sim_mode = is_sim_mode();

    let initial_url = std::env::var("APOGEE_DAEMON_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            if sim_mode {
                Some("ws://127.0.0.1:4000/ws".to_string())
            } else {
                hardware_config
                    .daemon_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            }
        })
        .unwrap_or_else(|| "ws://127.0.0.1:4000/ws".to_string());
    let (address_tx, address_rx) = tokio::sync::watch::channel(initial_url.clone());
    cx.set_global(DaemonAddress::new(initial_url, address_tx));

    let (cmd_tx, cmd_rx) = channel::<ViewerMessage>(64);
    cx.set_global(DaemonCommandSender(cmd_tx.clone()));

    let (update_tx, mut update_rx) = unbounded_channel::<ClientModelUpdate>();

    let state_model = cx.new(|cx| {
        let auto_sim_tx = sim_mode.then_some(cmd_tx);
        cx.spawn(async move |this, cx| {
            let mut auto_connect_sent = false;
            while let Some(update) = update_rx.recv().await {
                if this
                    .update(cx, |model: &mut DaemonStateModel, cx| {
                        let mut should_notify = true;
                        match update {
                            ClientModelUpdate::State {
                                state: new_state,
                                from_mount_status,
                            } => {
                                let new_state = *new_state;
                                if new_state.connection != ConnectionState::Connected {
                                    auto_connect_sent = false;
                                    model.last_mount_error = None;
                                } else if from_mount_status {
                                    if let Some(tx) = &auto_sim_tx {
                                        if !auto_connect_sent
                                            && new_state.mount == MountStatus::Disconnected
                                        {
                                            auto_connect_sent = true;
                                            let _ = tx.try_send(ViewerMessage::Mount(
                                                MountCommand::Connect {
                                                    driver: protocol::MountDriverKind::Simulator,
                                                    port: "simulator".to_string(),
                                                    baud_rate: 9600,
                                                },
                                            ));
                                        }
                                    }
                                }
                                if let Some(telemetry) = &new_state.mount_telemetry {
                                    model.record_pointing(MountPointing::from(telemetry));
                                }
                                // Only clear last_mount_error on transition into Ready, not on
                                // every 150ms telemetry tick while already Ready.
                                if model.state.mount != MountStatus::Ready
                                    && new_state.mount == MountStatus::Ready
                                {
                                    model.last_mount_error = None;
                                }
                                should_notify =
                                    !daemon_state_meaningfully_equal(&model.state, &new_state);
                                model.state = new_state;
                            }
                            ClientModelUpdate::Frame { seq, frame } => {
                                if seq > model.latest_frame_seq {
                                    model.latest_frame_seq = seq;
                                    model.latest_frame = Some(frame);
                                    model.last_error = None;
                                } else {
                                    should_notify = false;
                                }
                            }
                            ClientModelUpdate::Error { frame_id, message } => {
                                model.last_error = Some((frame_id, message));
                            }
                            ClientModelUpdate::MountError(message) => {
                                model.last_mount_error = Some(message);
                            }
                        }
                        if should_notify {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        DaemonStateModel::new(DaemonState::default(), store, hardware_config)
    });

    cx.set_global(GlobalDaemonState(state_model));

    tokio::spawn(async move {
        run_connection_supervisor(address_rx, update_tx, cmd_rx).await;
    });
}

enum PumpExit {
    AddressChanged,
    Shutdown,
    Disconnected(DisconnectReason),
}

enum DisconnectReason {
    WatchdogTimeout,
    PingSendFailed,
    CommandSendFailed,
    ClosedByPeer(String),
    WebSocketError(String),
    StreamClosedByServer,
}

impl std::fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisconnectReason::WatchdogTimeout => {
                write!(f, "Watchdog timeout: no response/pong from daemon for 15s")
            }
            DisconnectReason::PingSendFailed => {
                write!(f, "Failed to send WebSocket ping to daemon")
            }
            DisconnectReason::CommandSendFailed => {
                write!(f, "Failed to send command over WebSocket")
            }
            DisconnectReason::ClosedByPeer(frame) => write!(f, "Daemon closed connection: {frame}"),
            DisconnectReason::WebSocketError(err) => write!(f, "WebSocket error: {err}"),
            DisconnectReason::StreamClosedByServer => {
                write!(f, "WebSocket stream closed by server")
            }
        }
    }
}

fn reset_disconnected_state(state: &mut DaemonState) {
    state.camera = CameraStatus::Disconnected;
    state.mount = MountStatus::Disconnected;
    state.mount_telemetry = None;
    state.available_mount_ports.clear();
    state.daemon = DaemonStatus::Idle;
}

fn send_state(
    update_tx: &UnboundedSender<ClientModelUpdate>,
    state: &DaemonState,
    from_mount_status: bool,
) {
    let _ = update_tx.send(ClientModelUpdate::State {
        state: Box::new(state.clone()),
        from_mount_status,
    });
}

async fn run_connection_supervisor(
    mut address_rx: tokio::sync::watch::Receiver<String>,
    update_tx: UnboundedSender<ClientModelUpdate>,
    mut cmd_rx: Receiver<ViewerMessage>,
) {
    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_default();

    let mut attempt: u32 = 0;
    let mut last_backoff_ms: u64 = 0;
    let mut frame_seq: u64 = 0;
    let mut current_state = DaemonState::default();

    loop {
        if update_tx.is_closed() {
            info!("Client UI channel closed; shutting down connection supervisor.");
            return;
        }

        let current_url = address_rx.borrow().clone();
        attempt += 1;

        if attempt == 1 {
            current_state.connection = ConnectionState::Connecting;
        } else {
            current_state.connection = ConnectionState::Reconnecting {
                attempt,
                next_retry_ms: last_backoff_ms,
            };
        }
        reset_disconnected_state(&mut current_state);
        send_state(&update_tx, &current_state, false);

        info!("Connecting to daemon at {current_url} (attempt {attempt})...");

        let connect_future = tokio_tungstenite::connect_async(&current_url);
        tokio::pin!(connect_future);
        let connect_timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(connect_timeout);

        let connect_result = loop {
            tokio::select! {
                res = &mut connect_future => break Some(res),
                () = &mut connect_timeout => break None,
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(dropped) => {
                            warn!("Dropping queued command while connecting to daemon: {dropped:?}");
                        }
                        None => {
                            info!("Daemon command channel closed; exiting connection supervisor.");
                            return;
                        }
                    }
                }
                res = address_rx.changed() => {
                    if res.is_err() {
                        info!("Daemon address channel closed; client is shutting down. Exiting connection supervisor.");
                        return;
                    }
                    info!("Daemon address changed during connection attempt. Retrying with new address...");
                    attempt = 0;
                    last_backoff_ms = 0;
                    break None;
                }
                () = update_tx.closed() => {
                    info!("Client UI channel closed during connection attempt. Exiting connection supervisor.");
                    return;
                }
            }
        };

        if attempt == 0 {
            continue;
        }

        match connect_result {
            Some(Ok((ws_stream, _response))) => {
                info!("Successfully connected to daemon at {current_url}");
                attempt = 0;
                last_backoff_ms = 0;
                // Drain any stale commands queued while disconnected before publishing Connected.
                while cmd_rx.try_recv().is_ok() {}
                current_state.connection = ConnectionState::Connected;
                send_state(&update_tx, &current_state, false);

                let pump_exit = run_message_pump(
                    ws_stream,
                    &http_client,
                    &update_tx,
                    &mut current_state,
                    &mut cmd_rx,
                    &mut address_rx,
                    &mut frame_seq,
                )
                .await;

                match pump_exit {
                    PumpExit::Shutdown => {
                        info!("Client is shutting down. Exiting connection supervisor.");
                        return;
                    }
                    PumpExit::AddressChanged => {
                        info!("Daemon address changed by user. Reconnecting immediately...");
                        attempt = 0;
                        continue;
                    }
                    PumpExit::Disconnected(reason) => {
                        warn!("Daemon connection lost: {reason}. Initiating reconnect...");
                    }
                }
            }
            Some(Err(err)) => {
                warn!("Failed to connect to daemon at {current_url}: {err}");
            }
            None => {
                warn!("Connection to daemon at {current_url} timed out after 5s");
            }
        }

        let backoff = calculate_backoff_duration(attempt);
        last_backoff_ms = backoff.as_millis() as u64;
        current_state.connection = ConnectionState::Reconnecting {
            attempt,
            next_retry_ms: last_backoff_ms,
        };
        reset_disconnected_state(&mut current_state);
        send_state(&update_tx, &current_state, false);

        info!(
            "Waiting {:.1}s before reconnect attempt {}...",
            backoff.as_secs_f32(),
            attempt + 1
        );

        let backoff_sleep = tokio::time::sleep(backoff);
        tokio::pin!(backoff_sleep);
        loop {
            tokio::select! {
                () = &mut backoff_sleep => break,
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(dropped) => {
                            warn!("Dropping queued command while disconnected: {dropped:?}");
                        }
                        None => {
                            info!("Daemon command channel closed during backoff; exiting.");
                            return;
                        }
                    }
                }
                res = address_rx.changed() => {
                    if res.is_err() {
                        info!("Daemon address channel closed; client is shutting down. Exiting connection supervisor.");
                        return;
                    }
                    info!("Daemon address changed during backoff. Reconnecting immediately...");
                    attempt = 0;
                    last_backoff_ms = 0;
                    break;
                }
                () = update_tx.closed() => {
                    info!("Client UI channel closed during backoff. Exiting connection supervisor.");
                    return;
                }
            }
        }
    }
}

async fn run_message_pump(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    http_client: &reqwest::Client,
    update_tx: &UnboundedSender<ClientModelUpdate>,
    current_state: &mut DaemonState,
    cmd_rx: &mut Receiver<ViewerMessage>,
    address_rx: &mut tokio::sync::watch::Receiver<String>,
    frame_seq: &mut u64,
) -> PumpExit {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let mut ping_interval = tokio::time::interval(Duration::from_secs(5));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut last_activity = tokio::time::Instant::now();
    let watchdog_timeout = Duration::from_secs(15);

    loop {
        tokio::select! {
            res = address_rx.changed() => {
                if res.is_err() {
                    return PumpExit::Shutdown;
                }
                return PumpExit::AddressChanged;
            }

            _ = ping_interval.tick() => {
                if last_activity.elapsed() > watchdog_timeout {
                    return PumpExit::Disconnected(DisconnectReason::WatchdogTimeout);
                }
                if ws_sender.send(Message::Ping(vec![].into())).await.is_err() {
                    return PumpExit::Disconnected(DisconnectReason::PingSendFailed);
                }
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        match serde_json::to_string(&cmd) {
                            Ok(json) => {
                                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                    return PumpExit::Disconnected(DisconnectReason::CommandSendFailed);
                                }
                            }
                            Err(err) => {
                                error!("Failed to serialize ViewerMessage: {err}");
                            }
                        }
                    }
                    None => {
                        return PumpExit::Shutdown;
                    }
                }
            }

            () = update_tx.closed() => {
                return PumpExit::Shutdown;
            }

            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = tokio::time::Instant::now();
                        match serde_json::from_str::<DaemonMessage>(&text) {
                            Ok(daemon_msg) => {
                                match daemon_msg {
                                    DaemonMessage::Status(daemon_status) => {
                                        current_state.daemon = daemon_status;
                                        send_state(update_tx, current_state, false);
                                    }
                                    DaemonMessage::CameraStatus(camera_status) => {
                                        current_state.camera = camera_status;
                                        send_state(update_tx, current_state, false);
                                    }
                                    DaemonMessage::MountStatus(mount_status) => {
                                        if matches!(
                                            mount_status,
                                            MountStatus::Disconnected | MountStatus::Connecting { .. }
                                        ) {
                                            current_state.mount_telemetry = None;
                                        }
                                        current_state.mount = mount_status;
                                        send_state(update_tx, current_state, true);
                                    }
                                    DaemonMessage::MountTelemetry(telemetry) => {
                                        current_state.mount_telemetry = Some(telemetry);
                                        send_state(update_tx, current_state, false);
                                    }
                                    DaemonMessage::MountPorts(ports) => {
                                        current_state.available_mount_ports = ports;
                                        send_state(update_tx, current_state, false);
                                    }
                                    DaemonMessage::MountError { message } => {
                                        warn!("Daemon reported mount error: {message}");
                                        let _ = update_tx.send(ClientModelUpdate::MountError(message));
                                    }
                                    DaemonMessage::SequenceProgress { .. }
                                    | DaemonMessage::PreviewAvailable { .. } => {}
                                    DaemonMessage::FrameReady { frame_id, url } => {
                                        if url.trim().is_empty() {
                                            info!("Daemon reported frame {frame_id} ready (no viewer download URL)");
                                            continue;
                                        }
                                        info!("Daemon reported frame {frame_id} ready at {url}. Streaming...");
                                        let ws_url = address_rx.borrow().clone();
                                        let Some(full_url) = build_frame_url(&ws_url, &url) else {
                                            error!("Invalid frame URL '{url}' for daemon address '{ws_url}'");
                                            continue;
                                        };

                                        *frame_seq += 1;
                                        let seq = *frame_seq;
                                        let client = http_client.clone();
                                        let tx = update_tx.clone();
                                        tokio::spawn(async move {
                                            match stream_frame_from_daemon(&client, &full_url).await {
                                                Ok(bytes) => {
                                                    info!("Successfully streamed frame {frame_id} ({} bytes)", bytes.len());
                                                    let _ = tx.send(ClientModelUpdate::Frame {
                                                        seq,
                                                        frame: CapturedFrame {
                                                            frame_id,
                                                            data: std::sync::Arc::new(bytes),
                                                        },
                                                    });
                                                }
                                                Err(err) => {
                                                    error!("Failed to stream frame {frame_id} from {full_url}: {err}");
                                                }
                                            }
                                        });
                                    }
                                    DaemonMessage::CaptureError { frame_id, message } => {
                                        error!("Daemon reported capture error for frame {frame_id}: {message}");
                                        let _ = update_tx.send(ClientModelUpdate::Error { frame_id, message });
                                    }
                                }
                            }
                            Err(err) => {
                                warn!("Failed to parse DaemonMessage: {err}");
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_activity = tokio::time::Instant::now();
                    }
                    Some(Ok(Message::Ping(data))) => {
                        last_activity = tokio::time::Instant::now();
                        if ws_sender.send(Message::Pong(data)).await.is_err() {
                            return PumpExit::Disconnected(DisconnectReason::PingSendFailed);
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return PumpExit::Disconnected(DisconnectReason::ClosedByPeer(format!("{frame:?}")));
                    }
                    Some(Err(err)) => {
                        return PumpExit::Disconnected(DisconnectReason::WebSocketError(err.to_string()));
                    }
                    None => {
                        return PumpExit::Disconnected(DisconnectReason::StreamClosedByServer);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn build_frame_url(ws_url: &str, path_or_url: &str) -> Option<String> {
    if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        return Some(path_or_url.to_string());
    }
    let normalized_ws = if ws_url.contains("://") {
        std::borrow::Cow::Borrowed(ws_url)
    } else {
        std::borrow::Cow::Owned(format!("ws://{ws_url}"))
    };
    let mut base = reqwest::Url::parse(&normalized_ws).ok()?;
    let http_scheme = match base.scheme() {
        "wss" | "https" => "https",
        _ => "http",
    };
    base.set_scheme(http_scheme).ok()?;
    let trimmed_path = base.path().trim_end_matches('/');
    let base_path = trimmed_path.strip_suffix("/ws").unwrap_or(trimmed_path);
    let new_path = if base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{base_path}/")
    };
    base.set_path(&new_path);
    base.set_query(None);
    base.set_fragment(None);
    base.join(path_or_url.trim_start_matches('/'))
        .ok()
        .map(|u| u.to_string())
}

fn calculate_backoff_duration(attempt: u32) -> Duration {
    let base_ms = 500u64 * (1u64 << attempt.min(5).saturating_sub(1));
    let capped_ms = base_ms.min(10_000);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_factor = 0.8 + ((nanos % 40) as f64 / 100.0);
    let jittered_ms = (capped_ms as f64 * jitter_factor) as u64;

    Duration::from_millis(jittered_ms.max(200))
}

async fn stream_frame_from_daemon(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    use futures::StreamExt;
    const MAX_PREALLOC_BYTES: usize = 256 * 1024 * 1024;
    const MAX_FRAME_BYTES: usize = 512 * 1024 * 1024;

    let response = client.get(url).send().await?.error_for_status()?;
    let content_length = response
        .content_length()
        .unwrap_or(0)
        .min(MAX_PREALLOC_BYTES as u64) as usize;
    let mut buffer = Vec::with_capacity(content_length);
    let mut stream = response.bytes_stream();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res?;
        if buffer.len().saturating_add(chunk.len()) > MAX_FRAME_BYTES {
            bail!("Frame download exceeded maximum allowed size ({MAX_FRAME_BYTES} bytes)");
        }
        buffer.extend_from_slice(&chunk);
    }

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_frame_url_handles_ws_wss_and_absolute_urls() {
        assert_eq!(
            build_frame_url("ws://127.0.0.1:4000/ws", "/api/frames/42/raw").as_deref(),
            Some("http://127.0.0.1:4000/api/frames/42/raw")
        );
        assert_eq!(
            build_frame_url("wss://scope.local/prefix/ws", "/api/frames/7/raw").as_deref(),
            Some("https://scope.local/prefix/api/frames/7/raw")
        );
        assert_eq!(
            build_frame_url("ws://127.0.0.1:4000/ws", "https://cdn.example/f.raw").as_deref(),
            Some("https://cdn.example/f.raw")
        );
    }

    #[test]
    fn reset_disconnected_state_clears_ports_and_telemetry() {
        let mut state = DaemonState {
            available_mount_ports: vec!["/dev/ttyUSB0".into()],
            mount_telemetry: Some(MountTelemetry::default()),
            mount: MountStatus::Ready,
            ..DaemonState::default()
        };
        reset_disconnected_state(&mut state);
        assert_eq!(state.available_mount_ports, Vec::<String>::new());
        assert!(state.mount_telemetry.is_none());
        assert_eq!(state.mount, MountStatus::Disconnected);
    }
}
