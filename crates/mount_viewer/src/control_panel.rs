//! Right-dock Mount Control Panel for the Mount workspace.

use celestial::coords::{parse_dec_degrees, parse_ra_degrees};
use client::{GlobalDaemonState, send_mount_command};
use components::{LogicState, Slider, SliderEvent, StatusIndicator, TextField, TextFieldEvent};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseUpEvent, Render,
    SharedString, Window, div, prelude::*, px,
};
use protocol::{
    ConnectionState, MountCommand, MountLimits, MountStatus, SlewDirection, TrackingRate,
};
use theme::ActiveTheme;
use ui::{Button, ButtonStyle, Color, Divider, Label, LabelSize, TintColor, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::bridge::MountViewerBridge;
use crate::slew_keys::ManualSlewKeyController;

const MIN_ALT_RANGE: (f64, f64) = (0.0, 80.0);
const MAX_ALT_RANGE: (f64, f64) = (5.0, 90.0);
const MERIDIAN_RANGE: (f64, f64) = (0.0, 45.0);
const AZ_RANGE: (f64, f64) = (0.0, 360.0);

fn rate_to_slider(rate: u8) -> f32 {
    (f32::from(rate.clamp(1, 9)) - 1.0) / 8.0
}

fn slider_to_rate(value: f32) -> u8 {
    ((value.clamp(0.0, 1.0) * 8.0).round() as u8 + 1).clamp(1, 9)
}

fn rate_speed_label(rate: u8) -> &'static str {
    match rate.clamp(1, 9) {
        1 => "0.5×",
        2 => "1×",
        3 => "8×",
        4 => "16×",
        5 => "32×",
        6 => "64×",
        7 => "128×",
        8 => "400×",
        _ => "Max",
    }
}

fn norm_to_range(t: f32, range: (f64, f64)) -> f64 {
    (range.0 + f64::from(t.clamp(0.0, 1.0)) * (range.1 - range.0)).round()
}

fn range_to_norm(val: f64, range: (f64, f64)) -> f32 {
    ((val - range.0) / (range.1 - range.0).max(1e-6)).clamp(0.0, 1.0) as f32
}

struct MountUiSnapshot {
    is_ready: bool,
    is_parked: bool,
    is_slewing: bool,
    tracking: TrackingRate,
    mount_error: Option<String>,
}

pub struct MountControlPanel {
    bridge: MountViewerBridge,
    focus_handle: FocusHandle,
    slew_rate: u8,
    slew_rate_slider: Entity<Slider>,
    limits: MountLimits,
    min_alt_slider: Entity<Slider>,
    max_alt_slider: Entity<Slider>,
    meridian_slider: Entity<Slider>,
    az_start_slider: Entity<Slider>,
    az_end_slider: Entity<Slider>,
    ra_input: Entity<TextField>,
    dec_input: Entity<TextField>,
    input_error: Option<String>,
    slew_keys: ManualSlewKeyController,
    active_dpad_dir: Option<SlewDirection>,
}

impl EventEmitter<PanelEvent> for MountControlPanel {}

impl MountControlPanel {
    fn build_limit_slider(
        initial_val: f64,
        range: (f64, f64),
        mut set_field: impl FnMut(&mut MountLimits, f64) + 'static,
        cx: &mut Context<Self>,
    ) -> Entity<Slider> {
        let norm = range_to_norm(initial_val, range);
        let slider = cx.new(|_cx| Slider::new(norm).default_value(norm));
        cx.subscribe(
            &slider,
            move |this: &mut Self, _slider, event: &SliderEvent, cx| {
                let (v, commit) = match *event {
                    SliderEvent::Change(v) => (v, false),
                    SliderEvent::Commit(v) => (v, true),
                };
                set_field(&mut this.limits, norm_to_range(v, range));
                this.apply_limits(commit, cx);
            },
        )
        .detach();
        slider
    }

