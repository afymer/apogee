use std::time::{Duration, Instant};

use chrono::Timelike;
use components::{Slider, SliderEvent, StepPrecision, TextField, TextFieldEvent};
use gpui::{
    App, Context, CursorStyle, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, Render, ScrollDelta,
    ScrollWheelEvent, Task, Window, div, prelude::*, px, rgb,
};
use persistence::{
    CoordinateAxis, GeoLocation, ObservatoryIcon, ObservatoryProfile, TimezoneConfig,
    parse_coordinate, parse_elevation_m, parse_utc_offset_minutes,
};
use ui::{
    ActiveTheme, Button, ButtonCommon, ButtonStyle, Clickable, Color, Divider, IconButton,
    IconButtonShape, IconName, IconSize, Label, LabelCommon, LabelSize, StyledTypography, Switch,
    SwitchColor, ToggleState, h_flex, v_flex,
};

use crate::icons::{
    ICON_ELEVATION, ICON_LATITUDE, ICON_LONGITUDE, observatory_icon_svg, render_svg_icon,
};
use crate::sky_time_slider::{SkyTimeSlider, SkyTimeSliderEvent};
use crate::state::GlobalObserver;

struct ProfileEditorState {
    editing_id: Option<String>,
    name_input: Entity<TextField>,
    icon: ObservatoryIcon,
}

/// Expressive observer dropdown popover regrouping observatory profiles,
/// live/scrubbable observation time with a 24-hour solar twilight arc, and
/// geodetic coordinates (latitude, longitude, elevation).
pub struct ObserverPopover {
    focus_handle: FocusHandle,
    sky_slider: Entity<SkyTimeSlider>,
    lat_slider: Entity<Slider>,
    lon_slider: Entity<Slider>,
    elev_slider: Entity<Slider>,
    lat_input: Entity<TextField>,
    lon_input: Entity<TextField>,
    elev_input: Entity<TextField>,
    time_input: Entity<TextField>,
    utc_input: Entity<TextField>,
    utc_editing: bool,
    utc_hovered: bool,
    utc_initial_text: String,
    utc_reset_task: Option<Task<()>>,
    utc_pre_reset: Option<(Instant, TimezoneConfig)>,
    editing_lat: bool,
    editing_lon: bool,
    lat_initial_text: String,
    lon_initial_text: String,
    profile_editor: Option<ProfileEditorState>,
    last_synced_profile_id: String,
}

impl EventEmitter<DismissEvent> for ObserverPopover {}

impl Focusable for ObserverPopover {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ObserverPopover {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let observer_entity = cx.global::<GlobalObserver>().0.clone();

        let (
            profile_id,
            loc,
            hour_of_day,
            is_live,
            solar_profile,
            time_str,
            std_utc_mins,
        ) = {
            let model = observer_entity.read(cx);
            let dt = model.effective_local_datetime();
            (
                model.active_profile().id.clone(),
                model.active_location(),
                model.local_hour_of_day(),
                model.clock().is_live(),
                model.solar_day_profile(97),
                format!("{:02}:{:02}", dt.hour(), dt.minute()),
                model.effective_standard_utc_offset_minutes(),
            )
        };

        let cyan: Hsla = rgb(0x42b3c2).into();
        let sky_slider = cx.new(|_cx| SkyTimeSlider::new(hour_of_day, is_live, solar_profile));

        let lat_slider = cx.new(|_cx| {
            Slider::new(lat_to_slider(loc.latitude_deg))
                .center_mark(0.5)
                .fill_from_center(true)
                .accent_color(cyan)
                .default_value(0.5)
        });
        let lon_slider = cx.new(|_cx| {
            Slider::new(lon_to_slider(loc.longitude_deg))
                .center_mark(0.5)
                .fill_from_center(true)
                .accent_color(cyan)
                .default_value(0.5)
        });
        let elev_slider = cx.new(|_cx| {
            Slider::new(elev_to_slider(loc.elevation_m))
                .accent_color(cyan)
                .default_value(elev_to_slider(0.0))
        });

        let lat_dms = loc.format_latitude_dms();
        let lon_dms = loc.format_longitude_dms();
        let lat_input = cx.new(|cx| {
            TextField::new(lat_dms.clone(), window, cx)
                .placeholder("48°18'30\" N")
                .monospace(true)
        });
        let lon_input = cx.new(|cx| {
            TextField::new(lon_dms.clone(), window, cx)
                .placeholder("2°20'11\" E")
                .monospace(true)
        });
        let elev_input = cx.new(|cx| {
            TextField::new(format!("{:.0}", loc.elevation_m.round()), window, cx)
                .placeholder("0")
                .suffix("m")
                .monospace(true)
        });
        let time_input = cx.new(|cx| {
            TextField::new(time_str, window, cx)
                .placeholder("HH:MM")
                .monospace(true)
        });
        let utc_compact = format_utc_offset_compact(std_utc_mins);
        let utc_input = cx.new(|cx| {
            TextField::new(utc_compact.clone(), window, cx)
                .placeholder("+1 / auto")
                .monospace(true)
        });

        cx.observe_in(&observer_entity, window, |this, _model, window, cx| {
            this.sync_controls_from_model(window, cx);
            cx.notify();
        })
        .detach();

        cx.subscribe(&sky_slider, |_, _, event: &SkyTimeSliderEvent, cx| {
            let SkyTimeSliderEvent::Scrub { hour_of_day } = *event;
            let observer = cx.global::<GlobalObserver>().0.clone();
            observer.update(cx, |model, cx| {
                model.set_local_hour_of_day(hour_of_day, cx);
            });
        })
        .detach();

        Self::wire_location_slider(&lat_slider, CoordinateAxis::Latitude, cx);
        Self::wire_location_slider(&lon_slider, CoordinateAxis::Longitude, cx);
        Self::wire_elevation_slider(&elev_slider, cx);

        Self::wire_coordinate_input(&lat_input, CoordinateAxis::Latitude, window, cx);
        Self::wire_coordinate_input(&lon_input, CoordinateAxis::Longitude, window, cx);
        Self::wire_elevation_input(&elev_input, window, cx);
        Self::wire_time_input(&time_input, window, cx);
        Self::wire_utc_input(&utc_input, window, cx);

        Self {
            focus_handle,
            sky_slider,
            lat_slider,
            lon_slider,
            elev_slider,
            lat_input,
            lon_input,
            elev_input,
            time_input,
            utc_input,
            utc_editing: false,
            utc_hovered: false,
            utc_initial_text: utc_compact,
            utc_reset_task: None,
            utc_pre_reset: None,
            editing_lat: false,
            editing_lon: false,
            lat_initial_text: lat_dms,
            lon_initial_text: lon_dms,
            profile_editor: None,
            last_synced_profile_id: profile_id,
        }
    }

