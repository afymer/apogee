use client::{DaemonCommandSender, GlobalDaemonState};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    prelude::*,
};
use protocol::{
    CameraStatus, CaptureParams, ConnectionState, DaemonStatus, Frame, FrameType,
    SaveLocationConfig, ViewerMessage,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::{bridge::RawViewerBridge, raw_image::RawImage};

pub struct CaptureControlPanel {
    bridge: RawViewerBridge,
    focus_handle: FocusHandle,
    last_frame_id: Option<u64>,
}

impl EventEmitter<PanelEvent> for CaptureControlPanel {}

impl CaptureControlPanel {
    pub fn build(bridge: RawViewerBridge, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let global_state = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone());
            if let Some(state_entity) = global_state {
                cx.observe(&state_entity, |this: &mut Self, model, cx| {
                    let model_ref = model.read(cx);
                    if let Some(frame) = &model_ref.latest_frame {
                        if this.last_frame_id != Some(frame.frame_id) {
                            this.last_frame_id = Some(frame.frame_id);
                            let data = frame.data.clone();
                            let bridge = this.bridge.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    match RawImage::decode_from_memory(&data) {
                                        Ok(image) => {
                                            bridge.set_image(image);
                                        }
                                        Err(err) => {
                                            tracing::error!("Failed to decode raw image: {err}");
                                        }
                                    }
                                })
                                .detach();
                        }
                    }
                    cx.notify();
                })
                .detach();
            }

            Self {
                bridge,
                focus_handle: cx.focus_handle(),
                last_frame_id: None,
            }
        })
    }

    pub fn trigger_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(sender) = cx.try_global::<DaemonCommandSender>() {
            let frame_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let frame = Frame {
                id: frame_id,
                target_name: "capture".to_string(),
                frame_type: FrameType::Light,
                params: CaptureParams {
                    exposure: std::time::Duration::from_secs(1),
                    iso: None,
                },
                save_config: SaveLocationConfig::default(),
            };

            if sender
                .0
                .try_send(ViewerMessage::StartCapture(frame))
                .is_err()
            {
                tracing::error!("Failed to send StartCapture command to daemon");
            }
        }
        cx.notify();
    }
}

impl Render for CaptureControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demosaic_active = self.bridge.demosaic();
        let demosaic_label = if demosaic_active {
            "Demosaic: Bilinear (RGB)"
        } else {
            "Demosaic: Raw Bayer"
        };

        let daemon_model = cx.try_global::<GlobalDaemonState>().map(|g| g.0.read(cx));
        let (_is_capturing, can_capture, capture_label) = if let Some(model) = daemon_model {
            let capturing = model.state.daemon == DaemonStatus::Capturing;
            let ready = model.state.connection == ConnectionState::Connected
                && model.state.camera == CameraStatus::Ready
                && !capturing;
            let label = if capturing {
                "Capturing..."
            } else if model.state.connection != ConnectionState::Connected {
                "Daemon Disconnected"
            } else if model.state.camera != CameraStatus::Ready {
                "Camera Offline"
            } else {
                "Capture Frame"
            };
            (capturing, ready, label)
        } else {
            (false, false, "Capture Frame")
        };

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().colors().panel_background)
            .child(
                v_flex().gap_1().child(
                    Label::new("Capture Control")
                        .size(LabelSize::Default)
                        .color(Color::Default),
                ),
            )
            .child(
                Button::new("capture_frame_btn", capture_label)
                    .style(if can_capture {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .disabled(!can_capture)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.trigger_capture(cx);
                    })),
            )
            .child(
                Button::new("toggle_demosaic_btn", demosaic_label)
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.bridge.toggle_demosaic();
                        cx.notify();
                    })),
            )
            .child(
                v_flex().gap_1().child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("fit_view_cp_btn", "Fit")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.bridge.reset_view();
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("one_to_one_cp_btn", "1:1")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.bridge.set_one_to_one();
                                    cx.notify();
                                })),
                        ),
                ),
            )
    }
}

impl Focusable for CaptureControlPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CaptureControlPanel {
    fn persistent_name() -> &'static str {
        "capture_control_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Capture".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Capture)
    }
}