    pub fn build(bridge: MountViewerBridge, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let initial_rate = 6u8;
            let defaults = MountLimits::default();
            let focus_handle = cx.focus_handle();

            cx.on_blur(&focus_handle, window, |this: &mut Self, _window, cx| {
                this.slew_keys.release_all(cx);
                this.stop_active_dpad(cx);
            })
            .detach();

            let slew_rate_slider = cx.new(|_cx| {
                Slider::new(rate_to_slider(initial_rate))
                    .default_value(rate_to_slider(initial_rate))
                    .snap_stops(9)
            });
            cx.subscribe(
                &slew_rate_slider,
                |this: &mut Self, _slider, event: &SliderEvent, cx| {
                    let val = match *event {
                        SliderEvent::Change(v) | SliderEvent::Commit(v) => v,
                    };
                    let next_rate = slider_to_rate(val);
                    if next_rate != this.slew_rate {
                        this.slew_rate = next_rate;
                        send_mount_command(MountCommand::SetSlewRate(next_rate), cx);
                        cx.notify();
                    } else if matches!(event, SliderEvent::Commit(_)) {
                        send_mount_command(MountCommand::SetSlewRate(next_rate), cx);
                    }
                },
            )
            .detach();

            let min_alt_slider = Self::build_limit_slider(
                defaults.min_altitude_deg,
                MIN_ALT_RANGE,
                |l, v| l.min_altitude_deg = v,
                cx,
            );
            let max_alt_slider = Self::build_limit_slider(
                defaults.max_altitude_deg,
                MAX_ALT_RANGE,
                |l, v| l.max_altitude_deg = v,
                cx,
            );
            let meridian_slider = Self::build_limit_slider(
                defaults.meridian_limit_deg,
                MERIDIAN_RANGE,
                |l, v| l.meridian_limit_deg = v,
                cx,
            );
            let az_start_slider = Self::build_limit_slider(
                defaults.az_avoid_start_deg,
                AZ_RANGE,
                |l, v| l.az_avoid_start_deg = v,
                cx,
            );
            let az_end_slider = Self::build_limit_slider(
                defaults.az_avoid_end_deg,
                AZ_RANGE,
                |l, v| l.az_avoid_end_deg = v,
                cx,
            );

            let ra_input =
                cx.new(|cx| TextField::new("", window, cx).placeholder("00:42:44 or 10.68"));
            let dec_input =
                cx.new(|cx| TextField::new("", window, cx).placeholder("+41:16:09 or 41.27"));

            for input in [&ra_input, &dec_input] {
                cx.subscribe(
                    input,
                    |this: &mut Self, _field, event: &TextFieldEvent, cx| match event {
                        TextFieldEvent::Change(_) => {
                            if this.input_error.take().is_some() {
                                cx.notify();
                            }
                        }
                        TextFieldEvent::Submit(_) => {
                            this.submit_goto(cx);
                        }
                        _ => {}
                    },
                )
                .detach();
            }

            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |this: &mut Self, model, cx| {
                    let snapshot = model
                        .read(cx)
                        .state
                        .mount_telemetry
                        .as_ref()
                        .map(|t| (t.slew_rate, t.limits));
                    if let Some((rate, limits)) = snapshot {
                        if !this.slew_rate_slider.read(cx).is_dragging() && this.slew_rate != rate {
                            this.slew_rate = rate;
                            this.slew_rate_slider.update(cx, |s, cx| {
                                s.set_value(rate_to_slider(rate), cx);
                            });
                        }
                        let any_limit_dragging = this.min_alt_slider.read(cx).is_dragging()
                            || this.max_alt_slider.read(cx).is_dragging()
                            || this.meridian_slider.read(cx).is_dragging()
                            || this.az_start_slider.read(cx).is_dragging()
                            || this.az_end_slider.read(cx).is_dragging();
                        if !any_limit_dragging && this.limits != limits {
                            this.limits = limits;
                            this.sync_limit_sliders(cx);
                        }
                    }
                    cx.notify();
                })
                .detach();
            }

            Self {
                bridge,
                focus_handle,
                slew_rate: initial_rate,
                slew_rate_slider,
                limits: defaults,
                min_alt_slider,
                max_alt_slider,
                meridian_slider,
                az_start_slider,
                az_end_slider,
                ra_input,
                dec_input,
                input_error: None,
                slew_keys: ManualSlewKeyController::default(),
                active_dpad_dir: None,
            }
        })
    }

    fn mount_snapshot(&self, cx: &App) -> MountUiSnapshot {
        cx.try_global::<GlobalDaemonState>()
            .map(|g| {
                let model = g.0.read(cx);
                let is_ready = model.state.connection == ConnectionState::Connected
                    && model.state.mount == MountStatus::Ready;
                let (is_parked, is_slewing, tracking) = model
                    .state
                    .mount_telemetry
                    .as_ref()
                    .map(|t| (t.is_parked, t.is_slewing, t.tracking))
                    .unwrap_or((true, false, TrackingRate::Off));
                MountUiSnapshot {
                    is_ready,
                    is_parked,
                    is_slewing,
                    tracking,
                    mount_error: model.last_mount_error.clone(),
                }
            })
            .unwrap_or(MountUiSnapshot {
                is_ready: false,
                is_parked: true,
                is_slewing: false,
                tracking: TrackingRate::Off,
                mount_error: None,
            })
    }

    fn sync_limit_sliders(&self, cx: &mut Context<Self>) {
        let l = self.limits;
        self.min_alt_slider.update(cx, |s, cx| {
            s.set_value(range_to_norm(l.min_altitude_deg, MIN_ALT_RANGE), cx);
        });
        self.max_alt_slider.update(cx, |s, cx| {
            s.set_value(range_to_norm(l.max_altitude_deg, MAX_ALT_RANGE), cx);
        });
        self.meridian_slider.update(cx, |s, cx| {
            s.set_value(range_to_norm(l.meridian_limit_deg, MERIDIAN_RANGE), cx);
        });
        self.az_start_slider.update(cx, |s, cx| {
            s.set_value(range_to_norm(l.az_avoid_start_deg, AZ_RANGE), cx);
        });
        self.az_end_slider.update(cx, |s, cx| {
            s.set_value(range_to_norm(l.az_avoid_end_deg, AZ_RANGE), cx);
        });
    }

    fn apply_limits(&mut self, send_to_daemon: bool, cx: &mut Context<Self>) {
        let before = self.limits;
        self.limits.sanitize();
        if self.limits != before {
            self.sync_limit_sliders(cx);
        }
        if send_to_daemon {
            self.bridge.set_limits(self.limits);
            send_mount_command(MountCommand::SetLimits(self.limits), cx);
        } else {
            self.bridge.set_limits_preview(self.limits);
        }
        cx.notify();
    }

    fn start_dpad(&mut self, direction: SlewDirection, cx: &mut Context<Self>) {
        let snap = self.mount_snapshot(cx);
        if !snap.is_ready || snap.is_parked {
            return;
        }
        self.active_dpad_dir = Some(direction);
        send_mount_command(
            MountCommand::StartManualSlew {
                direction,
                rate: self.slew_rate,
            },
            cx,
        );
        cx.notify();
    }

    fn stop_active_dpad(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = self.active_dpad_dir.take() {
            send_mount_command(MountCommand::StopManualSlew { axis: dir.axis() }, cx);
            cx.notify();
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        let snap = self.mount_snapshot(cx);
        let can_slew = snap.is_ready && !snap.is_parked;
        if self
            .slew_keys
            .handle_key_down(event, can_slew, self.slew_rate, cx)
        {
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        self.slew_keys.handle_key_up(event, self.slew_rate, cx);
    }

    fn parse_target_radec(&self, cx: &App) -> Result<(f64, f64), String> {
        let ra_raw = self.ra_input.read(cx).text().trim().to_string();
        let dec_raw = self.dec_input.read(cx).text().trim().to_string();
        let ra_deg = parse_ra_degrees(&ra_raw)
            .ok_or_else(|| "Invalid RA (use degrees e.g. 10.68 or HH:MM:SS)".to_string())?;
        let dec_deg = parse_dec_degrees(&dec_raw)
            .ok_or_else(|| "Invalid Dec (use degrees e.g. +41.27 or +DD:MM:SS)".to_string())?;
        Ok((ra_deg, dec_deg))
    }

    fn submit_goto(&mut self, cx: &mut Context<Self>) {
        let snap = self.mount_snapshot(cx);
        if !snap.is_ready || snap.is_parked {
            return;
        }
        match self.parse_target_radec(cx) {
            Ok((ra_deg, dec_deg)) => {
                self.input_error = None;
                send_mount_command(MountCommand::SlewToEquatorial { ra_deg, dec_deg }, cx);
            }
            Err(err) => self.input_error = Some(err),
        }
        cx.notify();
    }

    fn render_top_bar(&self, snap: &MountUiSnapshot, cx: &mut Context<Self>) -> AnyElement {
        let (status_logic, status_label) = if !snap.is_ready {
            (LogicState::Invalid, "Disconnected")
        } else if snap.is_slewing {
            (LogicState::Pending, "Slewing")
        } else if snap.is_parked {
            (LogicState::Muted, "Parked")
        } else {
            (LogicState::Valid, "Ready")
        };
        let is_ready = snap.is_ready;
        let is_parked = snap.is_parked;

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(StatusIndicator::new(
                        status_logic,
                        SharedString::from(status_label),
                    ))
                    .child(
                        Button::new("mount_stop_all", "Stop")
                            .style(ButtonStyle::Tinted(TintColor::Error))
                            .label_size(LabelSize::Small)
                            .disabled(!is_ready)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_dpad_dir = None;
                                send_mount_command(MountCommand::StopAll, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        div().flex_1().child(
                            Button::new(
                                "mount_park_toggle",
                                if is_parked { "Unpark" } else { "Park" },
                            )
                            .full_width()
                            .style(ButtonStyle::Tinted(if is_parked {
                                TintColor::Accent
                            } else {
                                TintColor::Warning
                            }))
                            .label_size(LabelSize::Small)
                            .disabled(!is_ready)
                            .on_click(cx.listener(
                                move |_this, _, _, cx| {
                                    let cmd = if is_parked {
                                        MountCommand::Unpark
                                    } else {
                                        MountCommand::Park
                                    };
                                    send_mount_command(cmd, cx);
                                },
                            )),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("mount_flip_btn", "Meridian Flip")
                                .full_width()
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .disabled(!is_ready || is_parked)
                                .on_click(cx.listener(|_this, _, _, cx| {
                                    send_mount_command(MountCommand::MeridianFlip, cx);
                                })),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        div().flex_1().child(
                            Button::new("mount_home_btn", "Go Home")
                                .full_width()
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::XSmall)
                                .disabled(!is_ready || is_parked)
                                .on_click(cx.listener(|_this, _, _, cx| {
                                    send_mount_command(MountCommand::GoHome, cx);
                                })),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("mount_set_park_btn", "Set Park Here")
                                .full_width()
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::XSmall)
                                .disabled(!is_ready)
                                .on_click(cx.listener(|_this, _, _, cx| {
                                    send_mount_command(MountCommand::SetParkHere, cx);
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_tracking_section(&self, snap: &MountUiSnapshot, cx: &mut Context<Self>) -> AnyElement {
        let mut tracking_row = h_flex().gap_1().flex_wrap();
        for (idx, &rate) in TrackingRate::all().iter().enumerate() {
            let active = snap.tracking == rate;
            tracking_row = tracking_row.child(
                Button::new(("track_rate", idx), rate.label())
                    .style(if active {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::XSmall)
                    .disabled(!snap.is_ready || snap.is_parked)
                    .on_click(cx.listener(move |_this, _, _, cx| {
                        send_mount_command(MountCommand::SetTracking(rate), cx);
                    })),
            );
        }

        v_flex()
            .gap_1p5()
            .child(section_header("Tracking", cx))
            .child(tracking_row)
            .into_any_element()
    }

    fn render_dpad_button(
        &self,
        id: &'static str,
        label: &'static str,
        direction: SlewDirection,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.active_dpad_dir == Some(direction);
        let colors = cx.theme().colors();
        let bg = if active {
            colors.element_selected
        } else {
            colors.element_background
        };
        div()
            .id(id)
            .w(px(30.0))
            .h(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(colors.border_variant)
            .bg(bg)
            .when(!disabled, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(colors.element_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            this.start_dpad(direction, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.stop_active_dpad(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.stop_active_dpad(cx);
                        }),
                    )
            })
            .child(
                Label::new(label)
                    .size(LabelSize::Small)
                    .color(if disabled {
                        Color::Disabled
                    } else {
                        Color::Default
                    }),
            )
            .into_any_element()
    }

    fn render_manual_slew_section(
        &self,
        snap: &MountUiSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slew_rate = self.slew_rate;
        let disabled = !snap.is_ready || snap.is_parked;
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(section_header("Manual Slew", cx))
                    .child(
                        Label::new(format!(
                            "Rate {slew_rate} ({})",
                            rate_speed_label(slew_rate)
                        ))
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    ),
            )
            .child(self.slew_rate_slider.clone())
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(self.render_dpad_button(
                        "slew_n",
                        "N",
                        SlewDirection::North,
                        disabled,
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(self.render_dpad_button(
                                "slew_w",
                                "W",
                                SlewDirection::West,
                                disabled,
                                cx,
                            ))
                            .child(
                                Button::new("slew_stop_center", "■")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .disabled(!snap.is_ready)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.active_dpad_dir = None;
                                        send_mount_command(MountCommand::StopAll, cx);
                                    })),
                            )
                            .child(self.render_dpad_button(
                                "slew_e",
                                "E",
                                SlewDirection::East,
                                disabled,
                                cx,
                            )),
                    )
                    .child(self.render_dpad_button(
                        "slew_s",
                        "S",
                        SlewDirection::South,
                        disabled,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_goto_section(&self, snap: &MountUiSnapshot, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1p5()
            .child(section_header("GoTo / Sync", cx))
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .w(px(28.0))
                            .child(Label::new("RA").size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .child(div().flex_1().child(self.ra_input.clone())),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div().w(px(28.0)).child(
                            Label::new("Dec")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                    )
                    .child(div().flex_1().child(self.dec_input.clone())),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        div().flex_1().child(
                            Button::new("mount_goto_btn", "GoTo")
                                .full_width()
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .disabled(!snap.is_ready || snap.is_parked)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.submit_goto(cx);
                                })),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("mount_sync_btn", "Sync")
                                .full_width()
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .disabled(!snap.is_ready)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    match this.parse_target_radec(cx) {
                                        Ok((ra_deg, dec_deg)) => {
                                            this.input_error = None;
                                            send_mount_command(
                                                MountCommand::SyncEquatorial { ra_deg, dec_deg },
                                                cx,
                                            );
                                        }
                                        Err(err) => this.input_error = Some(err),
                                    }
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .when_some(self.input_error.clone(), |el, err| {
                el.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
            })
            .into_any_element()
    }

    fn render_limits_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let limits = self.limits;
        v_flex()
            .gap_2()
            .child(section_header("Safety & Meridian Limits", cx))
            .child(limit_slider_row(
                "Min Altitude",
                format!("{:.0}°", limits.min_altitude_deg),
                self.min_alt_slider.clone(),
            ))
            .child(limit_slider_row(
                "Zenith Limit",
                format!("{:.0}°", limits.max_altitude_deg),
                self.max_alt_slider.clone(),
            ))
            .child(limit_slider_row(
                "Meridian Limit",
                format!("{:.0}°", limits.meridian_limit_deg),
                self.meridian_slider.clone(),
            ))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("Auto Meridian Flip")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new(
                            "toggle_auto_flip",
                            if limits.auto_meridian_flip {
                                "On"
                            } else {
                                "Off"
                            },
                        )
                        .style(if limits.auto_meridian_flip {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .label_size(LabelSize::XSmall)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.limits.auto_meridian_flip = !this.limits.auto_meridian_flip;
                            this.apply_limits(true, cx);
                        })),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("Azimuth Exclusion")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new(
                            "toggle_az_excl",
                            if limits.az_avoid_enabled { "On" } else { "Off" },
                        )
                        .style(if limits.az_avoid_enabled {
                            ButtonStyle::Filled
                        } else {
                            ButtonStyle::Subtle
                        })
                        .label_size(LabelSize::XSmall)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.limits.az_avoid_enabled = !this.limits.az_avoid_enabled;
                            this.apply_limits(true, cx);
                        })),
                    ),
            )
            .when(limits.az_avoid_enabled, |el| {
                el.child(limit_slider_row(
                    "Sector Start",
                    format!("{:.0}°", limits.az_avoid_start_deg),
                    self.az_start_slider.clone(),
                ))
                .child(limit_slider_row(
                    "Sector End",
                    format!("{:.0}°", limits.az_avoid_end_deg),
                    self.az_end_slider.clone(),
                ))
            })
            .into_any_element()
    }
}

