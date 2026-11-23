//! Center 3D Mount Attitude & Safety-Limits Viewport Panel for the Mount workspace.

use std::sync::{Arc, Mutex};

use celestial::ObservationContext;
use client::{GlobalDaemonState, send_mount_command};
use components::{Slider, SliderEvent};
use futures::StreamExt;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    Hsla, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Window, div, prelude::*,
    px, rgb, svg,
};
use mount_renderer::{MountSceneState, OrbitCamera, WidgetVisibility};
use observer::GlobalObserver;
use protocol::{ConnectionState, MountCommand, MountStatus};
use settings::{GlobalSettings, LayerPillStyle, QuickLayer};
use status_bar::{format_altaz, format_radec};
use theme::ActiveTheme;
use ui::{Button, ButtonStyle, Color, Label, LabelSize, StyledTypography, Tooltip, prelude::*};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

use crate::bridge::MountViewerBridge;
use crate::gizmos::{
    GizmoHandle, drag_handle_to_limits, handle_screen_position, hit_test_handle, paint_axis_gizmos,
};
use crate::slew_keys::ManualSlewKeyController;

pub struct MountViewportPanel {
    bridge: MountViewerBridge,
    cached_texture: Option<Arc<wgpu::TextureView>>,
    focus_handle: FocusHandle,
    viewport_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    last_drag_pos: Option<Point<Pixels>>,
    cursor_pos: Option<Point<Pixels>>,
    hovered_gizmo: Option<GizmoHandle>,
    dragging_gizmo: Option<GizmoHandle>,
    slew_keys: ManualSlewKeyController,
    active_opacity_layer: Option<QuickLayer>,
    opacity_slider: Entity<Slider>,
}

impl EventEmitter<PanelEvent> for MountViewportPanel {}

impl MountViewportPanel {
    pub fn build(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let (tx, mut rx) = futures::channel::mpsc::channel::<()>(1);
        let tx = Mutex::new(tx);

        let bridge = MountViewerBridge::new();
        let notify_ui = move || {
            if let Ok(mut guard) = tx.lock() {
                let _ = guard.try_send(());
            }
        };

        if let Some(bevy_bridge) = cx
            .try_global::<crate::SharedPlanetariumBridge>()
            .map(|g| g.0.clone())
        {
            let listener_id = bevy_bridge.add_frame_listener(notify_ui);
            if let (Some(device), Some(queue)) = (window.wgpu_device(), window.wgpu_queue()) {
                bridge.attach_to_planetarium(
                    &bevy_bridge,
                    (*device).clone(),
                    (*queue).clone(),
                    listener_id,
                );
            } else {
                bevy_bridge.remove_frame_listener(listener_id);
            }
        }

        cx.new(|cx| {
            let focus_handle = cx.focus_handle();
            cx.on_blur(&focus_handle, window, |this: &mut Self, _window, cx| {
                this.slew_keys.release_all(cx);
            })
            .detach();

            sync_observer_to_bridge(&bridge, cx);
            sync_daemon_to_bridge(&bridge, cx);

            let opacity_slider = cx.new(|_cx| Slider::new(1.0).default_value(1.0));
            cx.subscribe(
                &opacity_slider,
                |this: &mut Self, _slider, event: &SliderEvent, cx| {
                    let Some(layer) = this.active_opacity_layer else {
                        return;
                    };
                    if layer == QuickLayer::SkyLimits {
                        let val = match *event {
                            SliderEvent::Change(v) | SliderEvent::Commit(v) => v.clamp(0.0, 1.0),
                        };
                        this.bridge.update_scene(|s| {
                            if !s.sky_limits_enabled && val > 1e-3 {
                                s.sky_limits_enabled = true;
                            }
                            s.sky_limits_opacity = val;
                        });
                        cx.notify();
                        return;
                    }
                    let Some(settings_entity) =
                        cx.try_global::<GlobalSettings>().map(|g| g.0.clone())
                    else {
                        return;
                    };
                    match *event {
                        SliderEvent::Change(val) => {
                            settings_entity.update(cx, |model, cx| {
                                layer.preview_opacity(model, val, cx);
                            });
                        }
                        SliderEvent::Commit(val) => {
                            settings_entity.update(cx, |model, cx| {
                                layer.commit_opacity(model, val, cx);
                            });
                        }
                    }
                },
            )
            .detach();

            if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
                cx.observe(&settings_entity, |this: &mut Self, model, cx| {
                    if let Some(layer) = this.active_opacity_layer {
                        if layer != QuickLayer::SkyLimits {
                            let stored = layer.stored_opacity(model.read(cx));
                            this.opacity_slider.update(cx, |slider, cx| {
                                slider.set_value(stored, cx);
                            });
                        }
                    }
                    cx.notify();
                })
                .detach();
            }

            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |this: &mut Self, _model, cx| {
                    sync_daemon_to_bridge(&this.bridge, cx);
                    if this.bridge.is_mount_camera_active() {
                        cx.notify();
                    }
                })
                .detach();
            }

