use client::GlobalDaemonState;
use components::{LogicState, StatusIndicator};
use gpui::{App, Context, Entity, Global, SharedString, div, px};
use protocol::{ConnectionState, MountStatus};
use settings::ToggleLogs;
use ui::{
    ButtonSize, Divider, FixedWidth, IconButton, IconName, IconSize, StyledTypography, Tooltip,
    prelude::*,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TelemetryCoordMode {
    #[default]
    Horizontal,
    Equatorial,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorSkyCoords {
    pub az_deg: f64,
    pub alt_deg: f64,
    pub ra_deg: f64,
    pub dec_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetariumTelemetry {
    pub mode: TelemetryCoordMode,
    pub fov_deg: f64,
    pub cursor: Option<CursorSkyCoords>,
}

pub struct PlanetariumStatusModel {
    telemetry: Option<PlanetariumTelemetry>,
}

impl PlanetariumStatusModel {
    pub fn new() -> Self {
        Self { telemetry: None }
    }

    pub fn telemetry(&self) -> Option<PlanetariumTelemetry> {
        self.telemetry
    }

    pub fn update_telemetry(&mut self, next: PlanetariumTelemetry, cx: &mut Context<Self>) {
        let changed = match self.telemetry {
            None => true,
            Some(prev) => display_signature(&prev) != display_signature(&next),
        };
        self.telemetry = Some(next);
        if changed {
            cx.notify();
        }
    }
}

impl Default for PlanetariumStatusModel {
    fn default() -> Self {
        Self::new()
    }
}

pub struct GlobalPlanetariumStatus(pub Entity<PlanetariumStatusModel>);

impl Global for GlobalPlanetariumStatus {}

pub fn init(cx: &mut App) {
    let model = cx.new(|_| PlanetariumStatusModel::new());
    cx.set_global(GlobalPlanetariumStatus(model));
}

pub fn format_fov(fov_deg: f64) -> String {
    let fov = if fov_deg.is_finite() {
        fov_deg.max(0.0)
    } else {
        0.0
    };
    if fov >= 1.0 {
        format!("{fov:.1}°")
    } else if fov * 60.0 >= 1.0 {
        format!("{:.1}'", fov * 60.0)
    } else {
        format!("{:.1}\"", fov * 3600.0)
    }
}

pub fn format_altaz(az_deg: f64, alt_deg: f64, fov_deg: f64) -> (String, String) {
    let safe_az = if az_deg.is_finite() {
        az_deg.rem_euclid(360.0)
    } else {
        0.0
    };
    let safe_alt = if alt_deg.is_finite() {
        alt_deg.clamp(-90.0, 90.0)
    } else {
        0.0
    };

    if fov_deg < 1.0 {
        let az_hundredths = (safe_az * 100.0).round() as u32 % 36_000;
        let alt_hundredths = (safe_alt * 100.0).round() as i32;
        let alt_val = if alt_hundredths == 0 {
            0.0
        } else {
            f64::from(alt_hundredths) / 100.0
        };
        (
            format!("{:06.2}°", f64::from(az_hundredths) / 100.0),
            format!("{alt_val:+6.2}°"),
        )
    } else {
        let az_tenths = (safe_az * 10.0).round() as u32 % 3_600;
        let alt_tenths = (safe_alt * 10.0).round() as i32;
        let alt_val = if alt_tenths == 0 {
            0.0
        } else {
            f64::from(alt_tenths) / 10.0
        };
        (
            format!("{:05.1}°", f64::from(az_tenths) / 10.0),
            format!("{alt_val:+5.1}°"),
        )
    }
}

pub fn format_radec(ra_deg: f64, dec_deg: f64) -> (String, String) {
    let safe_ra = if ra_deg.is_finite() {
        ra_deg.rem_euclid(360.0)
    } else {
        0.0
    };
    let ra_secs = ((safe_ra / 15.0) * 3600.0).round() as u32 % 86_400;
    let h = ra_secs / 3600;
    let m = (ra_secs % 3600) / 60;
    let s = ra_secs % 60;
    let ra_text = format!("{h:02}h {m:02}m {s:02}s");

    let clamped_dec = if dec_deg.is_finite() {
        dec_deg.clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let dec_arcsecs = (clamped_dec.abs() * 3600.0).round() as u32;
    let sign = if dec_arcsecs == 0 || clamped_dec >= 0.0 {
        '+'
    } else {
        '-'
    };
    let d = dec_arcsecs / 3600;
    let dm = (dec_arcsecs % 3600) / 60;
    let ds = dec_arcsecs % 60;
    let dec_text = format!("{sign}{d:02}° {dm:02}' {ds:02}\"");

    (ra_text, dec_text)
}

fn format_coord_pair(
    telemetry: &PlanetariumTelemetry,
) -> (&'static str, String, &'static str, String) {
    match telemetry.mode {
        TelemetryCoordMode::Horizontal => {
            if let Some(c) = telemetry.cursor {
                let (az, alt) = format_altaz(c.az_deg, c.alt_deg, telemetry.fov_deg);
                ("Az", az, "Alt", alt)
            } else if telemetry.fov_deg < 1.0 {
                ("Az", "---.--°".to_string(), "Alt", "---.--°".to_string())
            } else {
                ("Az", "---.-°".to_string(), "Alt", "---.-°".to_string())
            }
        }
        TelemetryCoordMode::Equatorial => {
            if let Some(c) = telemetry.cursor {
                let (ra, dec) = format_radec(c.ra_deg, c.dec_deg);
                ("RA", ra, "Dec", dec)
            } else {
                (
                    "RA",
                    "--h --m --s".to_string(),
                    "Dec",
                    "---° --' --\"".to_string(),
                )
            }
        }
    }
}

fn display_signature(
    t: &PlanetariumTelemetry,
) -> (TelemetryCoordMode, bool, String, String, String) {
    let (_, v1, _, v2) = format_coord_pair(t);
    (t.mode, t.cursor.is_some(), format_fov(t.fov_deg), v1, v2)
}

pub struct StatusBar {
    show_planetarium_telemetry: bool,
}

impl StatusBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let global_entity = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone());
        if let Some(entity) = global_entity {
            cx.observe(&entity, |_this, _model, cx| {
                cx.notify();
            })
            .detach();
        }
        let planetarium_status = cx
            .try_global::<GlobalPlanetariumStatus>()
            .map(|g| g.0.clone());
        if let Some(entity) = planetarium_status {
            cx.observe(&entity, |_this, _model, cx| {
                cx.notify();
            })
            .detach();
        }
        Self {
            show_planetarium_telemetry: true,
        }
    }

    pub fn set_show_planetarium_telemetry(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_planetarium_telemetry != show {
            self.show_planetarium_telemetry = show;
            cx.notify();
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let daemon_state = cx
            .try_global::<GlobalDaemonState>()
            .map(|g| g.0.read(cx).state.clone())
            .unwrap_or_default();

        let is_connected = daemon_state.connection == ConnectionState::Connected;
        let (daemon_logic, daemon_label) =
            components::connection_status_presentation(&daemon_state.connection);
        let daemon_text = SharedString::from(format!("Daemon: {daemon_label}"));

        let (camera_logic, camera_label) =
            components::camera_status_presentation(&daemon_state.camera, is_connected);
        let camera_text = SharedString::from(format!("Camera: {camera_label}"));

        let (mount_logic, mount_text) = if is_connected && daemon_state.mount == MountStatus::Ready
        {
            if let Some(t) = &daemon_state.mount_telemetry {
                if t.is_slewing {
                    (LogicState::Pending, SharedString::from("Mount: Slewing"))
                } else if t.is_parked {
                    (LogicState::Muted, SharedString::from("Mount: Parked"))
                } else {
                    (LogicState::Valid, SharedString::from("Mount: Ready"))
                }
            } else {
                (LogicState::Valid, SharedString::from("Mount: Ready"))
            }
        } else {
            let (logic, label) =
                components::mount_status_presentation(&daemon_state.mount, is_connected);
            (logic, SharedString::from(format!("Mount: {label}")))
        };

        let telemetry = if self.show_planetarium_telemetry {
            cx.try_global::<GlobalPlanetariumStatus>()
                .and_then(|g| g.0.read(cx).telemetry())
        } else {
            None
        };

        let label_color = cx.theme().colors().text_muted;
        let text_color = cx.theme().colors().text;
        let dimmed_color = label_color.opacity(0.6);

        let mut right_items = h_flex().items_center().gap_2p5();
        if let Some(telemetry) = telemetry {
            let (k1, v1, k2, v2) = format_coord_pair(&telemetry);
            let coord_value_color = if telemetry.cursor.is_some() {
                text_color
            } else {
                dimmed_color
            };
            let fov_text = format_fov(telemetry.fov_deg);

            right_items = right_items
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(div().text_size(px(11.0)).text_color(label_color).child(k1))
                                .child(
                                    div()
                                        .font_buffer(cx)
                                        .text_size(px(11.5))
                                        .text_color(coord_value_color)
                                        .child(v1),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(div().text_size(px(11.0)).text_color(label_color).child(k2))
                                .child(
                                    div()
                                        .font_buffer(cx)
                                        .text_size(px(11.5))
                                        .text_color(coord_value_color)
                                        .child(v2),
                                ),
                        ),
                )
                .child(Divider::vertical())
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(label_color)
                                .child("FOV"),
                        )
                        .child(
                            div()
                                .font_buffer(cx)
                                .text_size(px(11.5))
                                .text_color(text_color)
                                .child(fov_text),
                        ),
                )
                .child(Divider::vertical());
        }

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
                    .child(StatusIndicator::new(mount_logic, mount_text))
                    .child(Divider::vertical())
                    .child(StatusIndicator::new(camera_logic, camera_text)),
            )
            .child(
                right_items
                    .child(
                        IconButton::new("status_logs_button", IconName::FileDoc)
                            // `IconButtonShape::Square` derives its size from the icon, so
                            // size explicitly to match the neighbouring "Apogee" button.
                            .size(ButtonSize::Default)
                            .width(ButtonSize::Default.rems())
                            .icon_size(IconSize::Small)
                            .style(ButtonStyle::Subtle)
                            .tooltip(Tooltip::text("Toggle Logs"))
                            .on_click(|_event, window, cx| {
                                window.dispatch_action(Box::new(ToggleLogs), cx);
                            }),
                    )
                    .child(
                        Button::new("status_button", "Apogee")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Subtle),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_fov_switches_units_cleanly() {
        assert_eq!(format_fov(180.0), "180.0°");
        assert_eq!(format_fov(1.0), "1.0°");
        assert_eq!(format_fov(0.5), "30.0'");
        assert_eq!(format_fov(1.0 / 120.0), "30.0\"");
    }

    #[test]
    fn format_altaz_wraps_azimuth_and_avoids_negative_zero() {
        let (az, alt) = format_altaz(359.99, -0.02, 45.0);
        assert_eq!(az, "000.0°");
        assert_eq!(alt, " +0.0°");

        let (az_hi, alt_hi) = format_altaz(52.349, -2.94, 0.5);
        assert_eq!(az_hi, "052.35°");
        assert_eq!(alt_hi, " -2.94°");

        let (az_neg_wrap, alt_zero_hi) = format_altaz(-0.01, -0.001, 0.5);
        assert_eq!(az_neg_wrap, "359.99°");
        assert_eq!(alt_zero_hi, " +0.00°");
    }

    #[test]
    fn format_radec_wraps_seconds_and_avoids_negative_zero_dec() {
        let (ra_wrap, dec_zero) = format_radec(359.999, -0.00001);
        assert_eq!(ra_wrap, "00h 00m 00s");
        assert_eq!(dec_zero, "+00° 00' 00\"");

        let (ra, dec) = format_radec(75.75, -13.318_055_6);
        assert_eq!(ra, "05h 03m 00s");
        assert_eq!(dec, "-13° 19' 05\"");
    }

    #[test]
    fn coord_placeholders_only_show_active_mode() {
        let hz = PlanetariumTelemetry {
            mode: TelemetryCoordMode::Horizontal,
            fov_deg: 90.0,
            cursor: None,
        };
        assert_eq!(
            format_coord_pair(&hz),
            ("Az", "---.-°".to_string(), "Alt", "---.-°".to_string())
        );

        let eq = PlanetariumTelemetry {
            mode: TelemetryCoordMode::Equatorial,
            fov_deg: 0.5,
            cursor: None,
        };
        assert_eq!(
            format_coord_pair(&eq),
            (
                "RA",
                "--h --m --s".to_string(),
                "Dec",
                "---° --' --\"".to_string(),
            )
        );
    }
}
