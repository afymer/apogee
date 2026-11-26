use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::{
        Path as AxumPath, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use controller::{CapturedFrame, ControllerEvent, ControllerHandle};
use futures::{SinkExt, StreamExt};
use protocol::{
    CameraCapabilities, CameraStatus, DaemonMessage, DaemonStatus, FrameType, MountStatus,
    MountTelemetry, SequenceStatus, ViewerMessage,
};
use serde::Serialize;
use synscan::MountEvent;
use tokio::sync::{RwLock, broadcast};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};

pub struct FrameCacheEntry {
    pub data: axum::body::Bytes,
    pub mime_type: String,
    pub filename: String,
}

#[derive(Default)]
pub struct FrameCache {
    entries: HashMap<u64, FrameCacheEntry>,
    order: VecDeque<u64>,
}

impl FrameCache {
    fn insert(&mut self, frame_id: u64, entry: FrameCacheEntry) {
        const MAX_CACHED_FRAMES: usize = 10;

        if let std::collections::hash_map::Entry::Occupied(mut occupied) =
            self.entries.entry(frame_id)
        {
            occupied.insert(entry);
            return;
        }

        while self.entries.len() >= MAX_CACHED_FRAMES {
            if let Some(oldest_id) = self.order.pop_front() {
                self.entries.remove(&oldest_id);
            } else {
                break;
            }
        }

        self.entries.insert(frame_id, entry);
        self.order.push_back(frame_id);
    }

    fn get(&self, frame_id: u64) -> Option<&FrameCacheEntry> {
        self.entries.get(&frame_id)
    }
}

pub struct ServerState {
    pub controller: ControllerHandle,
    pub broadcast_tx: broadcast::Sender<DaemonMessage>,
    pub frame_cache: Arc<RwLock<FrameCache>>,
    pub latest_preview: Arc<RwLock<Option<(u64, Vec<u8>)>>>,
    pub active_viewers_count: Arc<AtomicUsize>,
    pub capture_dir: PathBuf,
}

fn default_capture_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("APOGEE_CAPTURE_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }

    #[cfg(test)]
    {
        std::env::temp_dir().join("apogee-daemon-test-captures")
    }

    #[cfg(not(test))]
    {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            let p = PathBuf::from(data_home);
            if !p.as_os_str().is_empty() {
                return p.join("apogee").join("captures");
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("apogee")
                .join("captures");
        }
        std::env::temp_dir().join("apogee-captures")
    }
}

fn sanitize_filename_component(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('.');
    if trimmed.is_empty() || trimmed.contains("..") {
        "frame".to_string()
    } else {
        trimmed.to_string()
    }
}

fn build_capture_filename(captured: &CapturedFrame, template_override: Option<&str>) -> String {
    let frame = &captured.frame;
    let template = template_override.unwrap_or(&frame.save_config.common_template);
    let frame_type = match frame.frame_type {
        FrameType::Light => "light",
        FrameType::Dark => "dark",
        FrameType::Flat => "flat",
        FrameType::Bias => "bias",
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let exposure_ms = frame.params.exposure.as_millis();

    let rendered = template
        .replace("{target}", &frame.target_name)
        .replace("{frame_type}", frame_type)
        .replace("{exposure_ms}", &exposure_ms.to_string())
        .replace("{timestamp}", &timestamp.to_string())
        .replace("{frame_id}", &frame.id.to_string());

    let stem = sanitize_filename_component(&rendered);
    let ext = Path::new(&captured.file_ref.filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        })
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "raw".to_string());

    format!("{stem}.{ext}")
}

fn detect_raw_mime_type(filename: &str) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("cr2") => "image/x-canon-cr2",
        Some("cr3") => "image/x-canon-cr3",
        Some("nef") => "image/x-nikon-nef",
        Some("arw") => "image/x-sony-arw",
        Some("dng") => "image/x-adobe-dng",
        Some("orf") => "image/x-olympus-orf",
        Some("raf") => "image/x-fuji-raf",
        Some("fits" | "fit") => "application/fits",
        _ => "application/octet-stream",
    }
}

impl ServerState {
    pub fn new(controller: ControllerHandle) -> Self {
        let (broadcast_tx, _) = broadcast::channel(128);

        Self {
            controller,
            broadcast_tx,
            frame_cache: Arc::new(RwLock::new(FrameCache::default())),
            latest_preview: Arc::new(RwLock::new(None)),
            active_viewers_count: Arc::new(AtomicUsize::new(0)),
            capture_dir: default_capture_dir(),
        }
    }

    pub fn broadcast(&self, msg: DaemonMessage) {
        let _ = self.broadcast_tx.send(msg);
    }