            if let Some(obs) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
                cx.observe(&obs, |this: &mut Self, _model, cx| {
                    sync_observer_to_bridge(&this.bridge, cx);
                    if this.bridge.is_mount_camera_active() {
                        cx.notify();
                    }
                })
                .detach();
            }

            cx.spawn(async move |this, cx| {
                while let Some(()) = rx.next().await {
                    if this
                        .update(cx, |this: &mut Self, cx| {
                            if this.bridge.is_mount_camera_active() {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            Self {
                bridge,
                cached_texture: None,
                focus_handle,
                viewport_bounds: Arc::new(Mutex::new(None)),
                last_drag_pos: None,
                cursor_pos: None,
                hovered_gizmo: None,
                dragging_gizmo: None,
                slew_keys: ManualSlewKeyController::default(),
                active_opacity_layer: None,
                opacity_slider,
            }
        })
    }

    pub fn bridge(&self) -> &MountViewerBridge {
        &self.bridge
    }

    fn finish_gizmo_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging_gizmo.take().is_some() {
            let limits = self.bridge.scene_state().limits;
            self.bridge.set_limits(limits);
            send_mount_command(MountCommand::SetLimits(limits), cx);
        }
    }

    fn toggle_quick_layer(&mut self, layer: QuickLayer, cx: &mut Context<Self>) {
        if layer == QuickLayer::SkyLimits {
            self.bridge.update_scene(|s| {
                let visible = s.sky_limits_enabled && s.sky_limits_opacity > 1e-3;
                if visible {
                    s.sky_limits_enabled = false;
                } else {
                    if s.sky_limits_opacity <= 1e-3 {
                        s.sky_limits_opacity = 1.0;
                    }
                    s.sky_limits_enabled = true;
                }
            });
            cx.notify();
            return;
        }
        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings_entity.update(cx, |model, cx| layer.toggle(model, cx));
        }
        cx.notify();
    }

    fn open_opacity_popover(&mut self, layer: QuickLayer, cx: &mut Context<Self>) {
        if self.active_opacity_layer == Some(layer) {
            self.active_opacity_layer = None;
            cx.notify();
            return;
        }
        let stored_opacity = if layer == QuickLayer::SkyLimits {
            self.bridge.scene_state().sky_limits_opacity
        } else {
            cx.try_global::<GlobalSettings>()
                .map_or(1.0, |g| layer.stored_opacity(g.0.read(cx)))
        };
        self.active_opacity_layer = Some(layer);
        self.opacity_slider.update(cx, |slider, cx| {
            slider.set_value(stored_opacity, cx);
        });
        cx.notify();
    }

    fn render_layer_pill(
        &self,
        pill_id: &'static str,
        layers: &[QuickLayer],
        scene: &MountSceneState,
        style: LayerPillStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let states: Vec<(QuickLayer, bool, f32)> = {
            let model = cx.try_global::<GlobalSettings>().map(|g| g.0.read(cx));
            layers
                .iter()
                .copied()
                .map(|layer| {
                    if layer == QuickLayer::SkyLimits {
                        let visible = scene.sky_limits_enabled && scene.sky_limits_opacity > 1e-3;
                        (layer, visible, scene.sky_limits_opacity)
                    } else {
                        (
                            layer,
                            model.is_none_or(|m| layer.is_visible(m)),
                            model.map_or(1.0, |m| layer.stored_opacity(m)),
                        )
                    }
                })
                .collect()
        };
        let active_popover = self.active_opacity_layer;

        h_flex()
            .id(pill_id)
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_full()
            .bg(style.pill_bg)
            .border_1()
            .border_color(style.pill_border)
            .shadow_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .children(states.into_iter().map(|(layer, is_visible, stored)| {
                let is_popover_open = active_popover == Some(layer);
                let tip = if is_visible {
                    format!(
                        "{}: {:.0}% (Right-click for opacity)",
                        layer.title(),
                        stored * 100.0
                    )
                } else {
                    format!("{}: Off (Right-click for opacity)", layer.title())
                };

                div()
                    .id(("mount_quick_layer", layer as usize))
                    .w(px(26.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(is_visible, |el| el.bg(style.active_bg))
                    .when(!is_visible, |el| el.hover(|s| s.bg(style.hover_bg)))
                    .when(is_popover_open, |el| {
                        el.border_1().border_color(style.popover_ring)
                    })
                    .tooltip(Tooltip::text(tip))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_quick_layer(layer, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.open_opacity_popover(layer, cx);
                        }),
                    )
                    .child(
                        svg()
                            .data(layer.icon())
                            .size(px(14.0))
                            .flex_none()
                            .text_color(if is_visible {
                                layer.active_icon_color(style.active_icon)
                            } else {
                                style.disabled_icon
                            }),
                    )
            }))
            .into_any_element()
    }

    fn current_slew_state(&self, cx: &App) -> (bool, bool, u8) {
        cx.try_global::<GlobalDaemonState>()
            .map(|g| {
                let model = g.0.read(cx);
                let is_ready = model.state.connection == ConnectionState::Connected
                    && model.state.mount == MountStatus::Ready;
                let (is_parked, is_slewing, rate) = model
                    .state
                    .mount_telemetry
                    .as_ref()
                    .map(|t| (t.is_parked, t.is_slewing, t.slew_rate))
                    .unwrap_or((true, false, 6));
                (is_ready && !is_parked, is_slewing, rate)
            })
            .unwrap_or((false, false, 6))
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        let (can_slew, is_slewing, slew_rate) = self.current_slew_state(cx);
        // Escape only dismisses the opacity popover when the mount is NOT slewing;
        // emergency stop always takes priority during motion.
        if event.keystroke.key.as_str() == "escape"
            && !is_slewing
            && !self.slew_keys.is_any_held()
            && self.active_opacity_layer.take().is_some()
        {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .slew_keys
            .handle_key_down(event, can_slew, slew_rate, cx)
        {
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let (_, _, slew_rate) = self.current_slew_state(cx);
        self.slew_keys.handle_key_up(event, slew_rate, cx);
    }

    fn render_widget_bar(
        &self,
        widgets: WidgetVisibility,
        panel_bg: Hsla,
        border_col: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .absolute()
            .top_3()
            .left_3()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(panel_bg)
            .border_1()
            .border_color(border_col)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new("toggle_alt_limits", "Alt Limits")
                    .style(if widgets.altitude_limits {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bridge.update_scene(|s| {
                            s.widgets.altitude_limits = !s.widgets.altitude_limits;
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("toggle_meridian", "Meridian")
                    .style(if widgets.meridian_flip {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bridge.update_scene(|s| {
                            s.widgets.meridian_flip = !s.widgets.meridian_flip;
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("toggle_az_sector", "Az Sector")
                    .style(if widgets.azimuth_sector {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bridge.update_scene(|s| {
                            s.widgets.azimuth_sector = !s.widgets.azimuth_sector;
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("toggle_ground_grid", "Compass")
                    .style(if widgets.ground_grid {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bridge.update_scene(|s| {
                            s.widgets.ground_grid = !s.widgets.ground_grid;
                        });
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_visibility_controls(
        &self,
        scene: &MountSceneState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let pill_style = LayerPillStyle {
            pill_bg: colors.elevated_surface_background.opacity(0.65),
            pill_border: colors.border_variant.opacity(0.75),
            active_bg: colors.element_selected.opacity(0.9),
            hover_bg: colors.ghost_element_hover.opacity(0.6),
            active_icon: colors.text,
            disabled_icon: colors.text_muted.opacity(0.45),
            popover_ring: colors.text_accent.opacity(0.7),
        };
        let popover_bg = colors.elevated_surface_background.opacity(0.92);
        let popover_border = colors.border;
        let text_color = colors.text;
        let muted_text = colors.text_muted;

        let active_popover_state = self.active_opacity_layer.map(|layer| {
            if layer == QuickLayer::SkyLimits {
                (
                    layer,
                    scene.sky_limits_enabled,
                    scene.sky_limits_opacity * 100.0,
                )
            } else {
                let model = cx.try_global::<GlobalSettings>().map(|g| g.0.read(cx));
                (
                    layer,
                    model.is_none_or(|m| layer.is_enabled(m)),
                    model.map_or(100.0, |m| layer.stored_opacity(m) * 100.0),
                )
            }
        });

        let sky_pill = self.render_layer_pill(
            "mount_layer_quick_toggles",
            &QuickLayer::SKY_LAYERS,
            scene,
            pill_style,
            cx,
        );
        let grid_pill = self.render_layer_pill(
            "mount_grid_quick_toggles",
            &QuickLayer::GRID_LAYERS,
            scene,
            pill_style,
            cx,
        );
        let limits_pill = self.render_layer_pill(
            "mount_limits_quick_toggles",
            &QuickLayer::MOUNT_LAYERS,
            scene,
            pill_style,
            cx,
        );

        v_flex()
            .id("mount_sky_visibility_controls")
            .absolute()
            .top_3()
            .right_3()
            .items_end()
            .gap_1p5()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                if this.active_opacity_layer.is_some()
                    && !this.opacity_slider.read(cx).is_dragging()
                {
                    this.active_opacity_layer = None;
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(sky_pill)
                    .child(grid_pill)
                    .child(limits_pill),
            )
            .when_some(
                active_popover_state,
                |el, (layer, is_enabled, stored_pct)| {
                    let value_label = if is_enabled {
                        format!("{stored_pct:.0}%")
                    } else {
                        "Off".to_string()
                    };

                    el.child(
                        v_flex()
                            .id("mount_quick_layer_opacity_popover")
                            .w(px(212.0))
                            .px_3()
                            .py_2()
                            .gap_1p5()
                            .rounded_lg()
                            .bg(popover_bg)
                            .border_1()
                            .border_color(popover_border)
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                            .on_mouse_move(|_, _, cx| cx.stop_propagation())
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(
                                                svg()
                                                    .data(layer.icon())
                                                    .size(px(13.0))
                                                    .flex_none()
                                                    .text_color(if is_enabled {
                                                        layer.active_icon_color(text_color)
                                                    } else {
                                                        muted_text
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(text_color)
                                                    .child(layer.title()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_buffer(cx)
                                            .text_size(px(11.5))
                                            .text_color(if is_enabled {
                                                text_color
                                            } else {
                                                muted_text
                                            })
                                            .child(value_label),
                                    ),
                            )
                            .child(self.opacity_slider.clone()),
                    )
                },
            )
            .into_any_element()
    }

    fn render_attitude_hud(
        &self,
        scene: &MountSceneState,
        panel_bg: Hsla,
        border_col: Hsla,
        cx: &App,
    ) -> AnyElement {
        let mount_model = cx
            .try_global::<GlobalDaemonState>()
            .and_then(|g| {
                g.0.read(cx)
                    .state
                    .mount_telemetry
                    .as_ref()
                    .and_then(|t| t.mount_model.clone())
            })
            .unwrap_or_else(|| "Mount".to_string());

        let (ra_str, dec_str) = format_radec(scene.ra_deg, scene.dec_deg);
        let (az_str, alt_str) = format_altaz(scene.az_deg, scene.alt_deg, 10.0);
        let ha_deg = scene.hour_angle_deg();
        let ha_hours = ha_deg / 15.0;
        let flip_margin_deg = scene.limits.meridian_limit_deg - ha_deg;
        let meridian_label = if ha_deg < 0.0 {
            format!("Meridian in {}", format_countdown_deg(-ha_deg))
        } else if flip_margin_deg > 0.0 {
            format!(
                "Past +{:.1}° (flip in {})",
                ha_deg,
                format_countdown_deg(flip_margin_deg)
            )
        } else {
            format!("Past limit ({ha_deg:+.1}°)")
        };

        v_flex()
            .absolute()
            .bottom_3()
            .left_3()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(panel_bg)
            .border_1()
            .border_color(border_col)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Label::new(if scene.is_connected {
                            format!("{mount_model} Attitude")
                        } else {
                            "Last Known Attitude (Offline)".to_string()
                        })
                        .size(LabelSize::Small)
                        .color(if scene.is_connected {
                            Color::Default
                        } else {
                            Color::Muted
                        }),
                    )
                    .child(
                        Label::new(format!("Pier: {}", scene.pier_side.label()))
                            .size(LabelSize::Small)
                            .color(Color::Accent),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .font_buffer(cx)
                    .text_size(px(11.5))
                    .text_color(cx.theme().colors().text)
                    .child(format!("RA {ra_str}"))
                    .child(format!("Dec {dec_str}")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .font_buffer(cx)
                    .text_size(px(11.5))
                    .text_color(cx.theme().colors().text_muted)
                    .child(format!("Az {az_str}"))
                    .child(format!("Alt {}", alt_str.trim()))
                    .child(format!("HA {ha_hours:+.2}h")),
            )
            .child(Label::new(meridian_label).size(LabelSize::XSmall).color(
                if flip_margin_deg <= 0.0 {
                    Color::Warning
                } else {
                    Color::Muted
                },
            ))
            .into_any_element()
    }

    fn render_legend(&self, panel_bg: Hsla, border_col: Hsla) -> AnyElement {
        h_flex()
            .absolute()
            .bottom_3()
            .right_3()
            .gap_2p5()
            .px_2p5()
            .py_1p5()
            .rounded_md()
            .bg(panel_bg)
            .border_1()
            .border_color(border_col)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(legend_chip(rgb(0xf59447).into(), "Min Alt"))
            .child(legend_chip(rgb(0xf25c66).into(), "Zenith / Az"))
            .child(legend_chip(rgb(0x59b8fa).into(), "Meridian"))
            .child(legend_chip(rgb(0x3dd9c7).into(), "Optical Axis"))
            .into_any_element()
    }
}

fn sync_observer_to_bridge(bridge: &MountViewerBridge, cx: &App) {
    let Some(obs_entity) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) else {
        return;
    };
    let obs = obs_entity.read(cx);
    let pos = obs.geodetic_position();
    let mut ctx = ObservationContext {
        location: pos,
        ..ObservationContext::default()
    };
    if let observer::ObserverClock::Manual { unix_seconds } = obs.clock() {
        ctx.set_epoch_unix_seconds(unix_seconds);
    }
    let lst_deg = ctx.local_sidereal_time().degrees();
    let lat_deg = pos.latitude_deg();
    bridge.update_scene(|scene| {
        scene.observer_lat_deg = lat_deg;
        scene.observer_lst_deg = lst_deg;
    });
}

fn sync_daemon_to_bridge(bridge: &MountViewerBridge, cx: &App) {
    let Some(daemon_entity) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) else {
        return;
    };
    let model = daemon_entity.read(cx);
    let is_connected = model.state.connection == ConnectionState::Connected
        && model.state.mount == MountStatus::Ready;
    if let Some(t) = &model.state.mount_telemetry {
        bridge.apply_telemetry(t, is_connected);
    } else if let Some((pointing, is_live)) = model.effective_mount_pointing() {
        bridge.update_scene(|scene| {
            scene.ra_deg = pointing.ra_deg;
            scene.dec_deg = pointing.dec_deg;
            scene.az_deg = pointing.az_deg;
            scene.alt_deg = pointing.alt_deg;
            scene.pier_side = pointing.pier_side;
            scene.is_connected = is_live;
            scene.is_slewing = false;
            scene.target_ra_deg = None;
            scene.target_dec_deg = None;
        });
    } else {
        bridge.update_scene(|scene| {
            scene.is_connected = false;
            scene.is_slewing = false;
        });
    }
}

impl Render for MountViewportPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.bridge.snapshot();
        let (texture_view, texture_size) = match snapshot {
            Some(snap) if snap.is_mount_frame => (snap.texture_view, snap.texture_size),
            _ => (None, None),
        };
        self.cached_texture = texture_view;
        let scale_factor = window.scale_factor().max(0.25);
        let bridge = self.bridge.clone();
        let scene = self.bridge.scene_state();
        let widgets = scene.widgets;
        let bounds_store = self.viewport_bounds.clone();

        let resize_canvas = gpui::canvas(
            move |bounds, window, _cx| {
                if let Ok(mut guard) = bounds_store.lock() {
                    *guard = Some(bounds);
                }
                let scale = window.scale_factor();
                let w = (f32::from(bounds.size.width) * scale).round() as u32;
                let h = (f32::from(bounds.size.height) * scale).round() as u32;
                if w > 0 && h > 0 {
                    bridge.activate_mount_mode_with_size(w, h);
                }
            },
            |_bounds, (), _window, _cx| {},
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        let panel_bg = cx
            .theme()
            .colors()
            .elevated_surface_background
            .opacity(0.90);
        let border_col = cx.theme().colors().border_variant;
        let cursor = if self.dragging_gizmo.is_some() || self.last_drag_pos.is_some() {
            CursorStyle::ClosedHand
        } else if self.hovered_gizmo.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::OpenHand
        };

        let camera_snapshot = self.bridge.camera();
        let gizmo_chip = self
            .dragging_gizmo
            .or(self.hovered_gizmo)
            .and_then(|handle| {
                let bounds = self.viewport_bounds.lock().ok().and_then(|g| *g)?;
                let pos = handle_screen_position(handle, &scene, camera_snapshot, bounds)?;
                let local_x = f32::from(pos.x) - f32::from(bounds.origin.x);
                let local_y = f32::from(pos.y) - f32::from(bounds.origin.y);
                let bw = f32::from(bounds.size.width);
                let bh = f32::from(bounds.size.height);
                let chip_x = (local_x + 12.0).clamp(8.0, (bw - 116.0).max(8.0));
                let chip_y = (local_y - 12.0).clamp(8.0, (bh - 28.0).max(8.0));
                Some((
                    handle,
                    chip_x,
                    chip_y,
                    handle.label_and_value(&scene.limits),
                ))
            });

        let widget_bar = self.render_widget_bar(widgets, panel_bg, border_col, cx);
        let visibility_controls = self.render_visibility_controls(&scene, cx);
        let attitude_hud = self.render_attitude_hud(&scene, panel_bg, border_col, cx);
        let legend = self.render_legend(panel_bg, border_col);

        let cursor_for_gizmos = self.cursor_pos;
        let hovered_for_gizmos = self.hovered_gizmo;
        let dragging_for_gizmos = self.dragging_gizmo;
        let gizmo_canvas = gpui::canvas(
            |_bounds, _window, _cx| {},
            move |bounds, (), window, _cx| {
                paint_axis_gizmos(
                    bounds,
                    cursor_for_gizmos,
                    hovered_for_gizmos,
                    dragging_for_gizmos,
                    &scene,
                    camera_snapshot,
                    window,
                );
            },
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        let viewport = div()
            .id("mount_3d_viewport")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key_down(event, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                this.handle_key_up(event, cx);
            }))
            .size_full()
            .overflow_hidden()
            .relative()
            .cursor(cursor)
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered && this.dragging_gizmo.is_none() {
                    this.cursor_pos = None;
                    this.hovered_gizmo = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.cursor_pos = Some(event.position);
                    let hit =
                        this.viewport_bounds
                            .lock()
                            .ok()
                            .and_then(|g| *g)
                            .and_then(|bounds| {
                                hit_test_handle(
                                    event.position,
                                    &this.bridge.scene_state(),
                                    this.bridge.camera(),
                                    bounds,
                                )
                            });
                    if let Some(handle) = hit {
                        this.dragging_gizmo = Some(handle);
                        this.hovered_gizmo = Some(handle);
                        this.last_drag_pos = None;
                    } else if event.click_count == 2 {
                        this.bridge.set_camera(OrbitCamera::preset_three_quarter());
                    } else {
                        this.last_drag_pos = Some(event.position);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let prev_cursor = this.cursor_pos;
                this.cursor_pos = Some(event.position);
                let bounds = this.viewport_bounds.lock().ok().and_then(|g| *g);

                if let Some(handle) = this.dragging_gizmo {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.finish_gizmo_drag(cx);
                        cx.notify();
                        return;
                    }
                    if let Some(bounds) = bounds {
                        let scene_now = this.bridge.scene_state();
                        let cam_now = this.bridge.camera();
                        if let Some(next_limits) = drag_handle_to_limits(
                            handle,
                            event.position,
                            &scene_now,
                            cam_now,
                            bounds,
                        ) {
                            this.bridge.set_limits_preview(next_limits);
                        }
                    }
                    cx.notify();
                    return;
                }

                if event.pressed_button != Some(MouseButton::Left) {
                    if this.last_drag_pos.is_some() {
                        this.last_drag_pos = None;
                    }
                    let next_hover = bounds.and_then(|b| {
                        hit_test_handle(
                            event.position,
                            &this.bridge.scene_state(),
                            this.bridge.camera(),
                            b,
                        )
                    });
                    let hover_changed = next_hover != this.hovered_gizmo;
                    this.hovered_gizmo = next_hover;
                    // Throttle proximity gizmo redraws to >= 2px cursor delta unless hover state changed.
                    let moved_enough = prev_cursor.is_none_or(|p| {
                        (f32::from(event.position.x - p.x)).hypot(f32::from(event.position.y - p.y))
                            >= 2.0
                    });
                    if hover_changed || moved_enough {
                        cx.notify();
                    }
                    return;
                }
                if let Some(prev) = this.last_drag_pos {
                    let dx = f32::from(event.position.x - prev.x);
                    let dy = f32::from(event.position.y - prev.y);
                    this.bridge.orbit(-dx * 0.0075, dy * 0.0075);
                    this.last_drag_pos = Some(event.position);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.finish_gizmo_drag(cx);
                    this.last_drag_pos = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.finish_gizmo_drag(cx);
                    this.last_drag_pos = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, _cx| {
                let delta_lines = match event.delta {
                    ScrollDelta::Lines(pt) => pt.y,
                    ScrollDelta::Pixels(pt) => f32::from(pt.y) / 24.0,
                };
                if delta_lines.abs() > 1e-4 {
                    let factor = (0.90_f32).powf(delta_lines.clamp(-4.0, 4.0));
                    this.bridge.zoom(factor);
                }
            }))
            .child(resize_canvas)
            .children(self.cached_texture.as_ref().map(|view| {
                let surface = gpui::surface(view.clone());
                match texture_size {
                    Some((tw, th)) => surface
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(tw as f32 / scale_factor))
                        .h(px(th as f32 / scale_factor)),
                    None => surface.size_full(),
                }
            }))
            .child(gizmo_canvas)
            .when_some(gizmo_chip, |el, (handle, x, y, text)| {
                el.child(
                    h_flex()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(panel_bg)
                        .border_1()
                        .border_color(border_col)
                        .shadow_sm()
                        .child(div().size(px(6.0)).rounded_full().bg(handle.accent_color()))
                        .child(
                            div()
                                .font_buffer(cx)
                                .text_size(px(11.0))
                                .text_color(cx.theme().colors().text)
                                .child(text),
                        ),
                )
            });

        div()
            .size_full()
            .bg(rgb(0x0b0e13))
            .relative()
            .child(viewport)
            .child(widget_bar)
            .child(visibility_controls)
            .child(attitude_hud)
            .child(legend)
    }
}

fn format_countdown_deg(deg: f64) -> String {
    let total_secs = (deg / 15.0 * 3600.0).max(0.0).round() as u64;
    if total_secs < 60 {
        format!("{total_secs}s")
    } else {
        format!("{}m", (total_secs + 30) / 60)
    }
}

fn legend_chip(color: Hsla, label: &'static str) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .child(div().size(px(7.0)).rounded_full().bg(color))
        .child(
            Label::new(label)
                .size(LabelSize::XSmall)
                .color(Color::Muted),
        )
}

impl Focusable for MountViewportPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MountViewportPanel {
    fn persistent_name() -> &'static str {
        "mount_viewport_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Mount 3D".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Mount)
    }
}

impl Drop for MountViewportPanel {
    fn drop(&mut self) {
        self.cached_texture = None;
        self.bridge.shutdown();
    }
}