    fn wire_location_slider(
        slider: &Entity<Slider>,
        axis: CoordinateAxis,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(slider, move |_, _, event: &SliderEvent, cx| {
            let observer = cx.global::<GlobalObserver>().0.clone();
            observer.update(cx, |model, cx| {
                let mut loc = model.active_location();
                match *event {
                    SliderEvent::Change(t) => {
                        match axis {
                            CoordinateAxis::Latitude => loc.latitude_deg = slider_to_lat(t),
                            CoordinateAxis::Longitude => loc.longitude_deg = slider_to_lon(t),
                        }
                        model.preview_active_location(loc, cx);
                    }
                    SliderEvent::Commit(t) => {
                        match axis {
                            CoordinateAxis::Latitude => loc.latitude_deg = slider_to_lat(t),
                            CoordinateAxis::Longitude => loc.longitude_deg = slider_to_lon(t),
                        }
                        model.commit_active_location(loc, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn wire_elevation_slider(slider: &Entity<Slider>, cx: &mut Context<Self>) {
        cx.subscribe(slider, |_, _, event: &SliderEvent, cx| {
            let observer = cx.global::<GlobalObserver>().0.clone();
            observer.update(cx, |model, cx| {
                let mut loc = model.active_location();
                match *event {
                    SliderEvent::Change(t) => {
                        loc.elevation_m = slider_to_elev(t);
                        model.preview_active_location(loc, cx);
                    }
                    SliderEvent::Commit(t) => {
                        loc.elevation_m = slider_to_elev(t);
                        model.commit_active_location(loc, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn wire_coordinate_input(
        input: &Entity<TextField>,
        axis: CoordinateAxis,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(input, window, move |this, entity, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(raw) => {
                    if let Some(deg) = parse_coordinate(raw, axis) {
                        match axis {
                            CoordinateAxis::Latitude => this.editing_lat = false,
                            CoordinateAxis::Longitude => this.editing_lon = false,
                        }
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            let mut loc = model.active_location();
                            match axis {
                                CoordinateAxis::Latitude => loc.latitude_deg = deg,
                                CoordinateAxis::Longitude => loc.longitude_deg = deg,
                            }
                            model.commit_active_location(loc, cx);
                        });
                        entity.update(cx, |field, cx| field.set_invalid(false, cx));
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    } else {
                        entity.update(cx, |field, cx| field.set_invalid(true, cx));
                    }
                }
                TextFieldEvent::Blur(raw) => {
                    let (is_editing, initial) = match axis {
                        CoordinateAxis::Latitude => (this.editing_lat, this.lat_initial_text.as_str()),
                        CoordinateAxis::Longitude => (this.editing_lon, this.lon_initial_text.as_str()),
                    };
                    if !is_editing {
                        return;
                    }
                    match axis {
                        CoordinateAxis::Latitude => this.editing_lat = false,
                        CoordinateAxis::Longitude => this.editing_lon = false,
                    }
                    if raw.trim() == initial {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| model.flush_dirty_location(cx));
                    } else if let Some(deg) = parse_coordinate(raw, axis) {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            let mut loc = model.active_location();
                            match axis {
                                CoordinateAxis::Latitude => loc.latitude_deg = deg,
                                CoordinateAxis::Longitude => loc.longitude_deg = deg,
                            }
                            model.commit_active_location(loc, cx);
                        });
                    }
                    entity.update(cx, |field, cx| field.set_invalid(false, cx));
                    cx.notify();
                }
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let observer = cx.global::<GlobalObserver>().0.clone();
                    let current_loc = observer.read(cx).active_location();
                    let fallback = match axis {
                        CoordinateAxis::Latitude => current_loc.latitude_deg,
                        CoordinateAxis::Longitude => current_loc.longitude_deg,
                    };
                    let (next_deg, formatted) = step_coordinate_value(
                        entity.read(cx).text(),
                        fallback,
                        axis,
                        *direction,
                        *precision,
                    );
                    observer.update(cx, |model, cx| {
                        let mut loc = model.active_location();
                        match axis {
                            CoordinateAxis::Latitude => loc.latitude_deg = next_deg,
                            CoordinateAxis::Longitude => loc.longitude_deg = next_deg,
                        }
                        model.preview_active_location(loc, cx);
                    });
                    entity.update(cx, |field, cx| field.set_text(formatted, cx));
                }
                TextFieldEvent::Cancel => {
                    let (is_editing, initial) = match axis {
                        CoordinateAxis::Latitude => (this.editing_lat, this.lat_initial_text.clone()),
                        CoordinateAxis::Longitude => (this.editing_lon, this.lon_initial_text.clone()),
                    };
                    match axis {
                        CoordinateAxis::Latitude => this.editing_lat = false,
                        CoordinateAxis::Longitude => this.editing_lon = false,
                    }
                    if is_editing && let Some(orig_deg) = parse_coordinate(&initial, axis) {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            let mut loc = model.active_location();
                            let current = match axis {
                                CoordinateAxis::Latitude => loc.latitude_deg,
                                CoordinateAxis::Longitude => loc.longitude_deg,
                            };
                            if (current - orig_deg).abs() > 1e-9 {
                                match axis {
                                    CoordinateAxis::Latitude => loc.latitude_deg = orig_deg,
                                    CoordinateAxis::Longitude => loc.longitude_deg = orig_deg,
                                }
                                model.preview_active_location(loc, cx);
                            }
                            model.flush_dirty_location(cx);
                        });
                    }
                    entity.update(cx, |field, cx| field.set_invalid(false, cx));
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }
                TextFieldEvent::Change(_) => {}
            }
        })
        .detach();
    }

    fn wire_elevation_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(input, window, |this, entity, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(raw) | TextFieldEvent::Blur(raw) => {
                    let current_elev = cx
                        .global::<GlobalObserver>()
                        .0
                        .read(cx)
                        .active_location()
                        .elevation_m;
                    if matches!(event, TextFieldEvent::Blur(_))
                        && raw.trim() == format!("{:.0}", current_elev.round())
                    {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| model.flush_dirty_location(cx));
                        return;
                    }
                    if let Some(meters) = parse_elevation_m(raw) {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            let mut loc = model.active_location();
                            loc.elevation_m = meters;
                            model.commit_active_location(loc, cx);
                        });
                        entity.update(cx, |field, cx| {
                            field.set_text(format!("{:.0}", meters.round()), cx);
                        });
                        if matches!(event, TextFieldEvent::Submit(_)) {
                            window.focus(&this.focus_handle, cx);
                        }
                    } else {
                        entity.update(cx, |field, cx| field.set_invalid(true, cx));
                    }
                }
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let observer = cx.global::<GlobalObserver>().0.clone();
                    let current_elev = observer.read(cx).active_location().elevation_m;
                    let next = step_elevation_m(
                        entity.read(cx).text(),
                        current_elev,
                        *direction,
                        *precision,
                    );
                    observer.update(cx, |model, cx| {
                        let mut loc = model.active_location();
                        loc.elevation_m = next;
                        model.preview_active_location(loc, cx);
                    });
                    entity.update(cx, |field, cx| {
                        field.set_text(format!("{:.0}", next.round()), cx);
                    });
                }
                TextFieldEvent::Cancel => {
                    let elev = cx
                        .global::<GlobalObserver>()
                        .0
                        .read(cx)
                        .active_location()
                        .elevation_m;
                    entity.update(cx, |field, cx| {
                        field.set_text(format!("{:.0}", elev.round()), cx);
                    });
                    window.focus(&this.focus_handle, cx);
                }
                TextFieldEvent::Change(_) => {}
            }
        })
        .detach();
    }

    fn wire_time_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(input, window, |this, entity, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(raw) | TextFieldEvent::Blur(raw) => {
                    let current_dt = cx
                        .global::<GlobalObserver>()
                        .0
                        .read(cx)
                        .effective_local_datetime();
                    let current_hm = format!("{:02}:{:02}", current_dt.hour(), current_dt.minute());
                    if matches!(event, TextFieldEvent::Blur(_)) && raw.trim() == current_hm {
                        return;
                    }
                    if let Some((h, m)) = parse_hm_input(raw) {
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            model.set_local_hm(h, m, cx);
                        });
                        entity.update(cx, |field, cx| {
                            field.set_text(format!("{h:02}:{m:02}"), cx);
                        });
                        if matches!(event, TextFieldEvent::Submit(_)) {
                            window.focus(&this.focus_handle, cx);
                        }
                    } else {
                        entity.update(cx, |field, cx| field.set_invalid(true, cx));
                    }
                }
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let observer = cx.global::<GlobalObserver>().0.clone();
                    let (fallback_hm, midnight) = {
                        let model = observer.read(cx);
                        let dt = model.effective_local_datetime();
                        ((dt.hour(), dt.minute()), model.local_midnight_unix_seconds())
                    };
                    let next_total_mins = step_time_total_minutes(
                        entity.read(cx).text(),
                        fallback_hm,
                        *direction,
                        *precision,
                    );
                    let target = midnight + (next_total_mins as f64) * 60.0;
                    observer.update(cx, |model, cx| {
                        model.set_manual_unix_seconds(target, cx);
                    });
                    let dt = observer.read(cx).effective_local_datetime();
                    entity.update(cx, |field, cx| {
                        field.set_text(format!("{:02}:{:02}", dt.hour(), dt.minute()), cx);
                    });
                }
                TextFieldEvent::Cancel => {
                    let dt = cx
                        .global::<GlobalObserver>()
                        .0
                        .read(cx)
                        .effective_local_datetime();
                    entity.update(cx, |field, cx| {
                        field.set_text(format!("{:02}:{:02}", dt.hour(), dt.minute()), cx);
                    });
                    window.focus(&this.focus_handle, cx);
                }
                TextFieldEvent::Change(_) => {}
            }
        })
        .detach();
    }

    fn wire_utc_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(input, window, |this, entity, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(raw) => {
                    if let Some(opt_mins) = parse_utc_offset_minutes(raw) {
                        this.utc_editing = false;
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            model.set_utc_offset_minutes(opt_mins, cx);
                        });
                        entity.update(cx, |field, cx| field.set_invalid(false, cx));
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    } else {
                        entity.update(cx, |field, cx| field.set_invalid(true, cx));
                    }
                }
                TextFieldEvent::Blur(raw) => {
                    if !this.utc_editing {
                        return;
                    }
                    this.utc_editing = false;
                    if raw.trim() != this.utc_initial_text {
                        if let Some(opt_mins) = parse_utc_offset_minutes(raw) {
                            let observer = cx.global::<GlobalObserver>().0.clone();
                            observer.update(cx, |model, cx| {
                                model.set_utc_offset_minutes(opt_mins, cx);
                            });
                        }
                    }
                    entity.update(cx, |field, cx| field.set_invalid(false, cx));
                    cx.notify();
                }
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let observer = cx.global::<GlobalObserver>().0.clone();
                    let fallback_mins =
                        observer.read(cx).effective_standard_utc_offset_minutes();
                    let next_mins = step_utc_offset_mins(
                        entity.read(cx).text(),
                        fallback_mins,
                        *direction,
                        *precision,
                    );
                    observer.update(cx, |model, cx| {
                        model.set_utc_offset_minutes(Some(next_mins), cx);
                    });
                    entity.update(cx, |field, cx| {
                        field.set_text(format_utc_offset_compact(next_mins), cx);
                    });
                }
                TextFieldEvent::Cancel => {
                    this.utc_editing = false;
                    entity.update(cx, |field, cx| field.set_invalid(false, cx));
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }
                TextFieldEvent::Change(_) => {}
            }
        })
        .detach();
    }

    fn begin_coordinate_edit(
        &mut self,
        axis: CoordinateAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loc = cx.global::<GlobalObserver>().0.read(cx).active_location();
        match axis {
            CoordinateAxis::Latitude => {
                let text = loc.format_latitude_dms();
                self.lat_initial_text.clone_from(&text);
                self.editing_lat = true;
                self.editing_lon = false;
                self.lat_input.update(cx, |field, cx| {
                    field.set_text(text, cx);
                    field.focus(window, cx);
                });
            }
            CoordinateAxis::Longitude => {
                let text = loc.format_longitude_dms();
                self.lon_initial_text.clone_from(&text);
                self.editing_lon = true;
                self.editing_lat = false;
                self.lon_input.update(cx, |field, cx| {
                    field.set_text(text, cx);
                    field.focus(window, cx);
                });
            }
        }
        cx.notify();
    }

    fn begin_utc_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.utc_reset_task = None;
        self.utc_pre_reset = None;
        let mins = cx
            .global::<GlobalObserver>()
            .0
            .read(cx)
            .effective_standard_utc_offset_minutes();
        let compact = format_utc_offset_compact(mins);
        self.utc_initial_text.clone_from(&compact);
        self.utc_editing = true;
        self.utc_hovered = false;
        self.utc_input.update(cx, |field, cx| {
            field.set_text(compact, cx);
            field.focus(window, cx);
        });
        cx.notify();
    }

    fn sync_controls_from_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let observer = cx.global::<GlobalObserver>().0.clone();
        let (profile_id, loc, hour_of_day, is_live, solar_profile, time_str, std_utc_mins) = {
            let model = observer.read(cx);
            let dt = model.effective_local_datetime();
            (
                model.active_profile().id.clone(),
                model.active_location(),
                model.local_hour_of_day(),
                model.clock().is_live(),
                model.solar_day_profile(97),
                format!("{:02}:{:02}", dt.hour(), dt.minute()),
                model.effective_standard_utc_offset_minutes(),
            )
        };

        let profile_switched = self.last_synced_profile_id != profile_id;
        self.last_synced_profile_id = profile_id;
        if profile_switched {
            self.editing_lat = false;
            self.editing_lon = false;
            self.utc_editing = false;
            self.utc_reset_task = None;
            self.utc_pre_reset = None;
        }

        self.sky_slider.update(cx, |slider, cx| {
            slider.sync(hour_of_day, is_live, solar_profile, cx);
        });
        self.lat_slider.update(cx, |slider, cx| {
            slider.set_value(lat_to_slider(loc.latitude_deg), cx);
        });
        self.lon_slider.update(cx, |slider, cx| {
            slider.set_value(lon_to_slider(loc.longitude_deg), cx);
        });
        self.elev_slider.update(cx, |slider, cx| {
            slider.set_value(elev_to_slider(loc.elevation_m), cx);
        });

        self.elev_input.update(cx, |field, cx| {
            if profile_switched || !field.is_focused(window) {
                field.set_text(format!("{:.0}", loc.elevation_m.round()), cx);
            }
        });
        self.time_input.update(cx, |field, cx| {
            if !field.is_focused(window) {
                field.set_text(time_str, cx);
            }
        });
        self.utc_input.update(cx, |field, cx| {
            if !self.utc_editing && !field.is_focused(window) {
                field.set_text(format_utc_offset_compact(std_utc_mins), cx);
            }
        });
    }

    fn open_new_profile_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx.new(|cx| {
            let mut field = TextField::new("", window, cx).placeholder("e.g. Dark Sky Meadow");
            field.focus(window, cx);
            field
        });
        self.wire_editor_name_input(&name_input, window, cx);
        self.profile_editor = Some(ProfileEditorState {
            editing_id: None,
            name_input,
            icon: ObservatoryIcon::Field,
        });
        cx.notify();
    }

    fn toggle_edit_active_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .profile_editor
            .as_ref()
            .is_some_and(|ed| ed.editing_id.is_some())
        {
            self.profile_editor = None;
            cx.notify();
            return;
        }

        let active = cx
            .global::<GlobalObserver>()
            .0
            .read(cx)
            .active_profile()
            .clone();
        let name_input = cx.new(|cx| {
            let mut field = TextField::new(active.name.clone(), window, cx).placeholder("Site name");
            field.focus(window, cx);
            field
        });
        self.wire_editor_name_input(&name_input, window, cx);
        self.profile_editor = Some(ProfileEditorState {
            editing_id: Some(active.id),
            name_input,
            icon: active.icon,
        });
        cx.notify();
    }

    fn wire_editor_name_input(
        &self,
        input: &Entity<TextField>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(input, window, |this, _, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(_) => {
                    this.save_profile_editor(cx);
                    window.focus(&this.focus_handle, cx);
                }
                TextFieldEvent::Cancel => {
                    this.profile_editor = None;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }
                TextFieldEvent::Change(_)
                | TextFieldEvent::Blur(_)
                | TextFieldEvent::Step { .. } => {}
            }
        })
        .detach();
    }

    fn save_profile_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.profile_editor.take() else {
            return;
        };
        let raw_name = editor.name_input.read(cx).text().trim().to_string();
        let icon = editor.icon;
        let observer = cx.global::<GlobalObserver>().0.clone();

        observer.update(cx, |model, cx| {
            if let Some(id) = editor.editing_id {
                let fallback = model
                    .catalog()
                    .profile(&id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Observatory".to_string());
                let name = if raw_name.is_empty() { fallback } else { raw_name };
                model.update_profile_meta(&id, &name, icon, cx);
            } else {
                let name = if raw_name.is_empty() {
                    format!("{} Site", icon.label())
                } else {
                    raw_name
                };
                let loc = model.active_location();
                model.create_profile(&name, icon, loc, cx);
            }
        });
        cx.notify();
    }

    fn delete_edited_profile(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .profile_editor
            .as_ref()
            .and_then(|ed| ed.editing_id.clone())
        else {
            return;
        };
        let observer = cx.global::<GlobalObserver>().0.clone();
        let deleted = observer.update(cx, |model, cx| model.delete_profile(&id, cx));
        if deleted {
            self.profile_editor = None;
            cx.notify();
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.utc_reset_task = None;
        self.utc_pre_reset = None;
        let observer = cx.global::<GlobalObserver>().0.clone();
        observer.update(cx, |model, cx| model.flush_dirty_location(cx));
        cx.emit(DismissEvent);
    }

    fn render_hero_band(
        &self,
        active: &ObservatoryProfile,
        solar_alt_deg: f64,
        lst_text: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let is_editing_active = self
            .profile_editor
            .as_ref()
            .is_some_and(|ed| ed.editing_id.as_deref() == Some(active.id.as_str()));

        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(14.0))
                                    .text_color(colors.text)
                                    .child(active.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(if active.source.is_remote() {
                                        cyan.opacity(0.45)
                                    } else {
                                        colors.border_variant
                                    })
                                    .bg(if active.source.is_remote() {
                                        cyan.opacity(0.14)
                                    } else {
                                        colors.element_background.opacity(0.6)
                                    })
                                    .text_size(px(9.5))
                                    .text_color(if active.source.is_remote() {
                                        rgb(0x7be0ec).into()
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(if active.source.is_remote() {
                                        "Remote"
                                    } else {
                                        "Local"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .font_buffer(cx)
                            .text_size(px(11.0))
                            .text_color(colors.text_muted)
                            .truncate()
                            .child(active.location.format_summary_dms()),
                    )
                    .child(
                        div()
                            .font_buffer(cx)
                            .text_size(px(10.5))
                            .text_color(colors.text_muted.opacity(0.85))
                            .child(format!("LST {lst_text} · Sun {solar_alt_deg:+.1}°")),
                    ),
            )
            .child(
                IconButton::new("edit_active_site_btn", IconName::Pencil)
                    .shape(IconButtonShape::Square)
                    .icon_size(IconSize::Small)
                    .style(if is_editing_active {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.toggle_edit_active_profile(window, cx);
                    })),
            )
    }

    fn render_profile_strip(
        &self,
        profiles: &[ObservatoryProfile],
        active_id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();

        let mut row = h_flex().w_full().gap_1p5().flex_wrap().items_stretch();
        for profile in profiles {
            let is_active = profile.id == active_id;
            let pid = profile.id.clone();
            let card_bg = if is_active {
                cyan.opacity(0.14)
            } else {
                colors.surface_background.opacity(0.75)
            };
            let card_border = if is_active {
                cyan.opacity(0.70)
            } else {
                colors.border_variant
            };
            let icon_color = if is_active {
                rgb(0x7be0ec).into()
            } else {
                colors.text_muted
            };

            row = row.child(
                h_flex()
                    .id(format!("obs_profile_card_{pid}"))
                    .h(px(44.0))
                    .items_center()
                    .gap_1p5()
                    .px_2p5()
                    .rounded_lg()
                    .bg(card_bg)
                    .border_1()
                    .border_color(card_border)
                    .cursor_pointer()
                    .hover(|s| {
                        s.border_color(if is_active {
                            cyan
                        } else {
                            colors.border_focused
                        })
                        .bg(if is_active {
                            cyan.opacity(0.18)
                        } else {
                            colors.element_hover
                        })
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.profile_editor = None;
                        this.editing_lat = false;
                        this.editing_lon = false;
                        this.utc_editing = false;
                        this.utc_reset_task = None;
                        this.utc_pre_reset = None;
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| model.select_profile(&pid, cx));
                    }))
                    .child(render_svg_icon(
                        observatory_icon_svg(profile.icon),
                        px(18.0),
                        icon_color,
                    ))
                    .child(
                        v_flex()
                            .gap_0()
                            .child(
                                div()
                                    .max_w(px(88.0))
                                    .truncate()
                                    .text_size(px(11.5))
                                    .font_weight(if is_active {
                                        gpui::FontWeight::MEDIUM
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .text_color(if is_active {
                                        colors.text
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(profile.name.clone()),
                            )
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(9.5))
                                    .text_color(colors.text_muted.opacity(0.8))
                                    .child(format!("{:.0} m", profile.location.elevation_m.round())),
                            ),
                    ),
            );
        }

        let is_creating = self
            .profile_editor
            .as_ref()
            .is_some_and(|ed| ed.editing_id.is_none());

        row.child(
            h_flex()
                .id("obs_profile_new_btn")
                .h(px(44.0))
                .items_center()
                .justify_center()
                .gap_1()
                .px_3()
                .rounded_lg()
                .border_1()
                .border_color(if is_creating {
                    cyan.opacity(0.75)
                } else {
                    colors.border_variant
                })
                .bg(if is_creating {
                    cyan.opacity(0.14)
                } else {
                    colors.ghost_element_background
                })
                .cursor_pointer()
                .hover(|s| s.bg(colors.element_hover).border_color(colors.border_focused))
                .on_click(cx.listener(|this, _event, window, cx| {
                    if this
                        .profile_editor
                        .as_ref()
                        .is_some_and(|ed| ed.editing_id.is_none())
                    {
                        this.profile_editor = None;
                        cx.notify();
                    } else {
                        this.open_new_profile_editor(window, cx);
                    }
                }))
                .child(
                    Label::new("+ New")
                        .size(LabelSize::Small)
                        .color(if is_creating {
                            Color::Accent
                        } else {
                            Color::Muted
                        }),
                ),
        )
    }

    fn render_profile_editor(
        &self,
        editor: &ProfileEditorState,
        can_delete: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let is_edit = editor.editing_id.is_some();

        let mut icon_grid = h_flex().w_full().gap_1();
        for icon in ObservatoryIcon::ALL {
            let selected = editor.icon == icon;
            icon_grid = icon_grid.child(
                v_flex()
                    .id(format!("icon_pick_{}", icon.label()))
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_0p5()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected {
                        cyan.opacity(0.18)
                    } else {
                        colors.element_background.opacity(0.45)
                    })
                    .border_1()
                    .border_color(if selected {
                        cyan.opacity(0.80)
                    } else {
                        colors.border_variant
                    })
                    .hover(|s| s.bg(colors.element_hover))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if let Some(ed) = this.profile_editor.as_mut() {
                            ed.icon = icon;
                            cx.notify();
                        }
                    }))
                    .child(render_svg_icon(
                        observatory_icon_svg(icon),
                        px(20.0),
                        if selected {
                            rgb(0x7be0ec).into()
                        } else {
                            colors.text_muted
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(if selected {
                                colors.text
                            } else {
                                colors.text_muted
                            })
                            .child(icon.label()),
                    ),
            );
        }

        v_flex()
            .w_full()
            .p_2()
            .gap_2()
            .rounded_lg()
            .bg(colors.surface_background.opacity(0.85))
            .border_1()
            .border_color(cyan.opacity(0.35))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child(if is_edit {
                        "Edit Observatory Profile"
                    } else {
                        "Create Observatory Profile (copies current coordinates)"
                    }),
            )
            .child(editor.name_input.clone())
            .child(icon_grid)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div().when(is_edit && can_delete, |el| {
                            el.child(
                                Button::new("delete_profile_btn", "Delete")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .color(Color::Error)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.delete_edited_profile(cx);
                                    })),
                            )
                        }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("cancel_profile_editor_btn", "Cancel")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.profile_editor = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(
                                    "save_profile_editor_btn",
                                    if is_edit { "Save" } else { "Create Site" },
                                )
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.save_profile_editor(cx);
                                })),
                            ),
                    ),
            )
    }

    fn render_time_band(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let utc_step_buttons = if !self.utc_editing && self.utc_hovered {
            Some((
                self.utc_step_button("utc_step_dec", "−", -1, cx),
                self.utc_step_button("utc_step_inc", "+", 1, cx),
            ))
        } else {
            None
        };
        let colors = cx.theme().colors();
        let status = cx.theme().status();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let (is_live, dt, events, std_offset_mins, dst_enabled) = {
            let observer = cx.global::<GlobalObserver>().0.read(cx);
            (
                observer.clock().is_live(),
                observer.effective_local_datetime(),
                observer.daily_solar_events(),
                observer.effective_standard_utc_offset_minutes(),
                observer.effective_dst_enabled(),
            )
        };
        let utc_badge = format_utc_offset(std_offset_mins * 60);
        let date_label = dt.format("%b %d, %Y").to_string();
        let seconds_str = format!(":{:02}", dt.second());

        let (dusk_label, dusk_hour) = if let Some(h) = events.astro_dusk_hour {
            ("Astro Dusk", h)
        } else if let Some(h) = events.sunset_hour {
            ("Sunset", h)
        } else {
            ("Deepest", events.solar_midnight_hour)
        };

        let (dawn_label, dawn_hour) = if let Some(h) = events.astro_dawn_hour {
            ("Astro Dawn", h)
        } else if let Some(h) = events.sunrise_hour {
            ("Sunrise", h)
        } else {
            ("Noon", events.solar_noon_hour)
        };

        let utc_control = if self.utc_editing {
            div().w(px(58.0)).child(self.utc_input.clone()).into_any_element()
        } else {
            let (utc_dec, utc_inc) = match utc_step_buttons {
                Some((dec, inc)) => (Some(dec), Some(inc)),
                None => (None, None),
            };
            h_flex()
                .id("utc_offset_pill")
                .h(px(22.0))
                .items_center()
                .gap_0p5()
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .bg(if self.utc_hovered {
                    colors.element_hover
                } else {
                    colors.element_background.opacity(0.65)
                })
                .border_1()
                .border_color(if self.utc_hovered {
                    colors.border
                } else {
                    colors.border_variant
                })
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if this.utc_hovered != *hovered {
                        this.utc_hovered = *hovered;
                        cx.notify();
                    }
                }))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                    this.utc_reset_task = None;
                    this.utc_pre_reset = None;
                    let dy = match event.delta {
                        ScrollDelta::Lines(pt) => pt.y,
                        ScrollDelta::Pixels(pt) => f32::from(pt.y),
                    };
                    if dy.abs() > 1e-4 {
                        let step = if event.modifiers.shift { 30 } else { 60 };
                        let delta = if dy > 0.0 { step } else { -step };
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            model.step_utc_offset_minutes(delta, cx);
                        });
                    }
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.utc_reset_task = None;
                        this.utc_pre_reset = None;
                        let observer = cx.global::<GlobalObserver>().0.clone();
                        observer.update(cx, |model, cx| {
                            model.set_utc_offset_minutes(None, cx);
                        });
                        cx.stop_propagation();
                    }),
                )
                .when_some(utc_dec, |el, btn| el.child(btn))
                .child(
                    div()
                        .id("utc_edit_trigger")
                        .font_buffer(cx)
                        .text_size(px(10.0))
                        .text_color(if self.utc_hovered {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                            if event.click_count() >= 2 {
                                this.utc_reset_task = None;
                                if let Some((at, prev_tz)) = this.utc_pre_reset.take()
                                    && at.elapsed() < Duration::from_millis(450)
                                {
                                    let observer = cx.global::<GlobalObserver>().0.clone();
                                    observer.update(cx, |model, cx| {
                                        model.restore_timezone_config(prev_tz, cx);
                                    });
                                }
                                this.begin_utc_edit(window, cx);
                            } else {
                                let observer = cx.global::<GlobalObserver>().0.clone();
                                let prev_tz = observer.read(cx).active_profile().timezone;
                                this.utc_reset_task = Some(cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(220))
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.utc_reset_task = None;
                                        this.utc_pre_reset = Some((Instant::now(), prev_tz));
                                        let observer = cx.global::<GlobalObserver>().0.clone();
                                        observer.update(cx, |model, cx| {
                                            model.set_utc_offset_minutes(None, cx);
                                        });
                                    });
                                }));
                            }
                        }))
                        .child(utc_badge),
                )
                .when_some(utc_inc, |el, btn| el.child(btn))
                .into_any_element()
        };

        let dst_pill = h_flex()
            .id("dst_toggle_pill")
            .h(px(22.0))
            .items_center()
            .gap_1()
            .px_1p5()
            .rounded_sm()
            .cursor_pointer()
            .bg(if dst_enabled {
                cyan.opacity(0.16)
            } else {
                colors.element_background.opacity(0.45)
            })
            .border_1()
            .border_color(if dst_enabled {
                cyan.opacity(0.55)
            } else {
                colors.border_variant
            })
            .hover(|s| {
                s.bg(if dst_enabled {
                    cyan.opacity(0.24)
                } else {
                    colors.element_hover
                })
            })
            .on_click(|_, _, cx| {
                let observer = cx.global::<GlobalObserver>().0.clone();
                observer.update(cx, |model, cx| model.toggle_dst(cx));
            })
            .on_mouse_down(MouseButton::Right, |_, _, cx| {
                let observer = cx.global::<GlobalObserver>().0.clone();
                observer.update(cx, |model, cx| model.reset_dst_auto(cx));
                cx.stop_propagation();
            })
            .child(div().size(px(5.0)).rounded_full().bg(if dst_enabled {
                rgb(0x7be0ec).into()
            } else {
                colors.text_muted.opacity(0.45)
            }))
            .child(
                div()
                    .font_buffer(cx)
                    .text_size(px(10.0))
                    .text_color(if dst_enabled {
                        rgb(0x7be0ec).into()
                    } else {
                        colors.text_muted
                    })
                    .child("DST"),
            );

        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(section_eyebrow("OBSERVATION TIME", cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div().size(px(6.0)).rounded_full().bg(if is_live {
                                            status.success
                                        } else {
                                            status.warning
                                        }),
                                    )
                                    .child(
                                        Label::new(if is_live { "Now" } else { "Custom" })
                                            .size(LabelSize::XSmall)
                                            .color(if is_live {
                                                Color::Default
                                            } else {
                                                Color::Warning
                                            }),
                                    ),
                            )
                            .child(
                                Switch::new(
                                    "observer_auto_sync_switch",
                                    if is_live {
                                        ToggleState::Selected
                                    } else {
                                        ToggleState::Unselected
                                    },
                                )
                                .color(SwitchColor::Accent)
                                .on_click(|state, _window, cx| {
                                    let observer = cx.global::<GlobalObserver>().0.clone();
                                    observer.update(cx, |model, cx| {
                                        if *state == ToggleState::Selected {
                                            model.set_live(cx);
                                        } else {
                                            let now_secs = model.effective_unix_seconds();
                                            model.set_manual_unix_seconds(now_secs, cx);
                                        }
                                    });
                                }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().w(px(58.0)).child(self.time_input.clone()))
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(12.0))
                                    .text_color(colors.text_muted)
                                    .child(seconds_str),
                            )
                            .child(utc_control)
                            .child(dst_pill),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_0p5()
                            .child(
                                IconButton::new("prev_day_btn", IconName::ChevronLeft)
                                    .shape(IconButtonShape::Square)
                                    .icon_size(IconSize::XSmall)
                                    .style(ButtonStyle::Subtle)
                                    .on_click(|_, _, cx| {
                                        let observer = cx.global::<GlobalObserver>().0.clone();
                                        observer.update(cx, |model, cx| model.step_days(-1, cx));
                                    }),
                            )
                            .child(
                                Button::new("jump_today_btn", date_label)
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(|_, _, cx| {
                                        let observer = cx.global::<GlobalObserver>().0.clone();
                                        observer.update(cx, |model, cx| {
                                            model.jump_to_today_keep_time(cx);
                                        });
                                    }),
                            )
                            .child(
                                IconButton::new("next_day_btn", IconName::ChevronRight)
                                    .shape(IconButtonShape::Square)
                                    .icon_size(IconSize::XSmall)
                                    .style(ButtonStyle::Subtle)
                                    .on_click(|_, _, cx| {
                                        let observer = cx.global::<GlobalObserver>().0.clone();
                                        observer.update(cx, |model, cx| model.step_days(1, cx));
                                    }),
                            ),
                    ),
            )
            .child(self.sky_slider.clone())
            .when(!is_live, |el| {
                el.child(
                    h_flex()
                        .w_full()
                        .gap_1p5()
                        .child(quick_time_pill(
                            "jump_dusk",
                            dusk_label,
                            format_hour_hm(dusk_hour),
                            cx,
                            move |_, _, cx| {
                                let observer = cx.global::<GlobalObserver>().0.clone();
                                observer.update(cx, |model, cx| {
                                    model.set_local_hour_of_day(dusk_hour, cx);
                                });
                            },
                        ))
                        .child(quick_time_pill(
                            "jump_midnight",
                            "Midnight",
                            "00:00",
                            cx,
                            |_, _, cx| {
                                let observer = cx.global::<GlobalObserver>().0.clone();
                                observer.update(cx, |model, cx| model.set_local_hm(0, 0, cx));
                            },
                        ))
                        .child(quick_time_pill(
                            "jump_dawn",
                            dawn_label,
                            format_hour_hm(dawn_hour),
                            cx,
                            move |_, _, cx| {
                                let observer = cx.global::<GlobalObserver>().0.clone();
                                observer.update(cx, |model, cx| {
                                    model.set_local_hour_of_day(dawn_hour, cx);
                                });
                            },
                        )),
                )
            })
    }

    fn utc_step_button(
        &self,
        id: &'static str,
        label: &'static str,
        dir: i32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.theme().colors();
        div()
            .id(id)
            .px_1()
            .rounded_xs()
            .cursor_pointer()
            .hover(|s| s.bg(colors.element_background))
            .font_buffer(cx)
            .text_size(px(10.5))
            .text_color(colors.text_muted)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.utc_reset_task = None;
                this.utc_pre_reset = None;
                let magnitude = if event.modifiers().shift { 30 } else { 60 };
                let step = dir * magnitude;
                let observer = cx.global::<GlobalObserver>().0.clone();
                observer.update(cx, |model, cx| {
                    model.step_utc_offset_minutes(step, cx);
                });
            }))
            .child(label)
    }

    fn render_coordinates_band(&self, loc: GeoLocation, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();

        v_flex()
            .w_full()
            .gap_2()
            .child(section_eyebrow("SITE COORDINATES", cx))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(coordinate_card(
                        "lat_card",
                        ICON_LATITUDE,
                        "LATITUDE",
                        loc.format_latitude_dms(),
                        self.editing_lat,
                        self.lat_input.clone(),
                        self.lat_slider.clone(),
                        cx,
                        cx.listener(|this, _event, window, cx| {
                            this.begin_coordinate_edit(CoordinateAxis::Latitude, window, cx);
                        }),
                    ))
                    .child(coordinate_card(
                        "lon_card",
                        ICON_LONGITUDE,
                        "LONGITUDE",
                        loc.format_longitude_dms(),
                        self.editing_lon,
                        self.lon_input.clone(),
                        self.lon_slider.clone(),
                        cx,
                        cx.listener(|this, _event, window, cx| {
                            this.begin_coordinate_edit(CoordinateAxis::Longitude, window, cx);
                        }),
                    )),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_1p5()
                    .rounded_lg()
                    .bg(colors.surface_background.opacity(0.65))
                    .border_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(render_svg_icon(
                                        ICON_ELEVATION,
                                        px(15.0),
                                        colors.text_muted,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(colors.text_muted)
                                            .child("ELEVATION"),
                                    ),
                            )
                            .child(div().w(px(96.0)).child(self.elev_input.clone())),
                    )
                    .child(self.elev_slider.clone()),
            )
    }
}

