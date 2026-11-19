use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::bail;
use gpui::{App, AppContext, Entity, Global};
use protocol::{
    CameraStatus, ConnectionState, DaemonMessage, DaemonState, DaemonStatus, ViewerMessage,
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
    State(DaemonState),
    Frame { seq: u64, frame: CapturedFrame },
    Error { frame_id: u64, message: String },
}

pub struct DaemonStateModel {
    pub state: DaemonState,
    pub latest_frame: Option<CapturedFrame>,
    pub last_error: Option<(u64, String)>,
    latest_frame_seq: u64,
}

impl DaemonStateModel {
    pub fn new(state: DaemonState) -> Self {
        Self {
            state,
            latest_frame: None,
            last_error: None,
            latest_frame_seq: 0,
        }
    }
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

pub fn init(cx: &mut App) {
    let initial_url =
        std::env::var("APOGEE_DAEMON_URL").unwrap_or_else(|_| "ws://127.0.0.1:4000/ws".to_string());
    let (address_tx, address_rx) = tokio::sync::watch::channel(initial_url.clone());
    cx.set_global(DaemonAddress::new(initial_url, address_tx));

    let (cmd_tx, cmd_rx) = channel::<ViewerMessage>(32);
    cx.set_global(DaemonCommandSender(cmd_tx));

    let (update_tx, mut update_rx) = unbounded_channel::<ClientModelUpdate>();

    let state_model = cx.new(|cx| {
        cx.spawn(async move |this, cx| {
            while let Some(update) = update_rx.recv().await {
                if this
                    .update(cx, |model: &mut DaemonStateModel, cx| {
                        match update {
                            ClientModelUpdate::State(new_state) => {
                                model.state = new_state;
                            }
                            ClientModelUpdate::Frame { seq, frame } => {
                                if seq > model.latest_frame_seq {
                                    model.latest_frame_seq = seq;
                                    model.latest_frame = Some(frame);
                                    model.last_error = None;
                                }
                            }
                            ClientModelUpdate::Error { frame_id, message } => {
                                model.last_error = Some((frame_id, message));
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        DaemonStateModel::new(DaemonState::default())
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
        current_state.camera = CameraStatus::Disconnected;
        current_state.daemon = DaemonStatus::Idle;
        let _ = update_tx.send(ClientModelUpdate::State(current_state.clone()));

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
                current_state.connection = ConnectionState::Connected;
                let _ = update_tx.send(ClientModelUpdate::State(current_state.clone()));

                while cmd_rx.try_recv().is_ok() {}

                let pump_exit = run_message_pump(
                    ws_stream,
                    &http_client,
                    &update_tx,
                    &mut current_state,
                    &mut cmd_rx,
                    &mut address_rx,
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
        current_state.camera = CameraStatus::Disconnected;
        current_state.daemon = DaemonStatus::Idle;
        let _ = update_tx.send(ClientModelUpdate::State(current_state.clone()));

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
) -> PumpExit {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let mut ping_interval = tokio::time::interval(Duration::from_secs(5));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut last_activity = tokio::time::Instant::now();
    let watchdog_timeout = Duration::from_secs(15);
    let mut frame_seq: u64 = 0;

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
                                        let _ = update_tx.send(ClientModelUpdate::State(current_state.clone()));
                                    }
                                    DaemonMessage::CameraStatus(camera_status) => {
                                        current_state.camera = camera_status;
                                        let _ = update_tx.send(ClientModelUpdate::State(current_state.clone()));
                                    }
                                    DaemonMessage::SequenceProgress { .. } => {}
                                    DaemonMessage::PreviewAvailable { .. } => {}
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

                                        frame_seq += 1;
                                        let seq = frame_seq;
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
                        let _ = ws_sender.send(Message::Pong(data)).await;
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
    let _ = base.set_scheme(http_scheme);
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
