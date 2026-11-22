use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use celestial::HorizonProfile;
use chrono::{Datelike, Days, NaiveDate, Timelike};
use components::{Slider, SliderEvent, StepPrecision, TextField, TextFieldEvent};
use gpui::{
    App, Bounds, Context, CursorStyle, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, Task, Window, canvas, div, prelude::*, px, rgb,
};
use horizon::{GlobalHorizons, open_horizon_manager_window};
use persistence::{
    CoordinateAxis, GeoLocation, ObservatoryIcon, ObservatoryProfile, TimezoneConfig,
    parse_coordinate, parse_elevation_m, parse_utc_offset_minutes,
};
use ui::{
    ActiveTheme, Button, ButtonCommon, ButtonStyle, Clickable, Color, Divider, Icon, IconButton,
    IconButtonShape, IconName, IconSize, Label, LabelCommon, LabelSize, StyledTypography, h_flex,
    v_flex,
};

use crate::icons::{
    ICON_ELEVATION, ICON_HORIZON, ICON_LATITUDE, ICON_LONGITUDE, observatory_icon_svg,
    render_svg_icon,
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
    horizon_dropdown_open: bool,
    horizon_trigger_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    horizon_dropdown_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    calendar_open: bool,
    calendar_view_year: i32,
    calendar_view_month: u32,
    calendar_dropdown_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
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
            initial_year,
            initial_month,
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
                dt.year(),
                dt.month(),
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

        if let Some(horizons_entity) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
            cx.observe_in(&horizons_entity, window, |_this, _model, _window, cx| {
                cx.notify();
            })
            .detach();
        }

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
            horizon_dropdown_open: false,
            horizon_trigger_bounds: Arc::new(Mutex::new(None)),
            horizon_dropdown_bounds: Arc::new(Mutex::new(None)),
            calendar_open: false,
            calendar_view_year: initial_year,
            calendar_view_month: initial_month,
            calendar_dropdown_bounds: Arc::new(Mutex::new(None)),
            last_synced_profile_id: profile_id,
        }
    }

    fn horizon_dropdown_contains(&self, pos: Point<Pixels>) -> bool {
        self.horizon_dropdown_bounds
            .lock()
            .ok()
            .and_then(|g| *g)
            .is_some_and(|b| b.contains(&pos))
    }

    fn calendar_dropdown_contains(&self, pos: Point<Pixels>) -> bool {
        self.calendar_dropdown_bounds
            .lock()
            .ok()
            .and_then(|g| *g)
            .is_some_and(|b| b.contains(&pos))
    }

    fn step_calendar_month(&mut self, delta_months: i32, cx: &mut Context<Self>) {
        (self.calendar_view_year, self.calendar_view_month) =
            step_year_month(self.calendar_view_year, self.calendar_view_month, delta_months);
        cx.notify();
    }

    fn wire_location_slider(slider: &Entity<Slider>, axis: CoordinateAxis, cx: &mut Context<Self>) {
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
        cx.subscribe_in(
            input,
            window,
            move |this, entity, event: &TextFieldEvent, window, cx| match event {
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
                        CoordinateAxis::Latitude => {
                            (this.editing_lat, this.lat_initial_text.as_str())
                        }
                        CoordinateAxis::Longitude => {
                            (this.editing_lon, this.lon_initial_text.as_str())
                        }
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
                        CoordinateAxis::Latitude => {
                            (this.editing_lat, this.lat_initial_text.clone())
                        }
                        CoordinateAxis::Longitude => {
                            (this.editing_lon, this.lon_initial_text.clone())
                        }
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
            },
        )
        .detach();
    }

    fn wire_elevation_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            input,
            window,
            |this, entity, event: &TextFieldEvent, window, cx| match event {
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
            },
        )
        .detach();
    }

    fn wire_time_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            input,
            window,
            |this, entity, event: &TextFieldEvent, window, cx| match event {
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
                        (
                            (dt.hour(), dt.minute()),
                            model.local_midnight_unix_seconds(),
                        )
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
            },
        )
        .detach();
    }

    fn wire_utc_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            input,
            window,
            |this, entity, event: &TextFieldEvent, window, cx| match event {
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
                    let fallback_mins = observer.read(cx).effective_standard_utc_offset_minutes();
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
            },
        )
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
            self.horizon_dropdown_open = false;
            self.calendar_open = false;
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
            let mut field =
                TextField::new(active.name.clone(), window, cx).placeholder("Site name");
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
        cx.subscribe_in(
            input,
            window,
            |this, _, event: &TextFieldEvent, window, cx| match event {
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
            },
        )
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
                let name = if raw_name.is_empty() {
                    fallback
                } else {
                    raw_name
                };
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

        v_flex()
            .w_full()
            .gap_1p5()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
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
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .gap_2()
                    .child(
                        v_flex()
                            .items_start()
                            .gap_0p5()
                            .font_buffer(cx)
                            .text_size(px(11.0))
                            .text_color(colors.text_muted)
                            .child(active.location.format_latitude_dms())
                            .child(active.location.format_longitude_dms())
                            .child(format!("{:.0} m", active.location.elevation_m.round())),
                    )
                    .child(
                        v_flex()
                            .items_end()
                            .gap_0p5()
                            .font_buffer(cx)
                            .text_size(px(11.0))
                            .text_color(colors.text_muted)
                            .child(format!("LST {lst_text}"))
                            .child(format!("Sun {solar_alt_deg:+.1}°")),
                    ),
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
                        this.horizon_dropdown_open = false;
                        this.calendar_open = false;
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
                                    .child(format!(
                                        "{:.0} m",
                                        profile.location.elevation_m.round()
                                    )),
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
                .hover(|s| {
                    s.bg(colors.element_hover)
                        .border_color(colors.border_focused)
                })
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
                    .child(div().when(is_edit && can_delete, |el| {
                        el.child(
                            Button::new("delete_profile_btn", "Delete")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .color(Color::Error)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.delete_edited_profile(cx);
                                })),
                        )
                    }))
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
                                .on_click(cx.listener(
                                    |this, _event, _window, cx| {
                                        this.save_profile_editor(cx);
                                    },
                                )),
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
        let (is_live, dt, today_date, events, std_offset_mins, dst_enabled) = {
            let observer = cx.global::<GlobalObserver>().0.read(cx);
            (
                observer.clock().is_live(),
                observer.effective_local_datetime(),
                observer.today_local_date(),
                observer.daily_solar_events(),
                observer.effective_standard_utc_offset_minutes(),
                observer.effective_dst_enabled(),
            )
        };
        let selected_date = dt.date_naive();
        if !self.calendar_open {
            if let Ok(mut guard) = self.calendar_dropdown_bounds.lock() {
                *guard = None;
            }
        }
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
            div()
                .w(px(58.0))
                .child(self.utc_input.clone())
                .into_any_element()
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
                            .id("observer_now_btn")
                            .h(px(22.0))
                            .items_center()
                            .px_2()
                            .rounded_sm()
                            .border_1()
                            .border_color(if is_live {
                                status.success.opacity(0.55)
                            } else {
                                colors.border_variant
                            })
                            .bg(if is_live {
                                status.success.opacity(0.12)
                            } else {
                                colors.element_background.opacity(0.45)
                            })
                            .when(!is_live, |s| {
                                s.cursor_pointer().hover(|s| {
                                    s.bg(colors.element_hover)
                                        .border_color(colors.border_focused)
                                })
                            })
                            .on_click(|_, _window, cx| {
                                let observer = cx.global::<GlobalObserver>().0.clone();
                                observer.update(cx, |model, cx| {
                                    model.set_live(cx);
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(if is_live {
                                        status.success
                                    } else {
                                        colors.text_muted
                                    })
                                    .child("Now"),
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
                        div()
                            .relative()
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
                                                observer.update(cx, |model, cx| {
                                                    model.step_days(-1, cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .id("jump_today_btn")
                                            .debug_selector(|| "jump_today_btn".to_string())
                                            .h(px(22.0))
                                            .px_1p5()
                                            .items_center()
                                            .justify_center()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .border_1()
                                            .when(self.calendar_open, |el| {
                                                el.border_color(cyan.opacity(0.75))
                                                    .bg(cyan.opacity(0.14))
                                                    .text_color(rgb(0x7be0ec))
                                            })
                                            .when(!self.calendar_open, |el| {
                                                el.border_color(gpui::transparent_black())
                                                    .bg(gpui::transparent_black())
                                                    .text_color(colors.text)
                                                    .hover(|s| s.bg(colors.element_hover))
                                            })
                                            .text_size(px(12.0))
                                            .whitespace_nowrap()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                                this.calendar_open = !this.calendar_open;
                                                if this.calendar_open {
                                                    this.horizon_dropdown_open = false;
                                                    this.calendar_view_year = selected_date.year();
                                                    this.calendar_view_month = selected_date.month();
                                                }
                                                cx.notify();
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Right,
                                                cx.listener(|this, _event, _window, cx| {
                                                    cx.stop_propagation();
                                                    this.calendar_open = false;
                                                    let observer =
                                                        cx.global::<GlobalObserver>().0.clone();
                                                    observer.update(cx, |model, cx| {
                                                        model.jump_to_today_keep_time(cx);
                                                    });
                                                    cx.notify();
                                                }),
                                            )
                                            .child(date_label),
                                    )
                                    .child(
                                        IconButton::new("next_day_btn", IconName::ChevronRight)
                                            .shape(IconButtonShape::Square)
                                            .icon_size(IconSize::XSmall)
                                            .style(ButtonStyle::Subtle)
                                            .on_click(|_, _, cx| {
                                                let observer = cx.global::<GlobalObserver>().0.clone();
                                                observer.update(cx, |model, cx| {
                                                    model.step_days(1, cx);
                                                });
                                            }),
                                    ),
                            )
                            .when(self.calendar_open, |el| {
                                el.child(self.render_calendar_popover(
                                    selected_date,
                                    today_date,
                                    cx,
                                ))
                            }),
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

    fn render_calendar_popover(
        &self,
        selected_date: NaiveDate,
        today_date: NaiveDate,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let dropdown_bounds_store = self.calendar_dropdown_bounds.clone();
        let grid = calendar_month_grid(self.calendar_view_year, self.calendar_view_month);
        let view_year = self.calendar_view_year;
        let view_month = self.calendar_view_month;

        gpui::deferred(
            v_flex()
                .id("calendar_dropdown_panel")
                .debug_selector(|| "calendar_dropdown_panel".to_string())
                .absolute()
                .right_0()
                .top(px(26.0))
                .w(px(236.0))
                .occlude()
                .shadow_lg()
                .p_2()
                .gap_1p5()
                .rounded_lg()
                .bg(colors.elevated_surface_background.alpha(1.0))
                .border_1()
                .border_color(colors.border)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                    let dy = match event.delta {
                        ScrollDelta::Lines(pt) => pt.y,
                        ScrollDelta::Pixels(pt) => f32::from(pt.y),
                    };
                    if dy.abs() > 1e-4 {
                        let step = if event.modifiers.shift { 12 } else { 1 };
                        let delta = if dy > 0.0 { -step } else { step };
                        this.step_calendar_month(delta, cx);
                    }
                    cx.stop_propagation();
                }))
                .child(
                    div().absolute().inset_0().child(
                        canvas(
                            move |bounds, _window, _cx| {
                                if let Ok(mut guard) = dropdown_bounds_store.lock() {
                                    *guard = Some(bounds);
                                }
                            },
                            |_, (), _, _| {},
                        )
                        .size_full(),
                    ),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            IconButton::new("calendar_prev_month_btn", IconName::ChevronLeft)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::XSmall)
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.step_calendar_month(-1, cx);
                                })),
                        )
                        .child(
                            div()
                                .id("calendar_month_year_label")
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.element_hover))
                                .text_size(px(11.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, _event, _window, cx| {
                                        cx.stop_propagation();
                                        this.calendar_view_year = today_date.year();
                                        this.calendar_view_month = today_date.month();
                                        cx.notify();
                                    }),
                                )
                                .child(format!("{} {}", month_name(view_month), view_year)),
                        )
                        .child(
                            IconButton::new("calendar_next_month_btn", IconName::ChevronRight)
                                .shape(IconButtonShape::Square)
                                .icon_size(IconSize::XSmall)
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.step_calendar_month(1, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_0p5()
                        .children(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().map(
                            |&dow| {
                                h_flex()
                                    .flex_1()
                                    .h(px(18.0))
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(9.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(colors.text_muted)
                                    .child(dow)
                            },
                        )),
                )
                .when_some(grid, |el, days| {
                    el.child(
                        v_flex()
                            .w_full()
                            .gap_0p5()
                            .children(days.as_chunks::<7>().0.iter().map(|week| {
                                h_flex().w_full().gap_0p5().children(week.iter().map(
                                    |&cell_date| {
                                        let is_current_month = cell_date.year() == view_year
                                            && cell_date.month() == view_month;
                                        let is_selected = cell_date == selected_date;
                                        let is_today = cell_date == today_date;
                                        let day_id =
                                            format!("calendar_day_{}", cell_date.format("%Y-%m-%d"));
                                        let debug_id = day_id.clone();

                                        h_flex()
                                            .id(day_id)
                                            .debug_selector(move || debug_id.clone())
                                            .flex_1()
                                            .h(px(24.0))
                                            .items_center()
                                            .justify_center()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .font_buffer(cx)
                                            .text_size(px(11.0))
                                            .when(is_selected, |el| {
                                                el.bg(cyan.opacity(0.22))
                                                    .border_1()
                                                    .border_color(cyan.opacity(0.85))
                                                    .text_color(rgb(0x7be0ec))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                            })
                                            .when(!is_selected && is_today, |el| {
                                                el.border_1()
                                                    .border_color(cyan.opacity(0.45))
                                                    .text_color(rgb(0x7be0ec))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                            })
                                            .when(!is_selected && !is_today, |el| {
                                                el.text_color(if is_current_month {
                                                    colors.text
                                                } else {
                                                    colors.text_muted.opacity(0.45)
                                                })
                                            })
                                            .hover(|s| {
                                                s.bg(if is_selected {
                                                    cyan.opacity(0.30)
                                                } else {
                                                    colors.element_hover
                                                })
                                            })
                                            .on_click(cx.listener(
                                                move |this, _event, _window, cx| {
                                                    let observer =
                                                        cx.global::<GlobalObserver>().0.clone();
                                                    observer.update(cx, |model, cx| {
                                                        model.set_local_date(cell_date, cx);
                                                    });
                                                    this.calendar_open = false;
                                                    cx.notify();
                                                },
                                            ))
                                            .child(format!("{}", cell_date.day()))
                                    },
                                ))
                            })),
                    )
                }),
        )
        .with_priority(10)
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

    fn render_coordinates_band(
        &self,
        loc: GeoLocation,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    fn render_horizon_band(
        &self,
        active: &ObservatoryProfile,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();

        let backgrounds: Vec<persistence::HorizonBackground> = cx
            .try_global::<GlobalHorizons>()
            .map(|g| g.0.read(cx).backgrounds().to_vec())
            .unwrap_or_default();

        let selected_bg = active
            .horizon_id
            .as_deref()
            .and_then(|id| backgrounds.iter().find(|b| b.id == id));
        let selected_label = selected_bg
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "None (Flat 0°)".to_string());
        let selected_id = selected_bg.map(|b| b.id.clone());
        let is_open = self.horizon_dropdown_open;
        if !is_open {
            if let Ok(mut guard) = self.horizon_dropdown_bounds.lock() {
                *guard = None;
            }
        }

        let trigger_bounds_store = self.horizon_trigger_bounds.clone();
        let dropdown_bounds_store = self.horizon_dropdown_bounds.clone();
        let viewport_h = f32::from(window.viewport_size().height);
        let open_upward = self
            .horizon_trigger_bounds
            .lock()
            .ok()
            .and_then(|g| *g)
            .is_some_and(|b| f32::from(b.origin.y) + f32::from(b.size.height) + 200.0 > viewport_h);

        let manage_id_row = selected_id.clone();

        v_flex()
            .w_full()
            .gap_1p5()
            .child(section_eyebrow("HORIZON BACKGROUND", cx))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        div().absolute().inset_0().child(
                            canvas(
                                move |bounds, _window, _cx| {
                                    if let Ok(mut guard) = trigger_bounds_store.lock() {
                                        *guard = Some(bounds);
                                    }
                                },
                                |_, (), _, _| {},
                            )
                            .size_full(),
                        ),
                    )
                    .child(
                        h_flex()
                            .id("horizon_dropdown_trigger")
                            .w_full()
                            .h(px(28.0))
                            .px_2()
                            .gap_1p5()
                            .items_center()
                            .justify_between()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(colors.surface_background.opacity(0.75))
                            .border_1()
                            .border_color(if is_open {
                                cyan.opacity(0.75)
                            } else {
                                colors.border_variant
                            })
                            .hover(|s| {
                                s.bg(colors.element_hover)
                                    .border_color(colors.border_focused)
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.horizon_dropdown_open = !this.horizon_dropdown_open;
                                if this.horizon_dropdown_open {
                                    this.calendar_open = false;
                                }
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1p5()
                                    .items_center()
                                    .child(render_svg_icon(
                                        ICON_HORIZON,
                                        px(14.0),
                                        if selected_bg.is_some() {
                                            rgb(0x7be0ec).into()
                                        } else {
                                            colors.text_muted
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.0))
                                            .text_color(if selected_bg.is_some() {
                                                colors.text
                                            } else {
                                                colors.text_muted
                                            })
                                            .child(selected_label),
                                    ),
                            )
                            .child(
                                Icon::new(if is_open {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            ),
                    )
                    .when(is_open, |el| {
                        el.child(
                            gpui::deferred(
                                v_flex()
                                    .id("horizon_dropdown_panel")
                                    .debug_selector(|| "horizon_dropdown_panel".to_string())
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .when(open_upward, |el| el.bottom(px(32.0)))
                                    .when(!open_upward, |el| el.top(px(32.0)))
                                    .occlude()
                                    .shadow_lg()
                                    .p_1()
                                    .gap_0p5()
                                    .rounded_lg()
                                    .bg(colors.elevated_surface_background.alpha(1.0))
                                    .border_1()
                                    .border_color(colors.border)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        div().absolute().inset_0().child(
                                            canvas(
                                                move |bounds, _window, _cx| {
                                                    if let Ok(mut guard) =
                                                        dropdown_bounds_store.lock()
                                                    {
                                                        *guard = Some(bounds);
                                                    }
                                                },
                                                |_, (), _, _| {},
                                            )
                                            .size_full(),
                                        ),
                                    )
                                    .child(
                                        v_flex()
                                            .id("horizon_dropdown_scroll")
                                            .debug_selector(|| {
                                                "horizon_dropdown_scroll".to_string()
                                            })
                                            .w_full()
                                            .max_h(px(152.0))
                                            .overflow_y_scroll()
                                            .gap_0p5()
                                            .child(horizon_dropdown_option(
                                                "horizon_opt_none",
                                                "None (Flat 0°)",
                                                None,
                                                selected_id.is_none(),
                                                cx,
                                                cx.listener(|this, _event, _window, cx| {
                                                    this.horizon_dropdown_open = false;
                                                    let observer =
                                                        cx.global::<GlobalObserver>().0.clone();
                                                    observer.update(cx, |model, cx| {
                                                        model.set_active_horizon(None, cx);
                                                    });
                                                    cx.notify();
                                                }),
                                            ))
                                            .children(backgrounds.into_iter().map(|bg| {
                                                let bg_id = bg.id.clone();
                                                let is_active =
                                                    selected_id.as_deref() == Some(bg_id.as_str());
                                                let prof = HorizonProfile::from_samples(
                                                    bg.samples_deg,
                                                    bg.azimuth_offset_deg,
                                                );
                                                let (min_a, max_a) = prof.min_max_altitude_deg();
                                                let range_str =
                                                    format!("{min_a:+.0}°..{max_a:+.0}°");
                                                let click_id = bg_id.clone();
                                                horizon_dropdown_option(
                                                    format!("horizon_opt_{bg_id}"),
                                                    bg.name,
                                                    Some(range_str),
                                                    is_active,
                                                    cx,
                                                    cx.listener(
                                                        move |this, _event, _window, cx| {
                                                            this.horizon_dropdown_open = false;
                                                            let observer = cx
                                                                .global::<GlobalObserver>()
                                                                .0
                                                                .clone();
                                                            observer.update(cx, |model, cx| {
                                                                model.set_active_horizon(
                                                                    Some(&click_id),
                                                                    cx,
                                                                );
                                                            });
                                                            cx.notify();
                                                        },
                                                    ),
                                                )
                                            })),
                                    )
                                    .child(Divider::horizontal())
                                    .child(
                                        h_flex()
                                            .id("horizon_dropdown_manage_row")
                                            .debug_selector(|| {
                                                "horizon_dropdown_manage_row".to_string()
                                            })
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .items_center()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(colors.element_hover))
                                            .on_click(cx.listener(
                                                move |this, _event, _window, cx| {
                                                    this.dismiss(cx);
                                                    open_horizon_manager_window(
                                                        manage_id_row.clone(),
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(11.5))
                                                    .text_color(colors.text)
                                                    .child("Manage Backgrounds…"),
                                            ),
                                    ),
                            )
                            .with_priority(10),
                        )
                    }),
            )
    }
}

impl Render for ObserverPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    if this.horizon_dropdown_open || this.calendar_open {
                        this.horizon_dropdown_open = false;
                        this.calendar_open = false;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                if this.horizon_dropdown_open && this.horizon_dropdown_contains(event.position) {
                    return;
                }
                if this.calendar_open && this.calendar_dropdown_contains(event.position) {
                    return;
                }
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
                    } else if this.horizon_dropdown_open {
                        this.horizon_dropdown_open = false;
                        cx.notify();
                    } else if this.calendar_open {
                        this.calendar_open = false;
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
            .child(Divider::horizontal())
            .child(self.render_horizon_band(&active, window, cx))
    }
}