impl Render for ObserverPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (active, profiles, solar_alt, lst_text) = {
            let model = cx.global::<GlobalObserver>().0.read(cx);
            let ctx = model.observation_context();
            (
                model.active_profile().clone(),
                model.catalog().profiles.clone(),
                model.solar_altitude_deg(),
                ctx.format_lst_hms(),
            )
        };
        let can_delete = profiles.len() > 1;

        v_flex()
            .id("observer_popover_root")
            .track_focus(&self.focus_handle)
            .w(px(404.0))
            .p_3()
            .gap_3()
            .rounded_xl()
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.dismiss(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" {
                    if this.utc_editing {
                        this.utc_editing = false;
                        this.utc_input.update(cx, |f, cx| f.set_invalid(false, cx));
                        cx.notify();
                    } else if this.editing_lat {
                        this.editing_lat = false;
                        this.lat_input.update(cx, |f, cx| f.set_invalid(false, cx));
                        cx.notify();
                    } else if this.editing_lon {
                        this.editing_lon = false;
                        this.lon_input.update(cx, |f, cx| f.set_invalid(false, cx));
                        cx.notify();
                    } else if this.profile_editor.is_some() {
                        this.profile_editor = None;
                        cx.notify();
                    } else {
                        this.dismiss(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(self.render_hero_band(&active, solar_alt, &lst_text, cx))
            .child(self.render_profile_strip(&profiles, &active.id, cx))
            .when_some(self.profile_editor.as_ref(), |el, editor| {
                el.child(self.render_profile_editor(editor, can_delete, cx))
            })
            .child(Divider::horizontal())
            .child(self.render_time_band(cx))
            .child(Divider::horizontal())
            .child(self.render_coordinates_band(active.location, cx))
    }
}

fn section_eyebrow(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().colors().text_muted)
        .child(title)
}

