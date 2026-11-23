//! Connect workspace panel — central hardware connection & status hub.
//!
//! Manages the Raspberry Pi daemon connection, SynScan mount connection
//! (serial hand controller or built-in simulator), DSLR camera status, and modular
//! slots for upcoming hardware (focuser, guide camera).

use client::{DaemonAddress, GlobalDaemonState, send_mount_command};
use components::{
    LogicState, StatusIndicator, TextField, TextFieldEvent, camera_status_presentation,
    connection_status_presentation, mount_status_presentation,
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    IntoElement, Render, SharedString, Window, div, prelude::*, px,
};
use observer::GlobalObserver;
use protocol::{
    ConnectionState, DaemonStatus, MountCommand, MountDriverKind, MountStatus, MountTelemetry,
};
use status_bar::{format_altaz, format_radec};
use theme::ActiveTheme;
use ui::{Button, ButtonStyle, Color, Divider, Label, LabelSize, StyledTypography, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    workbench::Workbench,
};

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        let Some(window) = window else {
            return;
        };
        let panel = ConnectPanel::build(window, cx);
        cx.observe(&panel, |_this, _panel, cx| cx.notify()).detach();
        workbench.add_panel(panel, window, cx);
    })
    .detach();
}

pub struct ConnectPanel {
    focus_handle: FocusHandle,
    daemon_url_input: Entity<TextField>,
    mount_port_input: Entity<TextField>,
    mount_baud_input: Entity<TextField>,
    selected_driver: MountDriverKind,
    validation_error: Option<String>,
    pending_site_sync: bool,
}

impl EventEmitter<PanelEvent> for ConnectPanel {}

