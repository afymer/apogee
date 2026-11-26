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

#[test]
fn test_needs_meridian_flip() {
    use crate::needs_meridian_flip;
    use protocol::{MountLimits, MountTelemetry, PierSide};

    let mut telemetry = MountTelemetry {
        az_deg: 195.0,
        pier_side: PierSide::West,
        is_parked: false,
        limits: MountLimits {
            meridian_limit_deg: 10.0,
            auto_meridian_flip: true,
            ..MountLimits::default()
        },
        ..MountTelemetry::default()
    };

    // Past 180 + 10 deg on West pier side -> flip needed
    assert!(needs_meridian_flip(&telemetry));

    // Already on East pier side -> no flip
    telemetry.pier_side = PierSide::East;
    assert!(!needs_meridian_flip(&telemetry));

    // Within meridian limit (185 < 190) -> no flip
    telemetry.pier_side = PierSide::West;
    telemetry.az_deg = 185.0;
    assert!(!needs_meridian_flip(&telemetry));

    // Auto meridian flip disabled -> no flip
    telemetry.az_deg = 200.0;
    telemetry.limits.auto_meridian_flip = false;
    assert!(!needs_meridian_flip(&telemetry));

    // Parked -> no flip
    telemetry.limits.auto_meridian_flip = true;
    telemetry.is_parked = true;
    assert!(!needs_meridian_flip(&telemetry));
}

#[tokio::test]
async fn test_sequence_runner_multi_step_completion() {
    use protocol::{ImagingSequence, SequencePhase, SequenceStep};

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    tokio::spawn(async move {
        let mut idx = 0u32;
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    idx += 1;
                    let _ = reply.send(Ok(CameraFileRef {
                        folder: "/store".into(),
                        filename: format!("SEQ_{idx:04}.CR2"),
                    }));
                }
                CameraCommand::Download { reply, .. } => {
                    let _ = reply.send(Ok(vec![1, 2, 3, 4]));
                }
                CameraCommand::DeleteFile { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut event_rx = controller_handle.subscribe_events();

    let sequence = ImagingSequence {
        id: 7,
        target_name: "M31".into(),
        steps: vec![
            SequenceStep {
                enabled: true,
                frame_type: FrameType::Light,
                count: 2,
                params: CaptureParams {
                    exposure: Duration::from_millis(10),
                    iso: Some(800),
                },
                delay_between_frames: Duration::ZERO,
            },
            SequenceStep {
                enabled: false,
                frame_type: FrameType::Flat,
                count: 5,
                params: CaptureParams {
                    exposure: Duration::from_millis(10),
                    iso: Some(800),
                },
                delay_between_frames: Duration::ZERO,
            },
            SequenceStep {
                enabled: true,
                frame_type: FrameType::Dark,
                count: 1,
                params: CaptureParams {
                    exposure: Duration::from_millis(10),
                    iso: Some(800),
                },
                delay_between_frames: Duration::ZERO,
            },
        ],
        save_config: SaveLocationConfig::default(),
        auto_meridian_flip: false,
    };

    controller_handle.start_sequence(sequence).await.unwrap();

    let mut captured_count = 0u32;
    let mut final_phase = None;
    let timeout = tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(event) = event_rx.recv().await {
            match event {
                ControllerEvent::FrameCaptured(_) => {
                    captured_count += 1;
                }
                ControllerEvent::SequenceStatus(st) if st.phase == SequencePhase::Completed => {
                    assert_eq!(st.completed, 3);
                    assert_eq!(st.total, 3);
                    assert_eq!(st.current_step, 2);
                    final_phase = Some(st.phase);
                    break;
                }
                _ => {}
            }
        }
    })
    .await;

    assert!(timeout.is_ok(), "Sequence did not complete within timeout");
    assert_eq!(captured_count, 3);
    assert_eq!(final_phase, Some(SequencePhase::Completed));

    let mut status_rx = controller_handle.status();
    status_rx
        .wait_for(|s| *s == DaemonStatus::Idle)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_sequence_pause_resume_and_abort() {
    use protocol::{ImagingSequence, SequencePhase, SequenceStep};

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<CameraCommand>(32);
    let (_status_tx, status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cmd_tx, status_rx);

    tokio::spawn(async move {
        let mut idx = 0u32;
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    idx += 1;
                    let _ = reply.send(Ok(CameraFileRef {
                        folder: "/store".into(),
                        filename: format!("PAUSE_{idx:04}.CR2"),
                    }));
                }
                CameraCommand::Download { reply, .. } => {
                    let _ = reply.send(Ok(vec![9, 9]));
                }
                CameraCommand::DeleteFile { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let controller_handle = Controller::spawn(camera_handle);
    let mut seq_rx = controller_handle.sequence_status();

    let sequence = ImagingSequence {
        id: 8,
        target_name: "M42".into(),
        steps: vec![SequenceStep {
            enabled: true,
            frame_type: FrameType::Light,
            count: 5,
            params: CaptureParams {
                exposure: Duration::from_millis(30),
                iso: None,
            },
            delay_between_frames: Duration::ZERO,
        }],
        save_config: SaveLocationConfig::default(),
        auto_meridian_flip: false,
    };

    controller_handle.start_sequence(sequence).await.unwrap();
    // Pause shortly after starting; it should pause before frame 2.
    controller_handle.pause_sequence().await.unwrap();

    tokio::time::timeout(Duration::from_secs(3), async {
        seq_rx
            .wait_for(|s| s.as_ref().is_some_and(|st| st.phase == SequencePhase::Paused))
            .await
            .unwrap();
    })
    .await
    .expect("Sequence should reach Paused phase");

    // Resume and then abort.
    controller_handle.resume_sequence().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        seq_rx
            .wait_for(|s| s.as_ref().is_some_and(|st| st.phase == SequencePhase::Running))
            .await
            .unwrap();
    })
    .await
    .expect("Sequence should resume to Running phase");

    controller_handle.abort().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        seq_rx
            .wait_for(|s| s.as_ref().is_some_and(|st| st.phase == SequencePhase::Aborted))
            .await
            .unwrap();
    })
    .await
    .expect("Sequence should reach Aborted phase");
}