fn quick_time_pill(
    id: &'static str,
    title: &'static str,
    subtitle: impl Into<String>,
    cx: &App,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let subtitle = subtitle.into();

    v_flex()
        .id(id)
        .flex_1()
        .items_center()
        .justify_center()
        .py_1()
        .px_1p5()
        .rounded_md()
        .cursor_pointer()
        .bg(colors.element_background.opacity(0.45))
        .border_1()
        .border_color(colors.border_variant)
        .hover(|s| s.bg(colors.element_hover))
        .on_click(on_click)
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child(title),
        )
        .child(
            div()
                .font_buffer(cx)
                .text_size(px(9.5))
                .text_color(colors.text_muted.opacity(0.8))
                .child(subtitle),
        )
}

#[allow(clippy::too_many_arguments)]
fn coordinate_card(
    id: &'static str,
    icon: &'static [u8],
    label: &'static str,
    dms_readout: String,
    is_editing: bool,
    input: Entity<TextField>,
    slider: Entity<Slider>,
    cx: &App,
    on_start_edit: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = cx.theme().colors();

    let value_slot = if is_editing {
        div().h(px(26.0)).child(input).into_any_element()
    } else {
        h_flex()
            .id(format!("{id}_value_edit"))
            .w_full()
            .h(px(26.0))
            .px_2()
            .items_center()
            .rounded_md()
            .bg(colors.element_background.opacity(0.35))
            .border_1()
            .border_color(colors.border_variant.opacity(0.55))
            .cursor(CursorStyle::IBeam)
            .hover(|s| {
                s.bg(colors.element_background.opacity(0.6))
                    .border_color(colors.border_focused)
            })
            .on_click(on_start_edit)
            .font_buffer(cx)
            .text_size(px(12.0))
            .text_color(colors.text_muted)
            .child(div().truncate().child(dms_readout))
            .into_any_element()
    };

    v_flex()
        .id(id)
        .flex_1()
        .min_w_0()
        .p_2()
        .gap_1p5()
        .rounded_lg()
        .bg(colors.surface_background.opacity(0.65))
        .border_1()
        .border_color(colors.border_variant)
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .child(render_svg_icon(icon, px(15.0), colors.text_muted))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors.text_muted)
                        .child(label),
                ),
        )
        .child(value_slot)
        .child(slider)
}

