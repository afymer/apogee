use std::sync::Arc;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;

use controller::Controller;
use gphoto::handle::CameraHandle;
use gphoto::types::{CameraCommand, CameraFileRef};
use protocol::{CameraStatus, DaemonMessage, DaemonStatus, Frame, FrameType, CaptureParams, SaveLocationConfig};

use crate::server::{create_router, spawn_controller_event_forwarder, ServerState};

fn setup_test_controller() -> (controller::ControllerHandle, mpsc::Receiver<CameraCommand>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let (_status_tx, status_rx) = watch::channel(CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);
    let controller_handle = Controller::spawn(camera_handle);
    (controller_handle, cmd_rx)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (controller, _cmd_rx) = setup_test_controller();
    let state = Arc::new(ServerState::new(controller));
    let app = create_router(state);

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"OK");
}

#[tokio::test]
async fn test_status_endpoint() {
    let (controller, _cmd_rx) = setup_test_controller();
    let state = Arc::new(ServerState::new(controller));
    let app = create_router(state);

    let response = app
        .oneshot(Request::builder().uri("/api/status").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["daemon"], "Idle");
    assert_eq!(json["camera"], "Ready");
    assert_eq!(json["viewers"], 0);
}

#[tokio::test]
async fn test_frame_cache_endpoint() {
    let (controller, _cmd_rx) = setup_test_controller();
    let state = Arc::new(ServerState::new(controller));
    state.store_frame(42, vec![1, 2, 3, 4], "image/x-canon-cr2".into()).await;

    let app = create_router(state);
    let response = app
        .oneshot(Request::builder().uri("/api/frames/42/raw").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], &[1, 2, 3, 4]);
}

#[tokio::test]
async fn test_controller_event_forwarding() {
    let (controller, mut cmd_rx) = setup_test_controller();
    let state = Arc::new(ServerState::new(controller.clone()));
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    spawn_controller_event_forwarder(state.clone());

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    let _ = reply.send(Ok(CameraFileRef {
                        filename: "test.cr2".into(),
                        folder: "/store".into(),
                    }));
                }
                CameraCommand::Download { reply, .. } => {
                    let _ = reply.send(Ok(vec![10, 20, 30]));
                }
                CameraCommand::DeleteFile { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let frame = Frame {
        id: 77,
        target_name: "test".into(),
        frame_type: FrameType::Light,
        params: CaptureParams {
            exposure: std::time::Duration::from_secs(1),
            iso: None,
        },
        save_config: SaveLocationConfig::default(),
    };

    controller.start_capture(frame).await.unwrap();

    let mut saw_capturing = false;
    let mut saw_frame_ready = false;
    let mut saw_idle = false;

    let receive_loop = async {
        while let Ok(msg) = broadcast_rx.recv().await {
            match msg {
                DaemonMessage::Status(DaemonStatus::Capturing) => saw_capturing = true,
                DaemonMessage::FrameReady { frame_id, url } => {
                    assert_eq!(frame_id, 77);
                    assert_eq!(url, "/api/frames/77/raw");
                    saw_frame_ready = true;
                }
                DaemonMessage::Status(DaemonStatus::Idle)
                    if saw_capturing && saw_frame_ready =>
                {
                    saw_idle = true;
                    break;
                }
                _ => {}
            }
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(3), receive_loop)
        .await
        .expect("Events should be received within 3 seconds");

    assert!(saw_capturing);
    assert!(saw_frame_ready);
    assert!(saw_idle);
}
