use std::time::Duration;

use gpui::{App, AppContext, Entity, Global};
use protocol::{CameraStatus, ConnectionState, DaemonMessage, DaemonState, ViewerMessage};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedSender, channel, unbounded_channel};
use tracing::{error, info, warn};

pub struct DaemonStateModel {
    pub state: DaemonState,
}

impl DaemonStateModel {
    pub fn new(state: DaemonState) -> Self {
        Self { state }
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
        self.url.clone_from(&new_url);
        let _ = self.tx.send(new_url);
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

    let (status_tx, mut status_rx) = unbounded_channel::<DaemonState>();

    let state_model = cx.new(|cx| {
        cx.spawn(async move |this, cx| {
            while let Some(new_state) = status_rx.recv().await {
                if this
                    .update(cx, |model: &mut DaemonStateModel, cx| {
                        model.state = new_state;
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
        run_connection_supervisor(address_rx, status_tx, cmd_rx).await;
    });
}

async fn run_connection_supervisor(
    mut address_rx: tokio::sync::watch::Receiver<String>,
    status_tx: UnboundedSender<DaemonState>,
    mut cmd_rx: Receiver<ViewerMessage>,
) {
    let mut attempt: u32 = 0;
    let mut current_state = DaemonState::default();

    loop {
        let current_url = address_rx.borrow().clone();
        attempt += 1;

        if attempt == 1 {
            current_state.connection = ConnectionState::Connecting;
        } else {
            let next_retry_ms = calculate_backoff_ms(attempt);
            current_state.connection = ConnectionState::Reconnecting {
                attempt,
                next_retry_ms,
            };
        }
        current_state.camera = CameraStatus::Disconnected;
        let _ = status_tx.send(current_state.clone());

        info!("Connecting to daemon at {current_url} (attempt {attempt})...");

        let connect_future = tokio_tungstenite::connect_async(&current_url);
        tokio::pin!(connect_future);

        let connect_result = tokio::select! {
            res = &mut connect_future => Some(res),
            () = tokio::time::sleep(Duration::from_secs(5)) => None,
            _ = address_rx.changed() => {
                info!("Daemon address changed during connection attempt. Retrying with new address...");
                attempt = 0;
                continue;
            }
        };

        match connect_result {
            Some(Ok((ws_stream, _response))) => {
                info!("Successfully connected to daemon at {current_url}");
                attempt = 0;
                current_state.connection = ConnectionState::Connected;
                let _ = status_tx.send(current_state.clone());

                let disconnect_reason = run_message_pump(
                    ws_stream,
                    &status_tx,
                    &mut current_state,
                    &mut cmd_rx,
                    &mut address_rx,
                )
                .await;

                warn!("Daemon connection lost: {disconnect_reason}. Initiating reconnect...");
            }
            Some(Err(err)) => {
                warn!("Failed to connect to daemon at {current_url}: {err}");
            }
            None => {
                warn!("Connection to daemon at {current_url} timed out after 5s");
            }
        }

        let backoff = calculate_backoff_duration(attempt);
        let next_retry_ms = backoff.as_millis() as u64;
        current_state.connection = ConnectionState::Reconnecting {
            attempt,
            next_retry_ms,
        };
        current_state.camera = CameraStatus::Disconnected;
        let _ = status_tx.send(current_state.clone());

        info!(
            "Waiting {:.1}s before reconnect attempt {}...",
            backoff.as_secs_f32(),
            attempt + 1
        );

        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            _ = address_rx.changed() => {
                info!("Daemon address changed during backoff. Reconnecting immediately...");
                attempt = 0;
            }
        }
    }
}

async fn run_message_pump(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    status_tx: &UnboundedSender<DaemonState>,
    current_state: &mut DaemonState,
    cmd_rx: &mut Receiver<ViewerMessage>,
    address_rx: &mut tokio::sync::watch::Receiver<String>,
) -> String {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let mut ping_interval = tokio::time::interval(Duration::from_secs(5));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut last_activity = tokio::time::Instant::now();
    let watchdog_timeout = Duration::from_secs(15);

    loop {
        tokio::select! {
            _ = address_rx.changed() => {
                return "Target daemon address changed by user".to_string();
            }

            _ = ping_interval.tick() => {
                if last_activity.elapsed() > watchdog_timeout {
                    return "Watchdog timeout: no response/pong from daemon for 15s".to_string();
                }
                if ws_sender.send(Message::Ping(vec![].into())).await.is_err() {
                    return "Failed to send WebSocket ping to daemon".to_string();
                }
            }

            Some(cmd) = cmd_rx.recv() => {
                match serde_json::to_string(&cmd) {
                    Ok(json) => {
                        if ws_sender.send(Message::Text(json.into())).await.is_err() {
                            return "Failed to send command over WebSocket".to_string();
                        }
                    }
                    Err(err) => {
                        error!("Failed to serialize ViewerMessage: {err}");
                    }
                }
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
                                        let _ = status_tx.send(current_state.clone());
                                    }
                                    DaemonMessage::CameraStatus(camera_status) => {
                                        current_state.camera = camera_status;
                                        let _ = status_tx.send(current_state.clone());
                                    }
                                    DaemonMessage::SequenceProgress { .. } => {}
                                    DaemonMessage::PreviewAvailable { .. } => {}
                                    DaemonMessage::FrameReady { .. } => {}
                                    DaemonMessage::CaptureError { .. } => {}
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
                        return format!("Daemon closed connection: {frame:?}");
                    }
                    Some(Err(err)) => {
                        return format!("WebSocket error: {err}");
                    }
                    None => {
                        return "WebSocket stream closed by server".to_string();
                    }
                    _ => {}
                }
            }
        }
    }
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

fn calculate_backoff_ms(attempt: u32) -> u64 {
    calculate_backoff_duration(attempt).as_millis() as u64
}
