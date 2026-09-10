use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use controller::{ControllerEvent, ControllerHandle};
use futures::{SinkExt, StreamExt};
use protocol::{CameraStatus, DaemonMessage, DaemonStatus, ViewerMessage};
use serde::Serialize;
use tokio::sync::{RwLock, broadcast};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};

pub struct FrameCacheEntry {
    pub data: Vec<u8>,
    pub mime_type: String,
}

pub struct ServerState {
    pub controller: ControllerHandle,
    pub broadcast_tx: broadcast::Sender<DaemonMessage>,
    pub frame_cache: Arc<RwLock<HashMap<u64, FrameCacheEntry>>>,
    pub latest_preview: Arc<RwLock<Option<(u64, Vec<u8>)>>>,
    pub active_viewers_count: Arc<AtomicUsize>,
}

impl ServerState {
    pub fn new(controller: ControllerHandle) -> Self {
        let (broadcast_tx, _) = broadcast::channel(128);

        Self {
            controller,
            broadcast_tx,
            frame_cache: Arc::new(RwLock::new(HashMap::new())),
            latest_preview: Arc::new(RwLock::new(None)),
            active_viewers_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn broadcast(&self, msg: DaemonMessage) {
        let _ = self.broadcast_tx.send(msg);
    }

    #[allow(dead_code)]
    pub async fn store_frame(&self, frame_id: u64, data: Vec<u8>, mime_type: String) {
        const MAX_CACHED_FRAMES: usize = 10;
        let mut cache = self.frame_cache.write().await;
        if cache.len() >= MAX_CACHED_FRAMES {
            if let Some(&oldest_id) = cache.keys().min() {
                cache.remove(&oldest_id);
            }
        }
        cache.insert(frame_id, FrameCacheEntry { data, mime_type });
    }

    #[allow(dead_code)]
    pub async fn set_latest_preview(&self, frame_id: u64, bytes: Vec<u8>) {
        *self.latest_preview.write().await = Some((frame_id, bytes));
    }
}

pub fn create_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/api/status", get(status_handler))
        .route("/api/frames/{id}/raw", get(frame_raw_handler))
        .route("/api/frames/{id}/preview", get(frame_preview_handler))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

#[derive(Serialize)]
struct StatusResponse {
    daemon: DaemonStatus,
    camera: CameraStatus,
    viewers: usize,
}

async fn status_handler(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let daemon = state.controller.status().borrow().clone();
    let camera = state.controller.camera_status().borrow().clone();
    let viewers = state.active_viewers_count.load(Ordering::Relaxed);

    axum::Json(StatusResponse {
        daemon,
        camera,
        viewers,
    })
}

async fn frame_raw_handler(Path(id): Path<u64>, State(state): State<Arc<ServerState>>) -> Response {
    let cache = state.frame_cache.read().await;
    if let Some(entry) = cache.get(&id) {
        let mut response = (StatusCode::OK, entry.data.clone()).into_response();
        if let Ok(mime) = HeaderValue::from_str(&entry.mime_type) {
            response.headers_mut().insert(header::CONTENT_TYPE, mime);
        }
        let filename = format!("attachment; filename=\"frame_{id}.raw\"");
        if let Ok(disp) = HeaderValue::from_str(&filename) {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, disp);
        }
        if let Ok(len) = HeaderValue::from_str(&entry.data.len().to_string()) {
            response.headers_mut().insert(header::CONTENT_LENGTH, len);
        }
        response
    } else {
        (
            StatusCode::NOT_FOUND,
            format!("Frame {id} not found in cache"),
        )
            .into_response()
    }
}

async fn frame_preview_handler(
    Path(id): Path<u64>,
    State(state): State<Arc<ServerState>>,
) -> Response {
    let preview_guard = state.latest_preview.read().await;
    if let Some((preview_id, bytes)) = preview_guard.as_ref() {
        if *preview_id == id {
            let mut response = (StatusCode::OK, bytes.clone()).into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            return response;
        }
    }
    (
        StatusCode::NOT_FOUND,
        format!("Preview for frame {id} not found"),
    )
        .into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<ServerState>>) -> Response {
    ws.on_upgrade(move |socket| handle_viewer_socket(socket, state))
}

async fn handle_viewer_socket(socket: WebSocket, state: Arc<ServerState>) {
    let viewer_idx = state.active_viewers_count.fetch_add(1, Ordering::SeqCst) + 1;
    info!("Viewer connected via WebSocket (active viewers: {viewer_idx})");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.broadcast_tx.subscribe();

    let initial_status = state.controller.status().borrow().clone();
    let camera_status = state.controller.camera_status().borrow().clone();

    if let Ok(text) = serde_json::to_string(&DaemonMessage::Status(initial_status)) {
        if ws_sender.send(Message::Text(text.into())).await.is_err() {
            state.active_viewers_count.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    }

    if let Ok(text) = serde_json::to_string(&DaemonMessage::CameraStatus(camera_status)) {
        if ws_sender.send(Message::Text(text.into())).await.is_err() {
            state.active_viewers_count.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    }

    if let Some((frame_id, bytes)) = state.latest_preview.read().await.as_ref() {
        let preview_msg = DaemonMessage::PreviewAvailable {
            frame_id: *frame_id,
            image_bytes: bytes.clone(),
        };
        if let Ok(text) = serde_json::to_string(&preview_msg) {
            let _ = ws_sender.send(Message::Text(text.into())).await;
        }
    }

    let mut send_task = tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(msg) => match serde_json::to_string(&msg) {
                    Ok(text) => {
                        if ws_sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        error!("Failed to serialize DaemonMessage: {err}");
                    }
                },
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("WebSocket broadcast receiver lagged by {skipped} messages; continuing");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    let incoming_state = state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg_result) = ws_receiver.next().await {
            match msg_result {
                Ok(Message::Text(text)) => match serde_json::from_str::<ViewerMessage>(&text) {
                    Ok(viewer_msg) => {
                        handle_viewer_command(viewer_msg, &incoming_state).await;
                    }
                    Err(err) => {
                        warn!("Invalid ViewerMessage JSON received: {err}");
                    }
                },
                Ok(Message::Close(_)) => {
                    info!("Viewer sent WebSocket close frame");
                    break;
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(Message::Binary(_)) => {
                    warn!("Unexpected binary message from viewer");
                }
                Err(err) => {
                    warn!("WebSocket receive error: {err}");
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
        }
        _ = (&mut recv_task) => {
            send_task.abort();
        }
    }

    let remaining = state.active_viewers_count.fetch_sub(1, Ordering::SeqCst) - 1;
    info!("Viewer disconnected (remaining active viewers: {remaining})");
}

async fn handle_viewer_command(msg: ViewerMessage, state: &Arc<ServerState>) {
    match msg {
        ViewerMessage::Connect => {
            info!("Received explicit Connect handshake from viewer");
            let daemon = state.controller.status().borrow().clone();
            let camera = state.controller.camera_status().borrow().clone();
            state.broadcast(DaemonMessage::Status(daemon));
            state.broadcast(DaemonMessage::CameraStatus(camera));
        }
        ViewerMessage::StartCapture(frame) => {
            info!(
                "Forwarding StartCapture for frame {} ({:?}, {:.1}s) to controller",
                frame.id,
                frame.frame_type,
                frame.params.exposure.as_secs_f32()
            );
            if let Err(err) = state.controller.start_capture(frame).await {
                error!("Failed to forward StartCapture to controller: {err}");
            }
        }
        ViewerMessage::Abort => {
            warn!("Forwarding Abort command to controller");
            if let Err(err) = state.controller.abort().await {
                error!("Failed to forward Abort to controller: {err}");
            }
        }
    }
}

pub fn spawn_controller_event_forwarder(state: Arc<ServerState>) {
    let mut event_rx = state.controller.subscribe_events();
    tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            match event {
                ControllerEvent::Status(status) => {
                    state.broadcast(DaemonMessage::Status(status));
                }
                ControllerEvent::FrameCaptured(captured) => {
                    let filename = &captured.file_ref.filename;
                    let mime = if filename.to_lowercase().ends_with(".cr2") {
                        "image/x-canon-cr2"
                    } else if filename.to_lowercase().ends_with(".cr3") {
                        "image/x-canon-cr3"
                    } else {
                        "application/octet-stream"
                    };
                    if let Some(data) = captured.data {
                        state.store_frame(captured.frame.id, data, mime.into()).await;
                        let raw_url = format!("/api/frames/{}/raw", captured.frame.id);
                        state.broadcast(DaemonMessage::FrameReady {
                            frame_id: captured.frame.id,
                            url: raw_url,
                        });
                    } else {
                        state.broadcast(DaemonMessage::FrameReady {
                            frame_id: captured.frame.id,
                            url: String::new(),
                        });
                    }
                }
                ControllerEvent::CaptureError { frame_id, error } => {
                    state.broadcast(DaemonMessage::CaptureError {
                        frame_id,
                        message: error,
                    });
                }
            }
        }
    });
}

pub fn spawn_camera_status_watcher(state: Arc<ServerState>) {
    let mut status_rx = state.controller.camera_status();
    tokio::spawn(async move {
        while status_rx.changed().await.is_ok() {
            let new_status = status_rx.borrow().clone();
            info!("Camera status changed: {:?}", new_status);
            state.broadcast(DaemonMessage::CameraStatus(new_status));
        }
    });
}

pub async fn start_server(addr: SocketAddr, controller: ControllerHandle) -> anyhow::Result<()> {
    let state = Arc::new(ServerState::new(controller));
    spawn_camera_status_watcher(state.clone());
    spawn_controller_event_forwarder(state.clone());

    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Daemon listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
