use crate::{Controller, ControllerEvent, should_delete_from_camera, should_download};
use gphoto::handle::CameraHandle;
use gphoto::types::{CameraCommand, CameraFileRef};
use protocol::{
    CameraSaveConfig, CaptureParams, DaemonStatus, Frame, FrameType, SaveLocationConfig,
    ServerSaveConfig, ViewerSaveConfig,
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[test]
fn test_should_delete_from_camera() {
    // 1. None camera config -> delete after download
    let default_config = SaveLocationConfig::default();
    assert!(should_delete_from_camera(&default_config));

    // 2. Explicitly configured delete_after_download: true
    let delete_config = SaveLocationConfig {
        common_template: "tmpl".into(),
        camera: Some(CameraSaveConfig {
            delete_after_download: true,
            override_template: None,
        }),
        server: None,
        viewer: None,
    };
    assert!(should_delete_from_camera(&delete_config));

    // 3. Explicitly configured delete_after_download: false -> preserve on camera
    let preserve_config = SaveLocationConfig {
        common_template: "tmpl".into(),
        camera: Some(CameraSaveConfig {
            delete_after_download: false,
            override_template: None,
        }),
        server: None,
        viewer: None,
    };
    assert!(!should_delete_from_camera(&preserve_config));
}

#[test]
fn test_should_download() {
    // 1. Default config has server and viewer -> true
    let default_config = SaveLocationConfig::default();
    assert!(should_download(&default_config));

    // 2. Camera-only config -> false
    let camera_only = SaveLocationConfig {
        common_template: "tmpl".into(),
        camera: Some(CameraSaveConfig {
            delete_after_download: false,
            override_template: None,
        }),
        server: None,
        viewer: None,
    };
    assert!(!should_download(&camera_only));

    // 3. Server-only config -> true
    let server_only = SaveLocationConfig {
        common_template: "tmpl".into(),
        camera: None,
        server: Some(ServerSaveConfig {
            override_template: None,
        }),
        viewer: None,
    };
    assert!(should_download(&server_only));

    // 4. Viewer-only config -> true
    let viewer_only = SaveLocationConfig {
        common_template: "tmpl".into(),
        camera: None,
        server: None,
        viewer: Some(ViewerSaveConfig {
            override_template: None,
        }),
    };
    assert!(should_download(&viewer_only));
}

#[tokio::test]
async fn test_capture_concurrency_gating() {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    // Keep camera command receiver open in background
    tokio::spawn(async move {
        while let Some(_cmd) = cmd_rx.recv().await {
            // Keep command channel alive without replying immediately to simulate ongoing capture
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut event_rx = controller_handle.subscribe_events();

    let make_frame = |id: u64| Frame {
        id,
        target_name: "test".into(),
        frame_type: FrameType::Light,
        params: CaptureParams {
            exposure: Duration::from_secs(10),
            iso: None,
        },
        save_config: SaveLocationConfig::default(),
    };

    // First capture starts
    controller_handle
        .start_capture(make_frame(1))
        .await
        .unwrap();

    // Wait for status to become Capturing
    let mut status_rx = controller_handle.status();
    status_rx
        .wait_for(|s| *s == DaemonStatus::Capturing)
        .await
        .unwrap();

    // Second capture while first is in progress
    controller_handle
        .start_capture(make_frame(2))
        .await
        .unwrap();

    // Should receive CaptureError for frame 2
    let mut got_error = false;
    while let Ok(event) = event_rx.recv().await {
        if let ControllerEvent::CaptureError { frame_id, error } = event {
            if frame_id == 2 {
                assert!(error.contains("already in progress"));
                got_error = true;
                break;
            }
        }
    }
    assert!(got_error);

    // Abort should reset status to Idle
    controller_handle.abort().await.unwrap();
    status_rx
        .wait_for(|s| *s == DaemonStatus::Idle)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_capture_deletion_flow_when_configured() {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    let downloaded_files =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let downloaded_clone = downloaded_files.clone();
    let deleted_files = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let deleted_clone = deleted_files.clone();

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    let _ = reply.send(Ok(CameraFileRef {
                        filename: "IMG_0001.CR2".into(),
                        folder: "/store_0001/DCIM".into(),
                    }));
                }
                CameraCommand::Download {
                    folder,
                    filename,
                    reply,
                } => {
                    downloaded_clone.lock().unwrap().push((folder, filename));
                    let _ = reply.send(Ok(vec![1, 2, 3, 4]));
                }
                CameraCommand::DeleteFile {
                    folder,
                    filename,
                    reply,
                } => {
                    deleted_clone.lock().unwrap().push((folder, filename));
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut event_rx = controller_handle.subscribe_events();

    let frame = Frame {
        id: 100,
        target_name: "m42".into(),
        frame_type: FrameType::Light,
        params: CaptureParams {
            exposure: Duration::from_secs(1),
            iso: None,
        },
        save_config: SaveLocationConfig {
            common_template: "tmpl".into(),
            camera: Some(CameraSaveConfig {
                delete_after_download: true,
                override_template: None,
            }),
            server: Some(ServerSaveConfig {
                override_template: None,
            }),
            viewer: None,
        },
    };

    controller_handle.start_capture(frame).await.unwrap();

    while let Ok(event) = event_rx.recv().await {
        if let ControllerEvent::FrameCaptured(captured) = event {
            assert_eq!(captured.frame.id, 100);
            assert_eq!(captured.file_ref.filename, "IMG_0001.CR2");
            assert_eq!(captured.data, Some(vec![1, 2, 3, 4]));
            break;
        }
    }

    // Verify Download was called on the camera driver
    let downloaded = downloaded_files.lock().unwrap().clone();
    assert_eq!(
        downloaded,
        vec![("/store_0001/DCIM".to_string(), "IMG_0001.CR2".to_string())]
    );

    // Verify DeleteFile was called on the camera driver
    let deleted = deleted_files.lock().unwrap().clone();
    assert_eq!(
        deleted,
        vec![("/store_0001/DCIM".to_string(), "IMG_0001.CR2".to_string())]
    );
}

#[tokio::test]
async fn test_capture_preservation_flow_when_configured() {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    let downloaded_files =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let downloaded_clone = downloaded_files.clone();
    let deleted_files = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let deleted_clone = deleted_files.clone();

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    let _ = reply.send(Ok(CameraFileRef {
                        filename: "IMG_0002.CR2".into(),
                        folder: "/store_0001/DCIM".into(),
                    }));
                }
                CameraCommand::Download {
                    folder,
                    filename,
                    reply,
                } => {
                    downloaded_clone.lock().unwrap().push((folder, filename));
                    let _ = reply.send(Ok(vec![5, 6, 7, 8]));
                }
                CameraCommand::DeleteFile {
                    folder,
                    filename,
                    reply,
                } => {
                    deleted_clone.lock().unwrap().push((folder, filename));
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut event_rx = controller_handle.subscribe_events();

    let frame = Frame {
        id: 101,
        target_name: "m42".into(),
        frame_type: FrameType::Light,
        params: CaptureParams {
            exposure: Duration::from_secs(1),
            iso: None,
        },
        save_config: SaveLocationConfig {
            common_template: "tmpl".into(),
            camera: Some(CameraSaveConfig {
                delete_after_download: false, // DO NOT DELETE!
                override_template: None,
            }),
            server: Some(ServerSaveConfig {
                override_template: None,
            }),
            viewer: None,
        },
    };

    controller_handle.start_capture(frame).await.unwrap();

    while let Ok(event) = event_rx.recv().await {
        if let ControllerEvent::FrameCaptured(captured) = event {
            assert_eq!(captured.frame.id, 101);
            assert_eq!(captured.file_ref.filename, "IMG_0002.CR2");
            assert_eq!(captured.data, Some(vec![5, 6, 7, 8]));
            break;
        }
    }

    // Verify Download was called
    let downloaded = downloaded_files.lock().unwrap().clone();
    assert_eq!(
        downloaded,
        vec![("/store_0001/DCIM".to_string(), "IMG_0002.CR2".to_string())]
    );

    // Verify DeleteFile was NOT called on the camera driver
    let deleted = deleted_files.lock().unwrap().clone();
    assert!(
        deleted.is_empty(),
        "Expected no files to be deleted from camera, but got: {:?}",
        deleted
    );
}

#[tokio::test]
async fn test_capture_camera_only_skips_download() {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    let downloaded_files =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let downloaded_clone = downloaded_files.clone();

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    let _ = reply.send(Ok(CameraFileRef {
                        filename: "IMG_0003.CR2".into(),
                        folder: "/store_0001/DCIM".into(),
                    }));
                }
                CameraCommand::Download {
                    folder,
                    filename,
                    reply,
                } => {
                    downloaded_clone.lock().unwrap().push((folder, filename));
                    let _ = reply.send(Ok(vec![9, 9, 9]));
                }
                CameraCommand::DeleteFile { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut event_rx = controller_handle.subscribe_events();

    let frame = Frame {
        id: 102,
        target_name: "m42".into(),
        frame_type: FrameType::Light,
        params: CaptureParams {
            exposure: Duration::from_secs(1),
            iso: None,
        },
        save_config: SaveLocationConfig {
            common_template: "tmpl".into(),
            camera: Some(CameraSaveConfig {
                delete_after_download: false,
                override_template: None,
            }),
            server: None,
            viewer: None,
        },
    };

    controller_handle.start_capture(frame).await.unwrap();

    while let Ok(event) = event_rx.recv().await {
        if let ControllerEvent::FrameCaptured(captured) = event {
            assert_eq!(captured.frame.id, 102);
            assert_eq!(captured.file_ref.filename, "IMG_0003.CR2");
            assert_eq!(captured.data, None);
            break;
        }
    }

    // Verify Download was NOT called on the camera driver
    let downloaded = downloaded_files.lock().unwrap().clone();
    assert!(
        downloaded.is_empty(),
        "Expected no downloads for camera-only save config, but got: {:?}",
        downloaded
    );
}