    #[cfg(test)]
    pub async fn store_frame(&self, frame_id: u64, data: Vec<u8>, mime_type: String) {
        self.store_frame_with_name(frame_id, format!("frame_{frame_id}.raw"), data, mime_type)
            .await;
    }

    pub async fn store_frame_with_name(
        &self,
        frame_id: u64,
        filename: String,
        data: impl Into<axum::body::Bytes>,
        mime_type: String,
    ) {
        let mut cache = self.frame_cache.write().await;
        cache.insert(
            frame_id,
            FrameCacheEntry {
                data: data.into(),
                mime_type,
                filename,
            },
        );
    }

    pub async fn persist_frame_to_disk(
        &self,
        captured: &CapturedFrame,
        data: &[u8],
    ) -> std::io::Result<PathBuf> {
        static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

        tokio::fs::create_dir_all(&self.capture_dir).await?;

        let template_override = captured
            .frame
            .save_config
            .server
            .as_ref()
            .and_then(|s| s.override_template.as_deref());
        let filename = build_capture_filename(captured, template_override);
        let mut dest_path = self.capture_dir.join(&filename);
        if tokio::fs::try_exists(&dest_path).await.unwrap_or(false) {
            let stem = dest_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("frame")
                .to_string();
            let ext = dest_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("raw")
                .to_string();
            let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
            dest_path = self
                .capture_dir
                .join(format!("{stem}_{}_{seq}.{ext}", captured.frame.id));
        }

        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.capture_dir.join(format!(
            ".{}.{}.{}.tmp",
            captured.frame.id,
            std::process::id(),
            seq
        ));

        tokio::fs::write(&tmp_path, data).await?;
        if let Err(err) = tokio::fs::rename(&tmp_path, &dest_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(err);
        }

        Ok(dest_path)
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
    camera_capabilities: Option<CameraCapabilities>,
    mount: MountStatus,
    mount_telemetry: Option<MountTelemetry>,
    sequence: Option<SequenceStatus>,
    viewers: usize,
}

async fn status_handler(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let daemon = state.controller.status().borrow().clone();
    let camera = state.controller.camera_status().borrow().clone();
    let camera_capabilities = state.controller.camera_capabilities().borrow().clone();
    let mount = state.controller.mount_status().borrow().clone();
    let mount_telemetry = state.controller.mount_telemetry().borrow().clone();
    let sequence = state.controller.sequence_status().borrow().clone();
    let viewers = state.active_viewers_count.load(Ordering::Relaxed);

    axum::Json(StatusResponse {
        daemon,
        camera,
        camera_capabilities,
        mount,
        mount_telemetry,
        sequence,
        viewers,
    })
}

async fn frame_raw_handler(
    AxumPath(id): AxumPath<u64>,
    State(state): State<Arc<ServerState>>,
) -> Response {
    let cache = state.frame_cache.read().await;
    if let Some(entry) = cache.get(id) {
        let mut response = (StatusCode::OK, entry.data.clone()).into_response();
        if let Ok(mime) = HeaderValue::from_str(&entry.mime_type) {
            response.headers_mut().insert(header::CONTENT_TYPE, mime);
        }
        let safe_name = sanitize_filename_component(&entry.filename);
        let disposition = format!("attachment; filename=\"{safe_name}\"");
        if let Ok(disp) = HeaderValue::from_str(&disposition) {
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
    AxumPath(id): AxumPath<u64>,
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

struct ViewerCountGuard {
    counter: Arc<AtomicUsize>,
}

impl ViewerCountGuard {
    fn acquire(counter: Arc<AtomicUsize>) -> (Self, usize) {
        let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
        (Self { counter }, count)
    }
}

impl Drop for ViewerCountGuard {
    fn drop(&mut self) {
        let remaining = self.counter.fetch_sub(1, Ordering::SeqCst) - 1;
        info!("Viewer disconnected (remaining active viewers: {remaining})");
    }
}

fn collect_snapshot_messages(state: &ServerState) -> Vec<DaemonMessage> {
    let mut msgs = Vec::with_capacity(6);
    msgs.push(DaemonMessage::Status(
        state.controller.status().borrow().clone(),
    ));
    msgs.push(DaemonMessage::CameraStatus(
        state.controller.camera_status().borrow().clone(),
    ));
    msgs.push(DaemonMessage::CameraCapabilities(
        state.controller.camera_capabilities().borrow().clone(),
    ));
    msgs.push(DaemonMessage::MountStatus(
        state.controller.mount_status().borrow().clone(),
    ));
    if let Some(telemetry) = state.controller.mount_telemetry().borrow().clone() {
        msgs.push(DaemonMessage::MountTelemetry(telemetry));
    }
    if let Some(sequence) = state.controller.sequence_status().borrow().clone() {
        msgs.push(DaemonMessage::SequenceStatus(sequence));
    }
    msgs
}

async fn send_daemon_message(
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    msg: &DaemonMessage,
) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => ws_sender.send(Message::Text(text.into())).await.is_ok(),
        Err(err) => {
            error!("Failed to serialize DaemonMessage: {err}");
            true
        }
    }
}

async fn handle_viewer_socket(socket: WebSocket, state: Arc<ServerState>) {
    let (_guard, viewer_idx) = ViewerCountGuard::acquire(state.active_viewers_count.clone());
    info!("Viewer connected via WebSocket (active viewers: {viewer_idx})");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    let (direct_tx, mut direct_rx) = tokio::sync::mpsc::unbounded_channel::<DaemonMessage>();

    for msg in collect_snapshot_messages(&state) {
        if !send_daemon_message(&mut ws_sender, &msg).await {
            return;
        }
    }

    if let Some((frame_id, bytes)) = state.latest_preview.read().await.as_ref() {
        let preview_msg = DaemonMessage::PreviewAvailable {
            frame_id: *frame_id,
            image_bytes: bytes.clone(),
        };
        if !send_daemon_message(&mut ws_sender, &preview_msg).await {
            return;
        }
    }

    let mut send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                direct = direct_rx.recv() => {
                    match direct {
                        Some(msg) => {
                            if !send_daemon_message(&mut ws_sender, &msg).await {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                res = broadcast_rx.recv() => {
                    match res {
                        Ok(msg) => {
                            if !send_daemon_message(&mut ws_sender, &msg).await {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!("WebSocket broadcast receiver lagged by {skipped} messages; continuing");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
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
                        handle_viewer_command(viewer_msg, &incoming_state, &direct_tx).await;
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
}

async fn handle_viewer_command(
    msg: ViewerMessage,
    state: &Arc<ServerState>,
    direct_tx: &tokio::sync::mpsc::UnboundedSender<DaemonMessage>,
) {
    match msg {
        ViewerMessage::Connect => {
            info!("Received explicit Connect handshake from viewer");
            for snapshot_msg in collect_snapshot_messages(state) {
                let _ = direct_tx.send(snapshot_msg);
            }
        }
        ViewerMessage::StartCapture(frame) => {
            let frame_id = frame.id;
            info!(
                "Forwarding StartCapture for frame {} ({:?}, {:.1}s) to controller",
                frame.id,
                frame.frame_type,
                frame.params.exposure.as_secs_f32()
            );
            if let Err(err) = state.controller.start_capture(frame).await {
                error!("Failed to forward StartCapture to controller: {err}");
                let _ = direct_tx.send(DaemonMessage::CaptureError {
                    frame_id,
                    message: err.to_string(),
                });
            }
        }
        ViewerMessage::StartSequence(sequence) => {
            let sequence_id = sequence.id;
            info!(
                "Forwarding StartSequence {} ({} frames) to controller",
                sequence_id,
                sequence.total_frames()
            );
            if let Err(err) = state.controller.start_sequence(sequence).await {
                error!("Failed to forward StartSequence to controller: {err}");
                let _ = direct_tx.send(DaemonMessage::CaptureError {
                    frame_id: sequence_id,
                    message: err.to_string(),
                });
            }
        }
        ViewerMessage::StartNightPlan(req) => {
            let run_id = req.run_id;
            info!(
                "Forwarding StartNightPlan {} ({} targets) to controller",
                run_id,
                req.plan.targets.len()
            );
            if let Err(err) = state.controller.start_night_plan(req).await {
                error!("Failed to forward StartNightPlan to controller: {err}");
                let _ = direct_tx.send(DaemonMessage::CaptureError {
                    frame_id: run_id,
                    message: err.to_string(),
                });
            }
        }
        ViewerMessage::UpdateNightPlan(plan) => {
            info!("Forwarding UpdateNightPlan '{}' to controller", plan.id);
            if let Err(err) = state.controller.update_night_plan(plan).await {
                error!("Failed to forward UpdateNightPlan to controller: {err}");
            }
        }
        ViewerMessage::PauseSequence => {
            info!("Forwarding PauseSequence to controller");
            if let Err(err) = state.controller.pause_sequence().await {
                error!("Failed to forward PauseSequence to controller: {err}");
            }
        }
        ViewerMessage::ResumeSequence => {
            info!("Forwarding ResumeSequence to controller");
            if let Err(err) = state.controller.resume_sequence().await {
                error!("Failed to forward ResumeSequence to controller: {err}");
            }
        }
        ViewerMessage::Abort => {
            warn!("Forwarding Abort command to controller");
            if let Err(err) = state.controller.abort().await {
                error!("Failed to forward Abort to controller: {err}");
            }
        }
        ViewerMessage::Mount(mount_cmd) => {
            if let Err(err) = state.controller.send_mount_command(mount_cmd).await {
                error!("Failed to forward MountCommand to controller: {err}");
                let _ = direct_tx.send(DaemonMessage::MountError {
                    message: err.to_string(),
                });
            }
        }
    }
}

pub fn spawn_controller_event_forwarder(state: Arc<ServerState>) {
    let mut event_rx = state.controller.subscribe_events();
    tokio::spawn(async move {
        loop {
            let event = match event_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("Controller event forwarder lagged by {skipped} messages; continuing");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            match event {
                ControllerEvent::Status(status) => {
                    state.broadcast(DaemonMessage::Status(status));
                }
                ControllerEvent::FrameCaptured(mut captured) => {
                    let frame_id = captured.frame.id;
                    let mime = detect_raw_mime_type(&captured.file_ref.filename);
                    let save_to_server = captured.frame.save_config.server.is_some();
                    let save_to_viewer = captured.frame.save_config.viewer.is_some();

                    if let Some(data) = captured.data.take() {
                        let mut persist_ok = true;
                        if save_to_server {
                            match state.persist_frame_to_disk(&captured, &data).await {
                                Ok(path) => {
                                    info!("Saved frame {frame_id} to {}", path.display());
                                }
                                Err(err) => {
                                    error!("Failed to save frame {frame_id} to disk: {err}");
                                    persist_ok = false;
                                    state.broadcast(DaemonMessage::CaptureError {
                                        frame_id,
                                        message: format!(
                                            "Failed to save frame to server storage: {err}"
                                        ),
                                    });
                                }
                            }
                        }

                        if save_to_viewer {
                            let filename = if captured.file_ref.filename.is_empty() {
                                format!("frame_{frame_id}.raw")
                            } else {
                                captured.file_ref.filename.clone()
                            };
                            state
                                .store_frame_with_name(frame_id, filename, data, mime.into())
                                .await;
                            let raw_url = format!("/api/frames/{frame_id}/raw");
                            state.broadcast(DaemonMessage::FrameReady {
                                frame_id,
                                url: raw_url,
                            });
                        } else if persist_ok {
                            state.broadcast(DaemonMessage::FrameReady {
                                frame_id,
                                url: String::new(),
                            });
                        }
                    } else {
                        state.broadcast(DaemonMessage::FrameReady {
                            frame_id,
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
                ControllerEvent::SequenceStatus(status) => {
                    state.broadcast(DaemonMessage::SequenceProgress {
                        completed: status.completed,
                        total: status.total,
                    });
                    state.broadcast(DaemonMessage::SequenceStatus(status));
                }
            }
        }
    });
}

pub fn spawn_camera_status_watcher(state: Arc<ServerState>) {
    let mut status_rx = state.controller.camera_status();
    let status_state = state.clone();
    tokio::spawn(async move {
        while status_rx.changed().await.is_ok() {
            let new_status = status_rx.borrow().clone();
            info!("Camera status changed: {:?}", new_status);
            status_state.broadcast(DaemonMessage::CameraStatus(new_status));
        }
    });

    let mut caps_rx = state.controller.camera_capabilities();
    tokio::spawn(async move {
        while caps_rx.changed().await.is_ok() {
            let caps = caps_rx.borrow().clone();
            state.broadcast(DaemonMessage::CameraCapabilities(caps));
        }
    });
}

pub fn spawn_mount_watchers(state: Arc<ServerState>) {
    let mut status_rx = state.controller.mount_status();
    let status_state = state.clone();
    tokio::spawn(async move {
        while status_rx.changed().await.is_ok() {
            let new_status = status_rx.borrow().clone();
            info!("Mount status changed: {:?}", new_status);
            status_state.broadcast(DaemonMessage::MountStatus(new_status));
        }
    });

    let mut telemetry_rx = state.controller.mount_telemetry();
    let telem_state = state.clone();
    tokio::spawn(async move {
        while telemetry_rx.changed().await.is_ok() {
            if let Some(telemetry) = telemetry_rx.borrow().clone() {
                telem_state.broadcast(DaemonMessage::MountTelemetry(telemetry));
            }
        }
    });

    let mut event_rx = state.controller.subscribe_mount_events();
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(MountEvent::Ports(ports)) => {
                    state.broadcast(DaemonMessage::MountPorts(ports));
                }
                Ok(MountEvent::Error(message)) => {
                    state.broadcast(DaemonMessage::MountError { message });
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("Mount event watcher lagged by {skipped} messages; continuing");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

pub async fn start_server(addr: SocketAddr, controller: ControllerHandle) -> anyhow::Result<()> {
    let state = Arc::new(ServerState::new(controller));
    spawn_camera_status_watcher(state.clone());
    spawn_mount_watchers(state.clone());
    spawn_controller_event_forwarder(state.clone());

    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Daemon listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
