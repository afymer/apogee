use gpui::{App, IntoElement, RenderOnce, SharedString, Window, div, px};
use protocol::{CameraStatus, ConnectionState};
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
