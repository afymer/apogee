use gpui::{App, IntoElement, RenderOnce, SharedString, Window, div, px};
use protocol::{CameraStatus, ConnectionState, MountStatus};
use theme::ActiveTheme;
use ui::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LogicState {
    #[default]
    Muted,
    Valid,
    Pending,
    Invalid,
}

impl LogicState {
    pub fn dot_color(self, cx: &App) -> gpui::Hsla {
        match self {
            LogicState::Valid => cx.theme().status().success,
            LogicState::Pending => cx.theme().status().warning,
            LogicState::Invalid => cx.theme().status().error,
            LogicState::Muted => cx.theme().colors().text_muted,
        }
    }

    pub fn label_color(self) -> ui::Color {
        match self {
            LogicState::Valid => ui::Color::Default,
            LogicState::Pending => ui::Color::Warning,
            LogicState::Invalid => ui::Color::Error,
            LogicState::Muted => ui::Color::Muted,
        }
    }
}

impl From<&ConnectionState> for LogicState {
    fn from(state: &ConnectionState) -> Self {
        match state {
            ConnectionState::Connected => LogicState::Valid,
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                LogicState::Pending
            }
            ConnectionState::Disconnected => LogicState::Invalid,
        }
    }
}

impl From<ConnectionState> for LogicState {
    fn from(state: ConnectionState) -> Self {
        LogicState::from(&state)
    }
}

impl From<&CameraStatus> for LogicState {
    fn from(status: &CameraStatus) -> Self {
        match status {
            CameraStatus::Ready => LogicState::Valid,
            CameraStatus::Recovering { .. } => LogicState::Pending,
            CameraStatus::Fatal(_) | CameraStatus::Disconnected => LogicState::Invalid,
        }
    }
}

impl From<CameraStatus> for LogicState {
    fn from(status: CameraStatus) -> Self {
        LogicState::from(&status)
    }
}

impl From<&MountStatus> for LogicState {
    fn from(status: &MountStatus) -> Self {
        match status {
            MountStatus::Ready => LogicState::Valid,
            MountStatus::Connecting { .. } => LogicState::Pending,
            MountStatus::Error(_) | MountStatus::Disconnected => LogicState::Invalid,
        }
    }
}

impl From<MountStatus> for LogicState {
    fn from(status: MountStatus) -> Self {
        LogicState::from(&status)
    }
}

/// Canonical `(LogicState, label)` presentation for a daemon WebSocket connection state.
pub fn connection_status_presentation(state: &ConnectionState) -> (LogicState, SharedString) {
    let label = match state {
        ConnectionState::Connected => SharedString::from("Connected"),
        ConnectionState::Connecting => SharedString::from("Connecting..."),
        ConnectionState::Reconnecting { attempt, .. } => {
            SharedString::from(format!("Reconnecting ({attempt})"))
        }
        ConnectionState::Disconnected => SharedString::from("Disconnected"),
    };
    (LogicState::from(state), label)
}

/// Canonical `(LogicState, label)` presentation for a camera driver status.
pub fn camera_status_presentation(
    status: &CameraStatus,
    daemon_connected: bool,
) -> (LogicState, SharedString) {
    if !daemon_connected {
        return (LogicState::Muted, SharedString::from("Offline"));
    }
    let label = match status {
        CameraStatus::Ready => SharedString::from("Ready"),
        CameraStatus::Recovering { attempt, .. } => {
            SharedString::from(format!("Recovering ({attempt})"))
        }
        CameraStatus::Fatal(_) => SharedString::from("Fatal Error"),
        CameraStatus::Disconnected => SharedString::from("Disconnected"),
    };
    (LogicState::from(status), label)
}

/// Canonical `(LogicState, label)` presentation for a mount driver status.
pub fn mount_status_presentation(
    status: &MountStatus,
    daemon_connected: bool,
) -> (LogicState, SharedString) {
    if !daemon_connected {
        return (LogicState::Muted, SharedString::from("Offline"));
    }
    let label = match status {
        MountStatus::Ready => SharedString::from("Ready"),
        MountStatus::Connecting { port } => SharedString::from(format!("Connecting ({port})")),
        MountStatus::Error(_) => SharedString::from("Error"),
        MountStatus::Disconnected => SharedString::from("Disconnected"),
    };
    (LogicState::from(status), label)
}

#[derive(IntoElement)]
pub struct StatusIndicator {
    state: LogicState,
    label: SharedString,
    label_size: LabelSize,
}

impl StatusIndicator {
    pub fn new(state: impl Into<LogicState>, label: impl Into<SharedString>) -> Self {
        Self {
            state: state.into(),
            label: label.into(),
            label_size: LabelSize::Small,
        }
    }

    #[must_use]
    pub fn label_size(mut self, size: LabelSize) -> Self {
        self.label_size = size;
        self
    }
}

impl RenderOnce for StatusIndicator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(self.state.dot_color(cx)),
            )
            .child(
                Label::new(self.label)
                    .size(self.label_size)
                    .color(self.state.label_color()),
            )
    }
}

pub type StatusBadge = StatusIndicator;
