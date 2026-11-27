use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use client::{DaemonCommandSender, GlobalDaemonState};
use components::TextField;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    Window, prelude::*,
};
use protocol::{
    CameraStatus, CaptureParams, ConnectionState, DaemonStatus, Frame, FrameType,
    SaveLocationConfig, ViewerMessage,
};
use ui::{Button, ButtonStyle, Color, Divider, Label, LabelSize, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::{bridge::RawViewerBridge, raw_image::RawImage};

const MAX_RECENT_FRAMES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraBenchMode {
    #[default]
    Capture,
}

struct CaptureUiState {
    is_capturing: bool,
    can_capture: bool,
    can_toggle_loop: bool,
    seq_active: bool,
    capture_label: &'static str,
    last_error: Option<(u64, String)>,
}

impl CameraBenchMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Capture]
    }
}

#[derive(Clone, Debug)]
struct RecentFrameEntry {
    frame_id: u64,
    label: String,
    data: Arc<Vec<u8>>,
}

#[derive(Default)]
struct RecentFrameBuffer {
    entries: VecDeque<RecentFrameEntry>,
}

impl RecentFrameBuffer {
    fn push_front(&mut self, entry: RecentFrameEntry) -> bool {
        if self.entries.iter().any(|e| e.frame_id == entry.frame_id) {
            return false;
        }
        self.entries.push_front(entry);
        while self.entries.len() > MAX_RECENT_FRAMES {
            self.entries.pop_back();
        }
        true
    }

    fn iter(&self) -> impl Iterator<Item = &RecentFrameEntry> {
        self.entries.iter()
    }

