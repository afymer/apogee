use client::GlobalDaemonState;
use components::{LogicState, StatusIndicator};
use gpui::SharedString;
use protocol::{CameraStatus, ConnectionState};
use ui::{Divider, prelude::*};

pub struct StatusBar {}

impl StatusBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let global_entity = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone());
        if let Some(entity) = global_entity {
            cx.observe(&entity, |_this, _model, cx| {
                cx.notify();
            })
            .detach();
        }
        Self {}
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let daemon_state = cx
            .try_global::<GlobalDaemonState>()
            .map(|g| g.0.read(cx).state.clone())
            .unwrap_or_default();

        let (daemon_logic, daemon_text) = match daemon_state.connection {
            ConnectionState::Connected => {
                (LogicState::Valid, SharedString::from("Daemon: Connected"))
            }
            ConnectionState::Connecting => (
                LogicState::Pending,
                SharedString::from("Daemon: Connecting..."),
            ),
            ConnectionState::Reconnecting { attempt, .. } => (
                LogicState::Pending,
                SharedString::from(format!("Daemon: Reconnecting (#{attempt})")),
            ),
            ConnectionState::Disconnected => (
                LogicState::Invalid,
                SharedString::from("Daemon: Disconnected"),
            ),
        };

        let (camera_logic, camera_text) = match daemon_state.connection {
            ConnectionState::Connected => match daemon_state.camera {
                CameraStatus::Ready => (LogicState::Valid, SharedString::from("Camera: Ready")),
                CameraStatus::Recovering { attempt, reason } => (
                    LogicState::Pending,
                    SharedString::from(format!("Camera: Recovering (#{attempt} - {reason})")),
                ),
                CameraStatus::Fatal(reason) => (
                    LogicState::Invalid,
                    SharedString::from(format!("Camera: Error ({reason})")),
                ),
                CameraStatus::Disconnected => (
                    LogicState::Invalid,
                    SharedString::from("Camera: Disconnected"),
                ),
            },
            _ => (LogicState::Muted, SharedString::from("Camera: Offline")),
        };

        h_flex()
            .w_full()
            .h_7()
            .items_center()
            .justify_between()
            .px_3()
            .bg(cx.theme().colors().status_bar_background)
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(StatusIndicator::new(daemon_logic, daemon_text))
                    .child(Divider::vertical())
                    .child(StatusIndicator::new(camera_logic, camera_text)),
            )
            .child(
                h_flex().items_center().gap_1().child(
                    Button::new("status_button", "Apogee")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Subtle),
                ),
            )
    }
}