impl ConnectPanel {
    pub fn build(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let initial_url = cx
                .try_global::<DaemonAddress>()
                .map(|a| a.url().to_string())
                .unwrap_or_else(|| "ws://127.0.0.1:4000/ws".to_string());

            let (initial_driver, initial_port, initial_baud) = cx
                .try_global::<GlobalDaemonState>()
                .map(|g| {
                    let cfg = &g.0.read(cx).hardware_config;
                    let driver = if cfg.use_simulator {
                        MountDriverKind::Simulator
                    } else {
                        MountDriverKind::SynScanSerial
                    };
                    (
                        driver,
                        cfg.mount_port.clone(),
                        cfg.mount_baud_rate.to_string(),
                    )
                })
                .unwrap_or((
                    MountDriverKind::SynScanSerial,
                    "/dev/ttyUSB0".to_string(),
                    "9600".to_string(),
                ));

            let daemon_url_input = cx.new(|cx| {
                TextField::new(initial_url, window, cx)
                    .placeholder("ws://raspberrypi.local:4000/ws")
            });
            let mount_port_input =
                cx.new(|cx| TextField::new(initial_port, window, cx).placeholder("/dev/ttyUSB0"));
            let mount_baud_input =
                cx.new(|cx| TextField::new(initial_baud, window, cx).placeholder("9600"));

            cx.subscribe(
                &daemon_url_input,
                |this: &mut Self, _, event: &TextFieldEvent, cx| {
                    if matches!(event, TextFieldEvent::Submit(_)) {
                        this.apply_daemon_url(cx);
                    }
                },
            )
            .detach();

            for field in [&mount_port_input, &mount_baud_input] {
                cx.subscribe(field, |this: &mut Self, _, event: &TextFieldEvent, cx| {
                    match event {
                        TextFieldEvent::Change(_) => {
                            if this.validation_error.take().is_some() {
                                cx.notify();
                            }
                        }
                        TextFieldEvent::Submit(_) => {
                            this.connect_mount(cx);
                        }
                        _ => {}
                    }
                })
                .detach();
            }

            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |this: &mut Self, model, cx| {
                    if this.pending_site_sync {
                        let m = model.read(cx);
                        if m.state.connection == ConnectionState::Connected
                            && m.state.mount == MountStatus::Ready
                        {
                            this.pending_site_sync = false;
                            this.sync_observer_site(cx);
                        }
                    }
                    cx.notify();
                })
                .detach();
            }

            Self {
                focus_handle: cx.focus_handle(),
                daemon_url_input,
                mount_port_input,
                mount_baud_input,
                selected_driver: initial_driver,
                validation_error: None,
                pending_site_sync: false,
            }
        })
    }

    fn apply_daemon_url(&mut self, cx: &mut Context<Self>) {
        let raw = self.daemon_url_input.read(cx).text().trim().to_string();
        if raw.is_empty() {
            return;
        }
        let normalized = if raw.contains("://") {
            raw
        } else {
            format!("ws://{raw}")
        };
        if cx.has_global::<DaemonAddress>() {
            cx.global_mut::<DaemonAddress>().set_url(normalized.clone());
        }
        if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
            daemon.update(cx, |model, cx| {
                model.update_hardware_config(|cfg| cfg.daemon_url = Some(normalized), cx);
            });
        }
        cx.notify();
    }

    fn connect_mount(&mut self, cx: &mut Context<Self>) {
        let driver = self.selected_driver;
        let port = self.mount_port_input.read(cx).text().trim().to_string();
        let baud_raw = self.mount_baud_input.read(cx).text().trim().to_string();

        let baud_rate = if driver == MountDriverKind::SynScanSerial {
            if port.is_empty() {
                self.validation_error =
                    Some("Enter a serial port path (e.g. /dev/ttyUSB0)".into());
                cx.notify();
                return;
            }
            match baud_raw.parse::<u32>() {
                Ok(b) if b > 0 => b,
                _ => {
                    self.validation_error =
                        Some("Invalid baud rate (use 9600 for SynScan HC)".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            baud_raw.parse::<u32>().ok().filter(|&b| b > 0).unwrap_or(9600)
        };

        self.validation_error = None;
        self.pending_site_sync = true;

        if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
            let port_copy = port.clone();
            daemon.update(cx, |model, cx| {
                model.update_hardware_config(
                    |cfg| {
                        cfg.use_simulator = driver == MountDriverKind::Simulator;
                        if !port_copy.is_empty() {
                            cfg.mount_port = port_copy;
                        }
                        cfg.mount_baud_rate = baud_rate;
                    },
                    cx,
                );
            });
        }

        send_mount_command(
            MountCommand::Connect {
                driver,
                port,
                baud_rate,
            },
            cx,
        );
        cx.notify();
    }

    fn sync_observer_site(&self, cx: &App) {
        let Some(obs_entity) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) else {
            return;
        };
        let pos = obs_entity.read(cx).geodetic_position();
        send_mount_command(
            MountCommand::SetSite {
                latitude_deg: pos.latitude_deg(),
                longitude_deg: pos.longitude_deg(),
                elevation_m: pos.elevation_m(),
            },
            cx,
        );
    }

    fn render_daemon_card(
        &self,
        daemon_logic: LogicState,
        daemon_status_text: SharedString,
        card_bg: Hsla,
        border: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        device_card_shell(
            "Raspberry Pi Daemon",
            "WebSocket endpoint hosting hardware drivers (NEQ6-Pro, DSLR, Focuser)",
            daemon_logic,
            daemon_status_text,
            card_bg,
            border,
            cx,
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(self.daemon_url_input.clone()))
                .child(
                    Button::new("daemon_apply_url", "Connect")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.apply_daemon_url(cx);
                        })),
                ),
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_mount_card(
        &self,
        is_daemon_connected: bool,
        is_mount_ready: bool,
        mount_logic: LogicState,
        mount_status_text: SharedString,
        ports: Vec<String>,
        mount_error: Option<String>,
        telemetry: Option<MountTelemetry>,
        card_bg: Hsla,
        border: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_serial = self.selected_driver == MountDriverKind::SynScanSerial;
        let muted = cx.theme().colors().text_muted;
        let display_error = self.validation_error.clone().or(mount_error);

        device_card_shell(
            "Sky-Watcher NEQ6-Pro Mount",
            "Connect via SynScan Hand Controller serial cable (9600 8N1) or use the built-in simulator",
            mount_logic,
            mount_status_text,
            card_bg,
            border,
            cx,
        )
        .child(
            h_flex()
                .gap_1p5()
                .child(
                    Button::new("driver_synscan", MountDriverKind::SynScanSerial.label())
                        .style(if is_serial {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_driver = MountDriverKind::SynScanSerial;
                            this.validation_error = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("driver_sim", MountDriverKind::Simulator.label())
                        .style(if is_serial {
                            ButtonStyle::Subtle
                        } else {
                            ButtonStyle::Filled
                        })
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_driver = MountDriverKind::Simulator;
                            this.validation_error = None;
                            cx.notify();
                        })),
                ),
        )
        .when(is_serial, |el| {
            el.child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                Label::new("Serial Port")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(self.mount_port_input.clone()),
                    )
                    .child(
                        v_flex()
                            .w(px(96.0))
                            .gap_1()
                            .child(Label::new("Baud").size(LabelSize::XSmall).color(Color::Muted))
                            .child(self.mount_baud_input.clone()),
                    )
                    .child(
                        Button::new("scan_ports_btn", "Scan Ports")
                            .style(ButtonStyle::Subtle)
                            .disabled(!is_daemon_connected)
                            .on_click(cx.listener(|_this, _, _, cx| {
                                send_mount_command(MountCommand::ListPorts, cx);
                            })),
                    ),
            )
            .when(!ports.is_empty(), |el| {
                let mut chips = h_flex().gap_1().flex_wrap().items_center().child(
                    Label::new("Detected:")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                );
                for (idx, p) in ports.into_iter().enumerate() {
                    let port_val = p.clone();
                    chips = chips.child(
                        Button::new(("detected_port", idx), p)
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mount_port_input.update(cx, |field, cx| {
                                    field.set_text(port_val.clone(), cx);
                                });
                                this.validation_error = None;
                                cx.notify();
                            })),
                    );
                }
                el.child(chips)
            })
        })
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new(
                        "mount_connect_btn",
                        if is_mount_ready {
                            "Disconnect Mount"
                        } else {
                            "Connect Mount"
                        },
                    )
                    .style(if is_mount_ready {
                        ButtonStyle::Tinted(ui::TintColor::Warning)
                    } else {
                        ButtonStyle::Filled
                    })
                    .disabled(!is_daemon_connected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_mount_ready {
                            this.pending_site_sync = false;
                            send_mount_command(MountCommand::Disconnect, cx);
                        } else {
                            this.connect_mount(cx);
                        }
                    })),
                )
                .child(
                    Button::new("mount_sync_site_btn", "Sync Observer Site")
                        .style(ButtonStyle::Subtle)
                        .disabled(!is_mount_ready)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sync_observer_site(cx);
                        })),
                ),
        )
        .when_some(display_error, |el, err| {
            el.child(
                div()
                    .px_2p5()
                    .py_1p5()
                    .rounded_md()
                    .bg(cx.theme().status().error_background)
                    .child(Label::new(err).size(LabelSize::XSmall).color(Color::Error)),
            )
        })
        .when_some(telemetry, |el, t| {
            let (ra_s, dec_s) = format_radec(t.ra_deg, t.dec_deg);
            let (az_s, alt_s) = format_altaz(t.az_deg, t.alt_deg, 10.0);
            el.child(Divider::horizontal()).child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_4()
                            .flex_wrap()
                            .text_size(px(11.5))
                            .text_color(muted)
                            .child(format!(
                                "Model: {}",
                                t.mount_model.as_deref().unwrap_or("NEQ6-Pro")
                            ))
                            .child(format!(
                                "Firmware: {}",
                                t.firmware_version.as_deref().unwrap_or("N/A")
                            ))
                            .child(format!("Pier: {}", t.pier_side.label()))
                            .child(format!("Tracking: {}", t.tracking.label())),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .font_buffer(cx)
                            .text_size(px(12.0))
                            .text_color(cx.theme().colors().text)
                            .child(format!("RA {ra_s}"))
                            .child(format!("Dec {dec_s}"))
                            .child(format!("Az {az_s}"))
                            .child(format!("Alt {}", alt_s.trim())),
                    ),
            )
        })
        .into_any_element()
    }
}