fn horizon_dropdown_option(
    id: impl Into<gpui::ElementId>,
    title: impl Into<gpui::SharedString>,
    subtitle: Option<String>,
    selected: bool,
    cx: &App,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let cyan: Hsla = rgb(0x42b3c2).into();
    let title: gpui::SharedString = title.into();

    h_flex()
        .id(id)
        .w_full()
        .px_2()
        .py_1()
        .gap_1p5()
        .items_center()
        .justify_between()
        .rounded_sm()
        .cursor_pointer()
        .when(selected, |el| el.bg(cyan.opacity(0.14)))
        .hover(|s| s.bg(colors.element_hover))
        .on_click(on_click)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .items_center()
                .child(div().w(px(12.0)).flex_none().child(if selected {
                    Icon::new(IconName::Check)
                        .size(IconSize::XSmall)
                        .color(Color::Accent)
                        .into_any_element()
                } else {
                    div().into_any_element()
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(if selected {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .child(title),
                ),
        )
        .when_some(subtitle, |el, sub| {
            el.child(
                div()
                    .flex_none()
                    .font_buffer(cx)
                    .text_size(px(10.0))
                    .text_color(colors.text_muted.opacity(0.85))
                    .child(sub),
            )
        })
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

pub(crate) fn step_year_month(year: i32, month: u32, delta_months: i32) -> (i32, u32) {
    let clamped_month = month.clamp(1, 12) as i32;
    let total = year * 12 + (clamped_month - 1) + delta_months;
    let min_total = 1970 * 12;
    let max_total = 2100 * 12 + 11;
    let bounded = total.clamp(min_total, max_total);
    let next_year = bounded.div_euclid(12);
    let next_month = (bounded.rem_euclid(12) + 1) as u32;
    (next_year, next_month)
}

pub(crate) fn calendar_month_grid(year: i32, month: u32) -> Option<[NaiveDate; 42]> {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1)?;
    let leading_days = u64::from(first_of_month.weekday().num_days_from_monday());
    let grid_start = first_of_month.checked_sub_days(Days::new(leading_days))?;
    let mut days = [first_of_month; 42];
    for (idx, slot) in days.iter_mut().enumerate() {
        *slot = grid_start.checked_add_days(Days::new(idx as u64))?;
    }
    Some(days)
}

pub(crate) fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
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
            3 => (
                single[..1].parse::<u32>().ok()?,
                single[1..].parse::<u32>().ok()?,
            ),
            4 => (
                single[..2].parse::<u32>().ok()?,
                single[2..].parse::<u32>().ok()?,
            ),
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
            '°' | '\''
                | '"'
                | '′'
                | '″'
                | 'd'
                | 'D'
                | 'm'
                | 'M'
                | 's'
                | 'S'
                | 'n'
                | 'N'
                | 'e'
                | 'E'
                | 'w'
                | 'W'
        )
    });
    has_marker || raw.split_whitespace().count() > 1
}