pub(crate) fn lat_to_slider(lat: f64) -> f32 {
    ((lat + 90.0) / 180.0).clamp(0.0, 1.0) as f32
}

pub(crate) fn slider_to_lat(t: f32) -> f64 {
    (f64::from(t) * 180.0 - 90.0).clamp(-90.0, 90.0)
}

pub(crate) fn lon_to_slider(lon: f64) -> f32 {
    ((lon + 180.0) / 360.0).clamp(0.0, 1.0) as f32
}

pub(crate) fn slider_to_lon(t: f32) -> f64 {
    (f64::from(t) * 360.0 - 180.0).clamp(-180.0, 180.0)
}

pub(crate) fn elev_to_slider(meters: f64) -> f32 {
    ((meters + 100.0).clamp(0.0, 5100.0) / 5100.0).sqrt() as f32
}

pub(crate) fn slider_to_elev(t: f32) -> f64 {
    let clamped = f64::from(t).clamp(0.0, 1.0);
    (-100.0 + 5100.0 * clamped * clamped)
        .round()
        .clamp(GeoLocation::MIN_ELEV_M, GeoLocation::MAX_ELEV_M)
}

pub(crate) fn format_hour_hm(hour_f: f64) -> String {
    let total_mins = (hour_f.rem_euclid(24.0) * 60.0).round() as u32 % 1440;
    format!("{:02}:{:02}", total_mins / 60, total_mins % 60)
}