fn device_card_shell(
    title: &'static str,
    subtitle: &'static str,
    status_logic: LogicState,
    status_text: SharedString,
    card_bg: Hsla,
    border: Hsla,
    cx: &App,
) -> gpui::Div {
    v_flex()
        .gap_2p5()
        .p_4()
        .rounded_lg()
        .bg(card_bg)
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().colors().text)
                                .child(title),
                        )
                        .child(
                            Label::new(subtitle)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
                .child(StatusIndicator::new(status_logic, status_text)),
        )
}

impl Render for ConnectPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (
            is_daemon_connected,
            is_mount_ready,
            daemon_logic,
            daemon_status_text,
            mount_logic,
            mount_status_text,
            camera_logic,
            camera_status_text,
            ports,
            mount_error,
            telemetry,
        ) = cx
            .try_global::<GlobalDaemonState>()
            .map(|g| {
                let m = g.0.read(cx);
                let s = &m.state;
                let connected = s.connection == ConnectionState::Connected;
                let mount_ready = connected && s.mount == MountStatus::Ready;

                let (d_logic, d_base) = connection_status_presentation(&s.connection);
                let d_text = if connected {
                    match &s.daemon {
                        DaemonStatus::Idle => SharedString::from("Connected (Idle)"),
                        DaemonStatus::Capturing => SharedString::from("Connected (Capturing)"),
                        DaemonStatus::Error(e) => {
                            SharedString::from(format!("Connected (Error: {e})"))
                        }
                    }
                } else {
                    d_base
                };

                let (m_logic, m_text) = if mount_ready {
                    let suffix = s
                        .mount_telemetry
                        .as_ref()
                        .map(|t| {
                            if t.is_slewing {
                                "Slewing"
                            } else if t.is_parked {
                                "Parked"
                            } else {
                                "Tracking / Ready"
                            }
                        })
                        .unwrap_or("Ready");
                    (
                        LogicState::Valid,
                        SharedString::from(format!("Connected ({suffix})")),
                    )
                } else {
                    mount_status_presentation(&s.mount, connected)
                };

                let (c_logic, c_text) = camera_status_presentation(&s.camera, connected);

                (
                    connected,
                    mount_ready,
                    d_logic,
                    d_text,
                    m_logic,
                    m_text,
                    c_logic,
                    c_text,
                    s.available_mount_ports.clone(),
                    m.last_mount_error.clone(),
                    s.mount_telemetry.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    false,
                    false,
                    LogicState::Invalid,
                    SharedString::from("Disconnected"),
                    LogicState::Muted,
                    SharedString::from("Offline"),
                    LogicState::Muted,
                    SharedString::from("Offline"),
                    Vec::new(),
                    None,
                    None,
                )
            });

        let card_bg = cx.theme().colors().elevated_surface_background;
        let border = cx.theme().colors().border_variant;

        let daemon_card =
            self.render_daemon_card(daemon_logic, daemon_status_text, card_bg, border, cx);
        let mount_card = self.render_mount_card(
            is_daemon_connected,
            is_mount_ready,
            mount_logic,
            mount_status_text,
            ports,
            mount_error,
            telemetry,
            card_bg,
            border,
            cx,
        );
        let camera_card = device_card_shell(
            "Imaging Camera (DSLR / gPhoto2)",
            "Auto-detected over USB by the Raspberry Pi daemon",
            camera_logic,
            camera_status_text,
            card_bg,
            border,
            cx,
        );
        let expansion_row = h_flex()
            .gap_3()
            .child(future_device_card(
                "Motorized Focuser",
                "EAF / MoonLite / INDI serial focuser slot (ready for driver integration)",
                card_bg,
                border,
                cx,
            ))
            .child(future_device_card(
                "Guide Camera & Filter Wheel",
                "Autoguider & EFW telemetry slot (ready for driver integration)",
                card_bg,
                border,
                cx,
            ));

        v_flex()
            .id("connect_workspace_scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().colors().background)
            .p_6()
            .items_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(860.0))
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().colors().text)
                                    .child("Hardware Connections"),
                            )
                            .child(
                                Label::new("Connect to your observatory daemon and verify all instruments before imaging.")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(daemon_card)
                    .child(mount_card)
                    .child(camera_card)
                    .child(expansion_row),
            )
    }
}

fn future_device_card(
    title: &'static str,
    subtitle: &'static str,
    bg: Hsla,
    border: Hsla,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .gap_1()
        .p_3p5()
        .rounded_lg()
        .bg(bg.opacity(0.65))
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().colors().text_muted)
                        .child(title),
                )
                .child(StatusIndicator::new(
                    LogicState::Muted,
                    SharedString::from("Not Configured"),
                )),
        )
        .child(
            Label::new(subtitle)
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
}

impl Focusable for ConnectPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ConnectPanel {
    fn persistent_name() -> &'static str {
        "connect_panel"
    }

    fn title(&self, _cx: &App) -> SharedString {
        "Connect".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Connect)
    }
}