fn section_header(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().colors().text_muted)
        .child(title)
}

fn limit_slider_row(
    title: &'static str,
    value: String,
    slider: Entity<Slider>,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    Label::new(title)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .child(Label::new(value).size(LabelSize::XSmall)),
        )
        .child(slider)
}

impl Render for MountControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.mount_snapshot(cx);

        v_flex()
            .id("mount_control_scroll")
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key_down(event, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                this.handle_key_up(event, cx);
            }))
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .bg(cx.theme().colors().panel_background)
            .child(self.render_top_bar(&snap, cx))
            .when_some(snap.mount_error.clone(), |el, err| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(cx.theme().status().error_background)
                        .child(Label::new(err).size(LabelSize::XSmall).color(Color::Error)),
                )
            })
            .child(Divider::horizontal())
            .child(self.render_tracking_section(&snap, cx))
            .child(Divider::horizontal())
            .child(self.render_manual_slew_section(&snap, cx))
            .child(Divider::horizontal())
            .child(self.render_goto_section(&snap, cx))
            .child(Divider::horizontal())
            .child(self.render_limits_section(cx))
    }
}

impl Focusable for MountControlPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MountControlPanel {
    fn persistent_name() -> &'static str {
        "mount_control_panel"
    }

    fn title(&self, _cx: &App) -> SharedString {
        "Mount Control".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Mount)
    }
}