    fn get_by_frame_id(&self, frame_id: u64) -> Option<&RecentFrameEntry> {
        self.entries.iter().find(|e| e.frame_id == frame_id)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Parses exposure seconds `(0.0, 3600.0]` and optional positive ISO from text inputs.
pub fn parse_exposure_and_iso_str(
    exp_str: &str,
    iso_str: &str,
) -> Result<(Duration, Option<u32>), String> {
    let exp_trimmed = exp_str.trim();
    let exp_secs: f64 = exp_trimmed
        .parse()
        .map_err(|_| format!("Invalid exposure '{exp_trimmed}'"))?;
    if !exp_secs.is_finite() || exp_secs <= 0.0 || exp_secs > 3600.0 {
        return Err("Exposure must be between 0 and 3600 seconds".to_string());
    }

    let iso_trimmed = iso_str.trim();
    let iso = if iso_trimmed.is_empty() {
        None
    } else {
        let parsed: u32 = iso_trimmed
            .parse()
            .map_err(|_| format!("Invalid ISO '{iso_trimmed}'"))?;
        if parsed == 0 {
            return Err("ISO must be positive".to_string());
        }
        Some(parsed)
    };

    Ok((Duration::from_secs_f64(exp_secs), iso))
}

pub struct CameraControlPanel {
    bridge: RawViewerBridge,
    focus_handle: FocusHandle,
    mode: CameraBenchMode,
    decode_generation: Arc<AtomicU64>,
    target_input: Entity<TextField>,
    exposure_input: Entity<TextField>,
    iso_input: Entity<TextField>,
    selected_frame_type: FrameType,
    loop_enabled: bool,
    loop_awaiting_completion: bool,
    last_seen_error_frame: Option<u64>,
    pending_manual_meta: Option<(u64, String)>,
    recent_frames: RecentFrameBuffer,
    selected_frame_id: Option<u64>,
    input_error: Option<String>,
}

impl EventEmitter<PanelEvent> for CameraControlPanel {}

impl CameraControlPanel {
    pub fn build(bridge: RawViewerBridge, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let decode_generation = Arc::new(AtomicU64::new(0));
            let global_state = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone());
            if let Some(state_entity) = global_state {
                cx.observe(&state_entity, |this: &mut Self, model, cx| {
                    this.handle_daemon_state_update(&model, cx);
                })
                .detach();
            }

            let target_input = cx.new(|cx| TextField::new("test", window, cx).placeholder("Label"));
            let exposure_input = cx.new(|cx| {
                TextField::new("1.0", window, cx)
                    .placeholder("1.0")
                    .suffix("s")
                    .monospace(true)
            });
            let iso_input = cx.new(|cx| {
                TextField::new("", window, cx)
                    .placeholder("Auto")
                    .monospace(true)
            });

            Self {
                bridge,
                focus_handle: cx.focus_handle(),
                mode: CameraBenchMode::Capture,
                decode_generation,
                target_input,
                exposure_input,
                iso_input,
                selected_frame_type: FrameType::Light,
                loop_enabled: false,
                loop_awaiting_completion: false,
                last_seen_error_frame: None,
                pending_manual_meta: None,
                recent_frames: RecentFrameBuffer::default(),
                selected_frame_id: None,
                input_error: None,
            }
        })
    }

    fn handle_daemon_state_update(
        &mut self,
        model: &Entity<client::DaemonStateModel>,
        cx: &mut Context<Self>,
    ) {
        let (latest_frame, connection, camera, daemon, seq_active, last_error_frame) = {
            let model_ref = model.read(cx);
            (
                model_ref.latest_frame.clone(),
                model_ref.state.connection.clone(),
                model_ref.state.camera.clone(),
                model_ref.state.daemon.clone(),
                model_ref
                    .state
                    .sequence
                    .as_ref()
                    .is_some_and(|s| s.phase.is_active()),
                model_ref.last_error.as_ref().map(|(id, _)| *id),
            )
        };

        let mut frame_arrived = false;
        if let Some(frame) = latest_frame {
            let label = match &self.pending_manual_meta {
                Some((id, meta)) if *id == frame.frame_id => meta.clone(),
                _ => format!("#{}", frame.frame_id),
            };
            let entry = RecentFrameEntry {
                frame_id: frame.frame_id,
                label,
                data: frame.data.clone(),
            };
            if self.recent_frames.push_front(entry) {
                self.selected_frame_id = Some(frame.frame_id);
                self.spawn_decode(frame.data, cx);
                frame_arrived = true;
            }
        }

        if let Some(err_frame_id) = last_error_frame {
            if self.last_seen_error_frame != Some(err_frame_id) {
                self.last_seen_error_frame = Some(err_frame_id);
                if self.loop_enabled {
                    self.loop_enabled = false;
                    self.loop_awaiting_completion = false;
                }
            }
        }

        if seq_active {
            self.loop_enabled = false;
            self.loop_awaiting_completion = false;
        } else if self.loop_enabled {
            let hardware_ready =
                connection == ConnectionState::Connected && camera == CameraStatus::Ready;
            if !hardware_ready {
                self.loop_enabled = false;
                self.loop_awaiting_completion = false;
            } else if daemon == DaemonStatus::Idle
                && (frame_arrived || self.loop_awaiting_completion)
            {
                self.loop_awaiting_completion = false;
                let entity = cx.entity();
                cx.defer(move |cx| {
                    entity.update(cx, |this, cx| {
                        if this.loop_enabled {
                            this.trigger_capture(cx);
                        }
                    });
                });
            }
        }

        cx.notify();
    }

    fn spawn_decode(&self, data: Arc<Vec<u8>>, cx: &Context<Self>) {
        let generation = self.decode_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let decode_gen = Arc::clone(&self.decode_generation);
        let bridge = self.bridge.clone();
        cx.background_executor()
            .spawn(async move {
                match RawImage::decode_from_memory(&data) {
                    Ok(image) => {
                        if decode_gen.load(Ordering::Acquire) == generation {
                            bridge.set_image(image);
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to decode raw image: {err}");
                    }
                }
            })
            .detach();
    }

    pub fn trigger_capture(&mut self, cx: &mut Context<Self>) {
        let exp_str = self.exposure_input.read(cx).text().to_string();
        let iso_str = self.iso_input.read(cx).text().to_string();
        let (exposure, iso) = match parse_exposure_and_iso_str(&exp_str, &iso_str) {
            Ok(v) => v,
            Err(err) => {
                self.input_error = Some(err);
                self.loop_enabled = false;
                self.loop_awaiting_completion = false;
                cx.notify();
                return;
            }
        };
        self.input_error = None;

        let target = self.target_input.read(cx).text().trim().to_string();
        let target_name = if target.is_empty() {
            "test".to_string()
        } else {
            target
        };

        if let Some(sender) = cx.try_global::<DaemonCommandSender>() {
            let frame_id = client::next_frame_id();
            let meta_label = format!(
                "{} {:.1}s {}",
                target_name,
                exposure.as_secs_f64(),
                self.selected_frame_type.label()
            );
            self.pending_manual_meta = Some((frame_id, meta_label));
            if self.loop_enabled {
                self.loop_awaiting_completion = true;
            }

            let frame = Frame {
                id: frame_id,
                target_name,
                frame_type: self.selected_frame_type,
                params: CaptureParams { exposure, iso },
                save_config: SaveLocationConfig::default(),
            };

            if sender
                .0
                .try_send(ViewerMessage::StartCapture(frame))
                .is_err()
            {
                tracing::error!("Failed to send StartCapture command to daemon");
                self.loop_enabled = false;
                self.loop_awaiting_completion = false;
            }
        }
        cx.notify();
    }

    fn toggle_loop(&mut self, can_capture: bool, cx: &mut Context<Self>) {
        if self.loop_enabled {
            self.loop_enabled = false;
            self.loop_awaiting_completion = false;
            cx.notify();
            return;
        }
        if !can_capture {
            return;
        }
        self.loop_enabled = true;
        self.trigger_capture(cx);
    }

    fn abort_capture(&mut self, cx: &mut Context<Self>) {
        self.loop_enabled = false;
        self.loop_awaiting_completion = false;
        if let Some(sender) = cx.try_global::<DaemonCommandSender>() {
            if sender.0.try_send(ViewerMessage::Abort).is_err() {
                tracing::error!("Failed to send Abort command to daemon");
            }
        }
        cx.notify();
    }

    fn render_mode_header(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_1()
            .children(CameraBenchMode::all().iter().enumerate().map(|(idx, &m)| {
                let active = self.mode == m;
                Button::new(("camera_mode_btn", idx), m.label())
                    .style(if active {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mode = m;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn render_capture_section(
        &self,
        ui_state: &CaptureUiState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_capture = ui_state.can_capture;
        let can_toggle_loop = ui_state.can_toggle_loop;
        v_flex()
            .gap_2()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Label")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(self.target_input.clone()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .children(FrameType::all().iter().enumerate().map(|(idx, &ft)| {
                        let selected = self.selected_frame_type == ft;
                        Button::new(("camera_ft_btn", idx), ft.label())
                            .style(if selected {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.selected_frame_type = ft;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                Label::new("Exp")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(self.exposure_input.clone()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                Label::new("ISO")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(self.iso_input.clone()),
                    ),
            )
            .children(
                self.input_error
                    .clone()
                    .map(|err| Label::new(err).size(LabelSize::XSmall).color(Color::Error)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("capture_frame_btn", ui_state.capture_label)
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
                        Button::new(
                            "loop_capture_btn",
                            if self.loop_enabled {
                                "Loop: On"
                            } else {
                                "Loop"
                            },
                        )
                        .style(if self.loop_enabled {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .disabled(!self.loop_enabled && !can_toggle_loop)
                        .on_click(cx.listener(
                            move |this, _event, _window, cx| {
                                this.toggle_loop(can_capture, cx);
                            },
                        )),
                    )
                    .when(
                        (ui_state.is_capturing && !ui_state.seq_active) || self.loop_enabled,
                        |el| {
                            el.child(
                                Button::new("abort_capture_btn", "Abort")
                                    .style(ButtonStyle::Subtle)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.abort_capture(cx);
                                    })),
                            )
                        },
                    ),
            )
            .when(ui_state.seq_active, |el| {
                el.child(
                    Label::new("Session running — pause or abort in Plan to take manual shots")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .children(ui_state.last_error.clone().map(|(frame_id, message)| {
                Label::new(format!("Frame #{frame_id}: {message}"))
                    .size(LabelSize::Small)
                    .color(Color::Error)
            }))
            .into_any_element()
    }

    fn render_viewer_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let demosaic_active = self.bridge.demosaic();
        let demosaic_label = if demosaic_active {
            "Demosaic: Bilinear (RGB)"
        } else {
            "Demosaic: Raw Bayer"
        };

        v_flex()
            .gap_2()
            .child(
                Label::new("Viewer")
                    .size(LabelSize::Default)
                    .color(Color::Default),
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
                    )
                    .child(
                        Button::new("reset_stretch_cp_btn", "Reset Stretch")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.bridge.reset_display_range();
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_recent_frames_section(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .child(
                Label::new("Recent Frames")
                    .size(LabelSize::Default)
                    .color(Color::Default),
            )
            .when(self.recent_frames.entries.is_empty(), |el| {
                el.child(
                    Label::new("No frames captured yet")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .children(self.recent_frames.iter().enumerate().map(|(idx, entry)| {
                let frame_id = entry.frame_id;
                let selected = self.selected_frame_id == Some(frame_id);
                Button::new(("recent_frame_btn", idx), entry.label.clone())
                    .style(if selected {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if let Some(found) = this.recent_frames.get_by_frame_id(frame_id) {
                            this.selected_frame_id = Some(frame_id);
                            this.spawn_decode(found.data.clone(), cx);
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element()
    }
}

impl Render for CameraControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let daemon_model = cx.try_global::<GlobalDaemonState>().map(|g| g.0.read(cx));
        let ui_state = if let Some(model) = daemon_model {
            let seq_active = model
                .state
                .sequence
                .as_ref()
                .is_some_and(|s| s.phase.is_active());
            let capturing = model.state.daemon == DaemonStatus::Capturing;
            let hw_ready = model.state.connection == ConnectionState::Connected
                && model.state.camera == CameraStatus::Ready;
            let ready = hw_ready && !capturing && !seq_active;
            let label = if seq_active {
                "Session Active"
            } else if capturing {
                "Capturing..."
            } else if model.state.connection != ConnectionState::Connected {
                "Daemon Disconnected"
            } else if model.state.camera != CameraStatus::Ready {
                "Camera Offline"
            } else {
                "Capture Frame"
            };
            CaptureUiState {
                is_capturing: capturing,
                can_capture: ready,
                can_toggle_loop: hw_ready && !seq_active,
                seq_active,
                capture_label: label,
                last_error: model.last_error.clone(),
            }
        } else {
            CaptureUiState {
                is_capturing: false,
                can_capture: false,
                can_toggle_loop: false,
                seq_active: false,
                capture_label: "Capture Frame",
                last_error: None,
            }
        };

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().colors().panel_background)
            .child(self.render_mode_header(cx))
            .child(Divider::horizontal())
            .child(self.render_capture_section(&ui_state, cx))
            .child(Divider::horizontal())
            .child(self.render_viewer_section(cx))
            .child(Divider::horizontal())
            .child(self.render_recent_frames_section(cx))
    }
}

impl Focusable for CameraControlPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CameraControlPanel {
    fn persistent_name() -> &'static str {
        "camera_control_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Camera".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Camera)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exposure_and_iso_validates_inputs() {
        let (exp, iso) = parse_exposure_and_iso_str("2.5", "").unwrap();
        assert_eq!(exp, Duration::from_secs_f64(2.5));
        assert_eq!(iso, None);

        let (exp, iso) = parse_exposure_and_iso_str("120", "800").unwrap();
        assert_eq!(exp, Duration::from_secs(120));
        assert_eq!(iso, Some(800));

        assert!(parse_exposure_and_iso_str("0", "").is_err());
        assert!(parse_exposure_and_iso_str("-1", "").is_err());
        assert!(parse_exposure_and_iso_str("3601", "").is_err());
        assert!(parse_exposure_and_iso_str("NaN", "").is_err());
        assert!(parse_exposure_and_iso_str("1", "0").is_err());
        assert!(parse_exposure_and_iso_str("1", "abc").is_err());
    }

    #[test]
    fn recent_frame_buffer_deduplicates_and_caps_at_max() {
        let mut buf = RecentFrameBuffer::default();
        for id in 1..=10 {
            let added = buf.push_front(RecentFrameEntry {
                frame_id: id,
                label: format!("frame {id}"),
                data: Arc::new(vec![id as u8]),
            });
            assert!(added);
        }
        assert_eq!(buf.len(), MAX_RECENT_FRAMES);
        assert!(buf.get_by_frame_id(1).is_none());
        assert!(buf.get_by_frame_id(2).is_none());
        assert!(buf.get_by_frame_id(10).is_some());

        // Duplicate frame_id is ignored
        assert!(!buf.push_front(RecentFrameEntry {
            frame_id: 10,
            label: "dup".to_string(),
            data: Arc::new(vec![99]),
        }));
        assert_eq!(buf.len(), MAX_RECENT_FRAMES);
    }
}
