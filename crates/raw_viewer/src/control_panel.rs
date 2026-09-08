use std::path::PathBuf;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    prelude::*,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::bridge::RawViewerBridge;

pub struct CaptureControlPanel {
    bridge: RawViewerBridge,
    focus_handle: FocusHandle,
}

impl EventEmitter<PanelEvent> for CaptureControlPanel {}

impl CaptureControlPanel {
    pub fn build(bridge: RawViewerBridge, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            bridge,
            focus_handle: cx.focus_handle(),
        })
    }

    /// Resolves the expected location of test.cr2 next to the binary or in the current directory.
    pub fn locate_test_cr2() -> PathBuf {
        // 1. Next to the current executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("test.cr2");
                if candidate.exists() {
                    return candidate;
                }
            }
        }
        // 2. In current working directory
        let cwd_candidate = PathBuf::from("test.cr2");
        if cwd_candidate.exists() {
            return cwd_candidate;
        }
        // 3. Fallback: standard location next to executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                return parent.join("test.cr2");
            }
        }
        PathBuf::from("test.cr2")
    }

    pub fn load_test_cr2(&mut self, cx: &mut Context<Self>) {
        let path = Self::locate_test_cr2();
        if !path.exists() {
            return;
        }

        if let Ok(bytes) = std::fs::read(&path) {
            let _ = self.bridge.load_from_memory(&bytes);
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
                Button::new("load_test_cr2_btn", "Load test.cr2")
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.load_test_cr2(cx);
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
                                    // TODO: make this a real 1:1 by bypassing the renderer scaling
                                    if let (Some((raw_w, _)), Some((view_w, _))) =
                                        (this.bridge.image_size(), this.bridge.last_size())
                                    {
                                        let scale = (raw_w as f32) / (view_w as f32).max(1.0);
                                        this.bridge.set_pan_zoom(glam::Vec2::ZERO, scale);
                                    } else {
                                        this.bridge.set_pan_zoom(glam::Vec2::ZERO, 1.0);
                                    }
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