#[tokio::test]
async fn test_sequence_auto_meridian_flip_coordination() {
    use protocol::{
        ImagingSequence, MountCommand, MountLimits, MountStatus, MountTelemetry, PierSide,
        SequencePhase, SequenceStep,
    };
    use synscan::{MountEvent, MountHandle};
    use tokio::sync::broadcast;

    let (cam_tx, mut cam_rx) = mpsc::channel::<CameraCommand>(32);
    let (_cam_status_tx, cam_status_rx) = watch::channel(protocol::CameraStatus::Ready);
    let camera_handle = CameraHandle::new(cam_tx, cam_status_rx);

    tokio::spawn(async move {
        while let Some(cmd) = cam_rx.recv().await {
            match cmd {
                CameraCommand::Capture { reply, .. } => {
                    let _ = reply.send(Ok(CameraFileRef {
                        folder: "/store".into(),
                        filename: "FLIP_0001.CR2".into(),
                    }));
                }
                CameraCommand::Download { reply, .. } => {
                    let _ = reply.send(Ok(vec![42]));
                }
                CameraCommand::DeleteFile { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });

    let (mount_cmd_tx, mut mount_cmd_rx) = mpsc::channel::<MountCommand>(16);
    let (_mount_status_tx, mount_status_rx) = watch::channel(MountStatus::Ready);
    let initial_telem = MountTelemetry {
        az_deg: 195.0,
        pier_side: PierSide::West,
        is_parked: false,
        is_slewing: false,
        limits: MountLimits {
            meridian_limit_deg: 10.0,
            auto_meridian_flip: true,
            ..MountLimits::default()
        },
        ..MountTelemetry::default()
    };
    let (mount_telem_tx, mount_telem_rx) = watch::channel(Some(initial_telem.clone()));
    let (mount_event_tx, _) = broadcast::channel::<MountEvent>(16);
    let mount_handle = MountHandle::new(mount_cmd_tx, mount_status_rx, mount_telem_rx, mount_event_tx);

    let flip_received = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip_received_clone = flip_received.clone();

    tokio::spawn(async move {
        while let Some(cmd) = mount_cmd_rx.recv().await {
            if matches!(cmd, MountCommand::MeridianFlip) {
                flip_received_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                let mut slewing = initial_telem.clone();
                slewing.is_slewing = true;
                let _ = mount_telem_tx.send(Some(slewing));
                tokio::time::sleep(Duration::from_millis(20)).await;
                let mut settled = initial_telem.clone();
                settled.is_slewing = false;
                settled.pier_side = PierSide::East;
                let _ = mount_telem_tx.send(Some(settled));
            }
        }
    });

    let controller_handle = Controller::spawn_with_mount(camera_handle, mount_handle);
    let mut seq_rx = controller_handle.sequence_status();

    let sequence = ImagingSequence {
        id: 9,
        target_name: "NGC7000".into(),
        steps: vec![SequenceStep {
            enabled: true,
            frame_type: FrameType::Light,
            count: 1,
            params: CaptureParams {
                exposure: Duration::from_millis(10),
                iso: None,
            },
            delay_between_frames: Duration::ZERO,
        }],
        save_config: SaveLocationConfig::default(),
        auto_meridian_flip: true,
    };

    controller_handle.start_sequence(sequence).await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        seq_rx
            .wait_for(|s| s.as_ref().is_some_and(|st| st.phase == SequencePhase::Completed))
            .await
            .unwrap();
    })
    .await
    .expect("Sequence with meridian flip should complete");

    assert!(
        flip_received.load(std::sync::atomic::Ordering::SeqCst),
        "Expected MountCommand::MeridianFlip to be sent before Light exposure"
    );
}