pub(crate) fn parse_hm_input(input: &str) -> Option<(u32, u32)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized: String = trimmed
        .chars()
        .map(|c| match c {
            ':' | 'h' | 'H' => ' ',
            _ => c,
        })
        .collect();
    let parts: Vec<&str> = normalized.split_whitespace().collect();

    let (h, m) = match parts.as_slice() {
        [single] if single.chars().all(|c| c.is_ascii_digit()) => match single.len() {
            1..=2 => (single.parse::<u32>().ok()?, 0),
            3 => (single[..1].parse::<u32>().ok()?, single[1..].parse::<u32>().ok()?),
            4 => (single[..2].parse::<u32>().ok()?, single[2..].parse::<u32>().ok()?),
            _ => return None,
        },
        [hh, mm] => (hh.parse::<u32>().ok()?, mm.parse::<u32>().ok()?),
        _ => return None,
    };

    if h < 24 && m < 60 { Some((h, m)) } else { None }
}

fn format_utc_offset(offset_secs: i32) -> String {
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs_mins = offset_secs.abs() / 60;
    let h = abs_mins / 60;
    let m = abs_mins % 60;
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

fn format_utc_offset_compact(offset_mins: i32) -> String {
    let sign = if offset_mins >= 0 { '+' } else { '-' };
    let abs_mins = offset_mins.abs();
    let h = abs_mins / 60;
    let m = abs_mins % 60;
    if m == 0 {
        format!("{sign}{h}")
    } else {
        format!("{sign}{h}:{m:02}")
    }
}

pub(crate) fn step_elevation_m(
    raw: &str,
    fallback_m: f64,
    direction: i32,
    precision: StepPrecision,
) -> f64 {
    let base = parse_elevation_m(raw).unwrap_or(fallback_m);
    let delta = f64::from(direction) * (precision.integer_step() as f64);
    (base + delta).clamp(GeoLocation::MIN_ELEV_M, GeoLocation::MAX_ELEV_M)
}

pub(crate) fn step_time_total_minutes(
    raw: &str,
    fallback_hm: (u32, u32),
    direction: i32,
    precision: StepPrecision,
) -> i64 {
    let (h, m) = parse_hm_input(raw).unwrap_or(fallback_hm);
    let current_mins = i64::from(h) * 60 + i64::from(m);
    let step_mins = i64::from(direction) * precision.choose(1_i64, 10, 60);
    current_mins + step_mins
}

fn is_dms_input(raw: &str) -> bool {
    let has_marker = raw.chars().any(|c| {
        matches!(
            c,
            '°' | '\'' | '"' | '′' | '″' | 'd' | 'D' | 'm' | 'M' | 's' | 'S' | 'n' | 'N'
                | 'e' | 'E' | 'w' | 'W'
        )
    });
    has_marker || raw.split_whitespace().count() > 1
}

fn format_decimal_preserving_precision(
    value: f64,
    raw: &str,
    precision: StepPrecision,
) -> String {
    let raw_decimals = raw
        .trim()
        .split_once('.')
        .map_or(0, |(_, frac)| {
            frac.chars().take_while(char::is_ascii_digit).count()
        });
    let decimals = match precision {
        StepPrecision::Fine => raw_decimals.max(1),
        StepPrecision::Normal | StepPrecision::Coarse => raw_decimals,
    }
    .min(6);
    let factor = 10_f64.powi(decimals as i32);
    let rounded = (value * factor).round() / factor;
    let clean = if rounded.abs() < 0.5 / factor {
        0.0
    } else {
        rounded
    };
    format!("{clean:.decimals$}")
}

pub(crate) fn step_coordinate_value(
    raw: &str,
    fallback_deg: f64,
    axis: CoordinateAxis,
    direction: i32,
    precision: StepPrecision,
) -> (f64, String) {
    let base = parse_coordinate(raw, axis).unwrap_or(fallback_deg);
    let is_dms = is_dms_input(raw);
    let delta = f64::from(direction)
        * if is_dms {
            precision.choose(1.0 / 60.0, 1.0, 10.0)
        } else {
            precision.choose(0.1, 1.0, 10.0)
        };
    let (min_deg, max_deg) = match axis {
        CoordinateAxis::Latitude => (GeoLocation::MIN_LAT_DEG, GeoLocation::MAX_LAT_DEG),
        CoordinateAxis::Longitude => (GeoLocation::MIN_LON_DEG, GeoLocation::MAX_LON_DEG),
    };
    let next = (base + delta).clamp(min_deg, max_deg);
    let formatted = if is_dms {
        match axis {
            CoordinateAxis::Latitude => GeoLocation::format_dms(next, 'N', 'S'),
            CoordinateAxis::Longitude => GeoLocation::format_dms(next, 'E', 'W'),
        }
    } else {
        format_decimal_preserving_precision(next, raw, precision)
    };
    (next, formatted)
}

pub(crate) fn step_utc_offset_mins(
    raw: &str,
    fallback_mins: i32,
    direction: i32,
    precision: StepPrecision,
) -> i32 {
    let base = match parse_utc_offset_minutes(raw) {
        Some(Some(m)) => m,
        _ => fallback_mins,
    };
    let delta = direction * precision.choose(30, 60, 180);
    (base + delta).clamp(
        TimezoneConfig::MIN_UTC_OFFSET_MINUTES,
        TimezoneConfig::MAX_UTC_OFFSET_MINUTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hm_variants() {
        assert_eq!(parse_hm_input("21:45"), Some((21, 45)));
        assert_eq!(parse_hm_input("9h05"), Some((9, 5)));
        assert_eq!(parse_hm_input("2330"), Some((23, 30)));
        assert_eq!(parse_hm_input("830"), Some((8, 30)));
        assert_eq!(parse_hm_input("7"), Some((7, 0)));
        assert_eq!(parse_hm_input("24:00"), None);
        assert_eq!(parse_hm_input("12:60"), None);
    }

    #[test]
    fn elevation_quadratic_slider_roundtrip() {
        for elev in [-100.0, 0.0, 67.0, 500.0, 2877.0, 5000.0] {
            let t = elev_to_slider(elev);
            let back = slider_to_elev(t);
            assert!((back - elev).abs() <= 2.0, "elev={elev} -> t={t} -> {back}");
        }
    }

    #[test]
    fn stepping_helpers_support_modifiers_and_bounds() {
        assert_eq!(step_elevation_m("115", 0.0, 1, StepPrecision::Fine), 116.0);
        assert_eq!(step_elevation_m("115", 0.0, 1, StepPrecision::Normal), 125.0);
        assert_eq!(step_elevation_m("115", 0.0, 1, StepPrecision::Coarse), 215.0);
        assert_eq!(step_elevation_m("8950", 0.0, 1, StepPrecision::Coarse), 9000.0);
        assert_eq!(step_elevation_m("-495", 0.0, -1, StepPrecision::Normal), -500.0);

        assert_eq!(step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Fine), 570);
        assert_eq!(step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Normal), 579);
        assert_eq!(step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Coarse), 629);
        assert_eq!(step_time_total_minutes("00:05", (0, 0), -1, StepPrecision::Normal), -5);
        assert_eq!(step_time_total_minutes("23:55", (0, 0), 1, StepPrecision::Normal), 1445);

        let (dms_fine, dms_fine_str) =
            step_coordinate_value("48°18'30\" N", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Fine);
        assert!((dms_fine - (48.0 + 19.5 / 60.0)).abs() < 1e-6);
        assert_eq!(dms_fine_str, "48°19'30\" N");
        let (_, dms_norm_str) =
            step_coordinate_value("48°18'30\" N", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Normal);
        assert_eq!(dms_norm_str, "49°18'30\" N");
        let (_, dms_coarse_str) =
            step_coordinate_value("85°00'00\" N", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Coarse);
        assert_eq!(dms_coarse_str, "90°00'00\" N");

        let (dec_fine, dec_fine_str) =
            step_coordinate_value("48", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Fine);
        assert!((dec_fine - 48.1).abs() < 1e-9);
        assert_eq!(dec_fine_str, "48.1");
        let (_, dec_norm_str) =
            step_coordinate_value("48.50", 0.0, CoordinateAxis::Latitude, -1, StepPrecision::Normal);
        assert_eq!(dec_norm_str, "47.50");
        let (_, dec_zero_str) =
            step_coordinate_value("-0.1", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Fine);
        assert_eq!(dec_zero_str, "0.0");

        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Fine), 90);
        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Normal), 120);
        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Coarse), 240);
        assert_eq!(step_utc_offset_mins("+13", 0, 1, StepPrecision::Coarse), 840);
        assert_eq!(step_utc_offset_mins("-11", 0, -1, StepPrecision::Coarse), -720);
    }
}