fn format_decimal_preserving_precision(value: f64, raw: &str, precision: StepPrecision) -> String {
    let raw_decimals = raw.trim().split_once('.').map_or(0, |(_, frac)| {
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
        assert_eq!(
            step_elevation_m("115", 0.0, 1, StepPrecision::Normal),
            125.0
        );
        assert_eq!(
            step_elevation_m("115", 0.0, 1, StepPrecision::Coarse),
            215.0
        );
        assert_eq!(
            step_elevation_m("8950", 0.0, 1, StepPrecision::Coarse),
            9000.0
        );
        assert_eq!(
            step_elevation_m("-495", 0.0, -1, StepPrecision::Normal),
            -500.0
        );

        assert_eq!(
            step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Fine),
            570
        );
        assert_eq!(
            step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Normal),
            579
        );
        assert_eq!(
            step_time_total_minutes("09:29", (0, 0), 1, StepPrecision::Coarse),
            629
        );
        assert_eq!(
            step_time_total_minutes("00:05", (0, 0), -1, StepPrecision::Normal),
            -5
        );
        assert_eq!(
            step_time_total_minutes("23:55", (0, 0), 1, StepPrecision::Normal),
            1445
        );

        let (dms_fine, dms_fine_str) = step_coordinate_value(
            "48°18'30\" N",
            0.0,
            CoordinateAxis::Latitude,
            1,
            StepPrecision::Fine,
        );
        assert!((dms_fine - (48.0 + 19.5 / 60.0)).abs() < 1e-6);
        assert_eq!(dms_fine_str, "48°19'30\" N");
        let (_, dms_norm_str) = step_coordinate_value(
            "48°18'30\" N",
            0.0,
            CoordinateAxis::Latitude,
            1,
            StepPrecision::Normal,
        );
        assert_eq!(dms_norm_str, "49°18'30\" N");
        let (_, dms_coarse_str) = step_coordinate_value(
            "85°00'00\" N",
            0.0,
            CoordinateAxis::Latitude,
            1,
            StepPrecision::Coarse,
        );
        assert_eq!(dms_coarse_str, "90°00'00\" N");

        let (dec_fine, dec_fine_str) =
            step_coordinate_value("48", 0.0, CoordinateAxis::Latitude, 1, StepPrecision::Fine);
        assert!((dec_fine - 48.1).abs() < 1e-9);
        assert_eq!(dec_fine_str, "48.1");
        let (_, dec_norm_str) = step_coordinate_value(
            "48.50",
            0.0,
            CoordinateAxis::Latitude,
            -1,
            StepPrecision::Normal,
        );
        assert_eq!(dec_norm_str, "47.50");
        let (_, dec_zero_str) = step_coordinate_value(
            "-0.1",
            0.0,
            CoordinateAxis::Latitude,
            1,
            StepPrecision::Fine,
        );
        assert_eq!(dec_zero_str, "0.0");

        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Fine), 90);
        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Normal), 120);
        assert_eq!(step_utc_offset_mins("+1", 0, 1, StepPrecision::Coarse), 240);
        assert_eq!(
            step_utc_offset_mins("+13", 0, 1, StepPrecision::Coarse),
            840
        );
        assert_eq!(
            step_utc_offset_mins("-11", 0, -1, StepPrecision::Coarse),
            -720
        );
    }

    struct TestThemeSettingsProvider {
        ui_font: gpui::Font,
        buffer_font: gpui::Font,
    }

    impl Default for TestThemeSettingsProvider {
        fn default() -> Self {
            Self {
                ui_font: gpui::font(".ZedSans"),
                buffer_font: gpui::font(".ZedMono"),
            }
        }
    }

    impl theme::ThemeSettingsProvider for TestThemeSettingsProvider {
        fn ui_font<'a>(&'a self, _cx: &'a App) -> &'a gpui::Font {
            &self.ui_font
        }
        fn buffer_font<'a>(&'a self, _cx: &'a App) -> &'a gpui::Font {
            &self.buffer_font
        }
        fn ui_font_size(&self, _cx: &App) -> Pixels {
            px(14.0)
        }
        fn buffer_font_size(&self, _cx: &App) -> Pixels {
            px(14.0)
        }
        fn ui_density(&self, _cx: &App) -> theme::UiDensity {
            theme::UiDensity::Default
        }
    }

    struct TestPopoverHost {
        popover: Entity<ObserverPopover>,
        dismissed: bool,
    }

    impl Render for TestPopoverHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .position(gpui::point(px(900.0), px(40.0)))
                        .snap_to_window_with_margin(px(8.0))
                        .child(div().occlude().child(self.popover.clone())),
                )
                .with_priority(1),
            )
        }
    }

    #[gpui::test]
    fn horizon_dropdown_is_opaque_and_manage_row_opens_manager(cx: &mut gpui::TestAppContext) {
        let store = persistence::DocumentStore::in_memory();
        let store_h = store.clone();
        let store_o = store;

        cx.update(|cx| {
            theme::set_theme_settings_provider(Box::new(TestThemeSettingsProvider::default()), cx);
            theme::init(theme::LoadThemes::JustBase, cx);
            horizon::init_with_store(store_h, cx);
            crate::state::init_with_store(store_o, cx);
            let horizons = cx.global::<horizon::GlobalHorizons>().0.clone();
            horizons.update(cx, |m, cx| {
                m.create_blank("Ridge Test", cx);
            });
        });

        let (host, visual_cx) = cx.add_window_view(|window, cx| {
            let popover = cx.new(|cx| ObserverPopover::new(window, cx));
            cx.subscribe(
                &popover,
                move |this: &mut TestPopoverHost, _, _: &DismissEvent, _| {
                    this.dismissed = true;
                },
            )
            .detach();
            TestPopoverHost {
                popover,
                dismissed: false,
            }
        });

        visual_cx.run_until_parked();
        let popover = host.read_with(visual_cx, |h, _| h.popover.clone());
        popover.update(visual_cx, |p, cx| {
            p.horizon_dropdown_open = true;
            cx.notify();
        });
        visual_cx.run_until_parked();
        // Second pass so bounds measured during the first open frame are consumed.
        popover.update(visual_cx, |_, cx| cx.notify());
        visual_cx.run_until_parked();

        let panel = visual_cx
            .debug_bounds("horizon_dropdown_panel")
            .expect("dropdown panel rendered");
        let manage = visual_cx
            .debug_bounds("horizon_dropdown_manage_row")
            .expect("manage row rendered");

        // The measured dropdown bounds must cover the panel, otherwise clicks inside it
        // are treated as "outside" clicks and dismiss the popover before the click lands.
        let measured = popover
            .read_with(visual_cx, |p, _| *p.horizon_dropdown_bounds.lock().unwrap())
            .expect("dropdown bounds measured");
        assert!(
            measured.size.height > px(1.0),
            "dropdown bounds must not be empty"
        );
        assert!(measured.contains(&manage.center()));

        // The panel must be painted with an opaque background.
        let scale = visual_cx.update(|window, _| window.scale_factor());
        let quads = visual_cx.update(|window, _| window.painted_quads());
        let opaque_panel_bg = quads.iter().any(|q| {
            let bounds_match = (q.bounds.origin.x.0 / scale - f32::from(panel.origin.x)).abs()
                < 1.0
                && (q.bounds.origin.y.0 / scale - f32::from(panel.origin.y)).abs() < 1.0
                && (q.bounds.size.width.0 / scale - f32::from(panel.size.width)).abs() < 1.0
                && (q.bounds.size.height.0 / scale - f32::from(panel.size.height)).abs() < 1.0;
            bounds_match && format!("{:?}", q.background).ends_with("a: 1.0 })")
        });
        assert!(
            opaque_panel_bg,
            "dropdown panel must paint an opaque background quad"
        );

        visual_cx.simulate_mouse_down(
            manage.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();
        assert!(
            !host.read_with(visual_cx, |h, _| h.dismissed),
            "mouse down inside the dropdown must not dismiss the popover"
        );
        let windows_before = visual_cx.read(|cx| cx.windows().len());
        visual_cx.simulate_mouse_up(
            manage.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();
        assert!(host.read_with(visual_cx, |h, _| h.dismissed));
        let windows_after = visual_cx.read(|cx| cx.windows().len());
        assert_eq!(windows_after, windows_before + 1, "manager window should open");
    }

    #[test]
    fn calendar_helpers_and_grid_alignment() {
        assert_eq!(step_year_month(2026, 1, -1), (2025, 12));
        assert_eq!(step_year_month(2025, 12, 1), (2026, 1));
        assert_eq!(step_year_month(2026, 5, -12), (2025, 5));
        assert_eq!(step_year_month(1970, 1, -1), (1970, 1));
        assert_eq!(step_year_month(2100, 12, 1), (2100, 12));
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(11), "November");
        assert_eq!(month_name(12), "December");

        // Nov 1, 2026 is a Sunday -> Monday-first grid starts Oct 26, 2026.
        let grid = calendar_month_grid(2026, 11).expect("valid month grid");
        assert_eq!(grid[0], NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!(grid[0].weekday(), chrono::Weekday::Mon);
        assert_eq!(grid[6], NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(grid[35], NaiveDate::from_ymd_opt(2026, 11, 30).unwrap());
        assert_eq!(grid[41], NaiveDate::from_ymd_opt(2026, 12, 6).unwrap());
        for pair in grid.windows(2) {
            assert_eq!(pair[1].signed_duration_since(pair[0]).num_days(), 1);
        }

        // May 1, 2026 is a Friday -> Monday-first grid starts Apr 27, 2026.
        let may_grid = calendar_month_grid(2026, 5).expect("valid May grid");
        assert_eq!(may_grid[0], NaiveDate::from_ymd_opt(2026, 4, 27).unwrap());
        assert_eq!(may_grid[4], NaiveDate::from_ymd_opt(2026, 5, 1).unwrap());
    }

    #[gpui::test]
    fn date_picker_left_click_opens_calendar_and_right_click_resets_to_today(
        cx: &mut gpui::TestAppContext,
    ) {
        let store = persistence::DocumentStore::in_memory();
        let store_h = store.clone();
        let store_o = store;

        cx.update(|cx| {
            theme::set_theme_settings_provider(Box::new(TestThemeSettingsProvider::default()), cx);
            theme::init(theme::LoadThemes::JustBase, cx);
            horizon::init_with_store(store_h, cx);
            crate::state::init_with_store(store_o, cx);
        });

        let (host, visual_cx) = cx.add_window_view(|window, cx| {
            let popover = cx.new(|cx| ObserverPopover::new(window, cx));
            cx.subscribe(
                &popover,
                move |this: &mut TestPopoverHost, _, _: &DismissEvent, _| {
                    this.dismissed = true;
                },
            )
            .detach();
            TestPopoverHost {
                popover,
                dismissed: false,
            }
        });

        let observer = visual_cx.read(|cx| cx.global::<GlobalObserver>().0.clone());
        let today = observer.read_with(visual_cx, |m, _| m.today_local_date());
        let (initial_date, picked_date, day_selector) =
            if today == NaiveDate::from_ymd_opt(2026, 5, 15).unwrap() {
                (
                    NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
                    NaiveDate::from_ymd_opt(2026, 6, 20).unwrap(),
                    "calendar_day_2026-06-20",
                )
            } else {
                (
                    NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
                    NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
                    "calendar_day_2026-05-20",
                )
            };

        observer.update(visual_cx, |m, cx| {
            m.set_local_date(initial_date, cx);
            m.set_local_hm(22, 30, cx);
        });
        visual_cx.run_until_parked();

        let popover = host.read_with(visual_cx, |h, _| h.popover.clone());
        let jump_btn = visual_cx
            .debug_bounds("jump_today_btn")
            .expect("date button rendered");

        // Left-click opens the calendar picker without dismissing the popover.
        visual_cx.simulate_mouse_down(
            jump_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.simulate_mouse_up(
            jump_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();

        assert!(
            popover.read_with(visual_cx, |p, _| p.calendar_open),
            "left-clicking the date button should open the calendar"
        );
        assert!(!host.read_with(visual_cx, |h, _| h.dismissed));

        let panel = visual_cx
            .debug_bounds("calendar_dropdown_panel")
            .expect("calendar panel rendered");
        let day_btn = visual_cx
            .debug_bounds(day_selector)
            .expect("day cell rendered");

        let measured = popover
            .read_with(visual_cx, |p, _| *p.calendar_dropdown_bounds.lock().unwrap())
            .expect("calendar dropdown bounds measured");
        assert!(measured.size.height > px(1.0));
        assert!(measured.contains(&day_btn.center()));

        let scale = visual_cx.update(|window, _| window.scale_factor());
        let quads = visual_cx.update(|window, _| window.painted_quads());
        let opaque_panel_bg = quads.iter().any(|q| {
            let bounds_match = (q.bounds.origin.x.0 / scale - f32::from(panel.origin.x)).abs()
                < 1.0
                && (q.bounds.origin.y.0 / scale - f32::from(panel.origin.y)).abs() < 1.0
                && (q.bounds.size.width.0 / scale - f32::from(panel.size.width)).abs() < 1.0
                && (q.bounds.size.height.0 / scale - f32::from(panel.size.height)).abs() < 1.0;
            bounds_match && format!("{:?}", q.background).ends_with("a: 1.0 })")
        });
        assert!(
            opaque_panel_bg,
            "calendar panel must paint an opaque background quad"
        );

        // Clicking a day cell selects that date, preserves local HH:MM, closes calendar, and keeps popover open.
        visual_cx.simulate_mouse_down(
            day_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();
        assert!(
            !host.read_with(visual_cx, |h, _| h.dismissed),
            "mouse down inside calendar must not dismiss the popover"
        );
        visual_cx.simulate_mouse_up(
            day_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();

        assert!(
            !popover.read_with(visual_cx, |p, _| p.calendar_open),
            "calendar should close after selecting a date"
        );
        assert!(!host.read_with(visual_cx, |h, _| h.dismissed));
        let after_pick = observer.read_with(visual_cx, |m, _| {
            let dt = m.effective_local_datetime();
            (dt.date_naive(), dt.hour(), dt.minute())
        });
        assert_eq!(after_pick, (picked_date, 22, 30));

        // Left-click opens the calendar again; right-click closes it and resets to today's date keeping HH:MM.
        let jump_btn = visual_cx
            .debug_bounds("jump_today_btn")
            .expect("date button rendered");
        visual_cx.simulate_mouse_down(
            jump_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.simulate_mouse_up(
            jump_btn.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();
        assert!(popover.read_with(visual_cx, |p, _| p.calendar_open));

        visual_cx.simulate_mouse_down(
            jump_btn.center(),
            MouseButton::Right,
            gpui::Modifiers::default(),
        );
        visual_cx.simulate_mouse_up(
            jump_btn.center(),
            MouseButton::Right,
            gpui::Modifiers::default(),
        );
        visual_cx.run_until_parked();

        assert!(
            !popover.read_with(visual_cx, |p, _| p.calendar_open),
            "right-clicking the date button should close the calendar"
        );
        assert!(!host.read_with(visual_cx, |h, _| h.dismissed));
        let after_reset = observer.read_with(visual_cx, |m, _| {
            let dt = m.effective_local_datetime();
            (dt.date_naive(), dt.hour(), dt.minute())
        });
        assert_eq!(after_reset, (today, 22, 30));
    }
}
