use std::collections::HashMap;

use gpui::{
    App, Context, DEFAULT_ADDITIONAL_WINDOW_SIZE, Entity, FocusHandle, Focusable, IntoElement,
    Pixels, Render, ScrollHandle, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px,
};
use persistence::{
    AppSettings, DEFAULT_ATMOSPHERE_STRENGTH, DEFAULT_GRID_OPACITY, DEFAULT_GROUND_OPACITY,
    DEFAULT_HIPS_SURVEY_OPACITY, DEFAULT_HORIZON_OPACITY, DEFAULT_SOLAR_SYSTEM_OPACITY,
    DEFAULT_STARS_OPACITY,
};
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, DividerColor, Icon, IconButton, IconName,
    IconSize, Label, LabelCommon, LabelSize, Switch, SwitchColor, ToggleState, Tooltip,
    TreeViewItem, WithScrollbar, h_flex, prelude::*, v_flex,
};

use components::{Slider, SliderEvent, TextField, TextFieldEvent};

use crate::shortcuts::open_shortcuts_window;
use crate::state::{GlobalSettings, SettingsModel};

const SIDEBAR_WIDTH: Pixels = px(226.0);
const CONTENT_MIN_WIDTH: Pixels = px(400.0);

/// Interactive control descriptor for a single setting item.
pub enum SettingControl {
    Switch {
        get: fn(&AppSettings) -> bool,
        set: fn(&mut SettingsModel, bool, &mut Context<SettingsModel>),
        default: bool,
    },
    Slider {
        get: fn(&AppSettings) -> f32,
        preview: fn(&mut SettingsModel, f32, &mut Context<SettingsModel>),
        commit: fn(&mut SettingsModel, f32, &mut Context<SettingsModel>),
        min: f32,
        max: f32,
        default: f32,
        format: fn(f32) -> String,
    },
    ActionLink {
        button_text: &'static str,
        on_click: fn(&mut Window, &mut App),
    },
}

/// Individual setting row rendered inside a [`SettingsSection`].
pub struct SettingItem {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub control: SettingControl,
}

/// Logical group of settings rendered under a section header.
pub struct SettingsSection {
    pub title: &'static str,
    pub items: Vec<SettingItem>,
}

/// Top-level settings category shown in the left navigation sidebar.
pub struct SettingsCategory {
    pub title: &'static str,
    pub sections: Vec<SettingsSection>,
}

#[derive(Debug, Clone)]
struct NavBarEntry {
    title: &'static str,
    is_root: bool,
    expanded: bool,
    category_index: usize,
    section_index: Option<usize>,
}

fn default_categories() -> Vec<SettingsCategory> {
    vec![
        SettingsCategory {
            title: "Planetarium",
            sections: vec![
                SettingsSection {
                    title: "Atmosphere",
                    items: vec![
                        SettingItem {
                            id: "planetarium_atmosphere_enabled",
                            title: "Show Atmosphere",
                            description: "Simulate daytime sky scattering and twilight glow based on solar altitude. Mostly disabled at night.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.atmosphere.enabled,
                                set: |model, value, cx| model.set_atmosphere_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_atmosphere_strength",
                            title: "Atmosphere Strength",
                            description: "Adjust the intensity of the day and twilight sky atmosphere in the planetarium viewport.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.atmosphere.strength,
                                preview: |model, value, cx| model.preview_atmosphere_strength(value, cx),
                                commit: |model, value, cx| model.commit_atmosphere_strength(value, cx),
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_ATMOSPHERE_STRENGTH,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "Ground",
                    items: vec![
                        SettingItem {
                            id: "planetarium_ground_enabled",
                            title: "Show Ground",
                            description: "Mask the lower hemisphere below the local horizon.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.ground.enabled,
                                set: |model, value, cx| model.set_ground_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_ground_opacity",
                            title: "Ground Opacity",
                            description: "Adjust the opacity of the ground hemisphere mask below the horizon.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.ground.opacity,
                                preview: |model, value, cx| model.preview_ground_opacity(value, cx),
                                commit: |model, value, cx| model.commit_ground_opacity(value, cx),
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_GROUND_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "Custom Horizon",
                    items: vec![
                        SettingItem {
                            id: "planetarium_horizon_enabled",
                            title: "Show Custom Horizon",
                            description: "Display the observatory's custom horizon silhouette and 360° background panorama.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.horizon.enabled,
                                set: |model, value, cx| model.set_horizon_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_horizon_opacity",
                            title: "Custom Horizon Opacity",
                            description: "Adjust the opacity of the custom horizon line and 360° landscape layer.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.horizon.opacity,
                                preview: |model, value, cx| model.preview_horizon_opacity(value, cx),
                                commit: |model, value, cx| model.commit_horizon_opacity(value, cx),
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_HORIZON_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                        SettingItem {
                            id: "planetarium_horizon_show_line",
                            title: "Show Horizon Line",
                            description: "Draw the horizon contour line (blue) on top of the horizon profile or 360° panoramic background.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.horizon.show_line,
                                set: |model, value, cx| model.set_horizon_show_line(value, cx),
                                default: false,
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "HiPS Survey",
                    items: vec![
                        SettingItem {
                            id: "planetarium_hips_survey_enabled",
                            title: "Show HiPS Survey",
                            description: "Display progressive HiPS all-sky background imagery in the planetarium viewport.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.hips_survey.enabled,
                                set: |model, value, cx| model.set_hips_survey_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_hips_survey_opacity",
                            title: "HiPS Survey Opacity",
                            description: "Adjust the opacity of the HiPS sky survey imagery layer.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.hips_survey.opacity,
                                preview: |model, value, cx| {
                                    model.preview_hips_survey_opacity(value, cx);
                                },
                                commit: |model, value, cx| {
                                    model.commit_hips_survey_opacity(value, cx);
                                },
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_HIPS_SURVEY_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "Stars",
                    items: vec![
                        SettingItem {
                            id: "planetarium_stars_enabled",
                            title: "Show Stars",
                            description: "Display catalog star points in the planetarium viewport.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.stars.enabled,
                                set: |model, value, cx| model.set_stars_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_stars_opacity",
                            title: "Stars Opacity",
                            description: "Adjust the opacity of rendered catalog star points.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.stars.opacity,
                                preview: |model, value, cx| model.preview_stars_opacity(value, cx),
                                commit: |model, value, cx| model.commit_stars_opacity(value, cx),
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_STARS_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "Solar System",
                    items: vec![
                        SettingItem {
                            id: "planetarium_solar_system_enabled",
                            title: "Show Solar System Objects",
                            description: "Display the Sun and Moon in the planetarium viewport.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.solar_system.enabled,
                                set: |model, value, cx| model.set_solar_system_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_solar_system_opacity",
                            title: "Solar System Opacity",
                            description: "Adjust the opacity of rendered solar-system objects (Sun and Moon).",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.solar_system.opacity,
                                preview: |model, value, cx| {
                                    model.preview_solar_system_opacity(value, cx);
                                },
                                commit: |model, value, cx| {
                                    model.commit_solar_system_opacity(value, cx);
                                },
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_SOLAR_SYSTEM_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
                SettingsSection {
                    title: "Coordinate Grids",
                    items: vec![
                        SettingItem {
                            id: "planetarium_altaz_grid_enabled",
                            title: "Show Alt/Az Grid",
                            description: "Display the horizontal azimuth and altitude coordinate grid in the planetarium viewport.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.altaz_grid.enabled,
                                set: |model, value, cx| model.set_altaz_grid_enabled(value, cx),
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_altaz_grid_opacity",
                            title: "Alt/Az Grid Opacity",
                            description: "Adjust the opacity of the horizontal Alt/Az coordinate grid.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.altaz_grid.opacity,
                                preview: |model, value, cx| {
                                    model.preview_altaz_grid_opacity(value, cx);
                                },
                                commit: |model, value, cx| {
                                    model.commit_altaz_grid_opacity(value, cx);
                                },
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_GRID_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                        SettingItem {
                            id: "planetarium_equatorial_grid_enabled",
                            title: "Show Equatorial Grid",
                            description: "Display the equatorial RA and Dec coordinate grid in the planetarium viewport.",
                            control: SettingControl::Switch {
                                get: |settings| settings.planetarium.equatorial_grid.enabled,
                                set: |model, value, cx| {
                                    model.set_equatorial_grid_enabled(value, cx);
                                },
                                default: true,
                            },
                        },
                        SettingItem {
                            id: "planetarium_equatorial_grid_opacity",
                            title: "Equatorial Grid Opacity",
                            description: "Adjust the opacity of the equatorial RA/Dec coordinate grid.",
                            control: SettingControl::Slider {
                                get: |settings| settings.planetarium.equatorial_grid.opacity,
                                preview: |model, value, cx| {
                                    model.preview_equatorial_grid_opacity(value, cx);
                                },
                                commit: |model, value, cx| {
                                    model.commit_equatorial_grid_opacity(value, cx);
                                },
                                min: 0.0,
                                max: 1.0,
                                default: DEFAULT_GRID_OPACITY,
                                format: |v| format!("{:.0}%", v * 100.0),
                            },
                        },
                    ],
                },
            ],
        },
        SettingsCategory {
            title: "Keymap",
            sections: vec![SettingsSection {
                title: "Keybindings",
                items: vec![SettingItem {
                    id: "keymap_edit_keybindings",
                    title: "Edit Keybindings",
                    description: "Customize keyboard shortcuts in the keyboard shortcuts window.",
                    control: SettingControl::ActionLink {
                        button_text: "Open Keymap",
                        on_click: |_window, cx| open_shortcuts_window(cx),
                    },
                }],
            }],
        },
        SettingsCategory {
            title: "Debug",
            sections: vec![SettingsSection {
                title: "Planetarium",
                items: vec![SettingItem {
                    id: "debug_show_hips_grid",
                    title: "Show HiPS Grid",
                    description: "Display the green HEALPix hierarchical progressive survey tile boundary lines in the planetarium viewport.",
                    control: SettingControl::Switch {
                        get: |settings| settings.debug.show_hips_grid,
                        set: |model, value, cx| model.set_show_hips_grid(value, cx),
                        default: false,
                    },
                }],
            }],
        },
    ]
}

fn normalize_slider_value(value: f32, min: f32, max: f32) -> f32 {
    let span = max - min;
    if !span.is_finite() || span.abs() <= 1e-6 || !value.is_finite() {
        return 0.0;
    }
    ((value - min) / span).clamp(0.0, 1.0)
}

fn denormalize_slider_value(norm: f32, min: f32, max: f32) -> f32 {
    min + norm.clamp(0.0, 1.0) * (max - min)
}

fn build_navbar_entries(categories: &[SettingsCategory]) -> Vec<NavBarEntry> {
    let mut entries = Vec::new();
    for (category_index, category) in categories.iter().enumerate() {
        entries.push(NavBarEntry {
            title: category.title,
            is_root: true,
            expanded: false,
            category_index,
            section_index: None,
        });
        for (section_index, section) in category.sections.iter().enumerate() {
            entries.push(NavBarEntry {
                title: section.title,
                is_root: false,
                expanded: false,
                category_index,
                section_index: Some(section_index),
            });
        }
    }
    entries
}

fn matches_item(
    category: &SettingsCategory,
    section: &SettingsSection,
    item: &SettingItem,
    query: &str,
) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }

    let haystack_category = category.title.to_lowercase();
    let haystack_section = section.title.to_lowercase();
    let haystack_title = item.title.to_lowercase();
    let haystack_desc = item.description.to_lowercase();

    trimmed.split_whitespace().all(|term| {
        let term = term.to_lowercase();
        haystack_category.contains(&term)
            || haystack_section.contains(&term)
            || haystack_title.contains(&term)
            || haystack_desc.contains(&term)
    })
}

/// Opens the native Settings window, or activates it if already open.
pub fn open_settings_window(cx: &mut App) {
    let existing = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<SettingsWindow>());

    if let Some(existing) = existing {
        let _ = existing.update(cx, |_, window, _| {
            window.activate_window();
        });
        return;
    }

    cx.defer(|cx| {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Apogee — Settings".into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_movable: true,
            kind: gpui::WindowKind::Normal,
            window_background: cx.theme().window_background_appearance(),
            window_min_size: Some(gpui::Size {
                width: SIDEBAR_WIDTH + CONTENT_MIN_WIDTH,
                height: px(240.0),
            }),
            window_bounds: Some(WindowBounds::centered(DEFAULT_ADDITIONAL_WINDOW_SIZE, cx)),
            ..Default::default()
        };

        if let Err(err) = cx.open_window(options, |window, cx| {
            cx.new(|cx| SettingsWindow::new(window, cx))
        }) {
            tracing::error!("Failed to open settings window: {err:?}");
        }
    });
}

/// Native Settings window featuring a category tree sidebar and section-grouped settings content.
pub struct SettingsWindow {
    categories: Vec<SettingsCategory>,
    navbar_entries: Vec<NavBarEntry>,
    selected_nav_index: usize,
    search_input: Entity<TextField>,
    search_query: String,
    sliders: HashMap<&'static str, Entity<Slider>>,
    focus_handle: FocusHandle,
    content_scroll_handle: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let categories = default_categories();
        let navbar_entries = build_navbar_entries(&categories);

        let search_input = cx.new(|cx| {
            TextField::new("", window, cx)
                .placeholder("Search settings…")
                .select_all_on_focus(false)
                .frameless(true)
        });

        let settings_entity = cx.try_global::<GlobalSettings>().map(|g| g.0.clone());
        let initial_settings = settings_entity
            .as_ref()
            .map(|entity| entity.read(cx).settings().clone())
            .unwrap_or_default();

        let mut subscriptions = Vec::new();
        let mut sliders = HashMap::new();

        for category in &categories {
            for section in &category.sections {
                for item in &section.items {
                    if let SettingControl::Slider {
                        get,
                        preview,
                        commit,
                        min,
                        max,
                        default,
                        ..
                    } = item.control
                    {
                        let initial_norm = normalize_slider_value(get(&initial_settings), min, max);
                        let default_norm = normalize_slider_value(default, min, max);
                        let slider =
                            cx.new(|_cx| Slider::new(initial_norm).default_value(default_norm));
                        let settings_handle = settings_entity.clone();
                        subscriptions.push(cx.subscribe(
                            &slider,
                            move |_this, _, event: &SliderEvent, cx| {
                                if let Some(entity) = &settings_handle {
                                    entity.update(cx, |model, cx| match *event {
                                        SliderEvent::Change(t) => preview(
                                            model,
                                            denormalize_slider_value(t, min, max),
                                            cx,
                                        ),
                                        SliderEvent::Commit(t) => commit(
                                            model,
                                            denormalize_slider_value(t, min, max),
                                            cx,
                                        ),
                                    });
                                }
                            },
                        ));
                        sliders.insert(item.id, slider);
                    }
                }
            }
        }

        if let Some(settings_entity) = settings_entity {
            subscriptions.push(cx.observe(&settings_entity, |this, model, cx| {
                let current = model.read(cx).settings().clone();
                for category in &this.categories {
                    for section in &category.sections {
                        for item in &section.items {
                            if let SettingControl::Slider { get, min, max, .. } = item.control
                                && let Some(slider) = this.sliders.get(item.id)
                            {
                                let norm = normalize_slider_value(get(&current), min, max);
                                slider.update(cx, |s, cx| s.set_value(norm, cx));
                            }
                        }
                    }
                }
                cx.notify();
            }));
        }

        subscriptions.push(cx.subscribe_in(
            &search_input,
            window,
            |this, _, event: &TextFieldEvent, window, cx| match event {
                TextFieldEvent::Change(text) => {
                    this.set_search_query(text.clone(), cx);
                }
                TextFieldEvent::Cancel if !this.search_query.is_empty() => {
                    this.clear_search(window, cx);
                }
                _ => {}
            },
        ));

        subscriptions.push(cx.on_window_closed(|cx, _window_id| {
            if let Some(existing_window) = cx
                .windows()
                .into_iter()
                .find_map(|window| window.downcast::<SettingsWindow>())
                && cx.windows().len() == 1
            {
                let _ = existing_window.update(cx, |_, window, _| {
                    window.remove_window();
                });
            }
        }));

        Self {
            categories,
            navbar_entries,
            selected_nav_index: 0,
            search_input,
            search_query: String::new(),
            sliders,
            focus_handle: cx.focus_handle(),
            content_scroll_handle: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn has_query(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.ensure_valid_selection();
        self.content_scroll_handle
            .set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn clear_search(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.search_input.update(cx, |field, cx| {
            field.set_text("", cx);
        });
        self.search_query.clear();
        self.ensure_valid_selection();
        cx.notify();
    }

    fn ensure_valid_selection(&mut self) {
        if let Some(current) = self.navbar_entries.get(self.selected_nav_index) {
            let visible = match current.section_index {
                Some(sec_idx) => self.section_has_matches(current.category_index, sec_idx),
                None => self.category_has_matches(current.category_index),
            };
            if visible {
                return;
            }
        }

        let first_visible = self
            .visible_navbar_entries()
            .map(|(index, _)| index)
            .next();
        if let Some(first_visible) = first_visible {
            self.selected_nav_index = first_visible;
        }
    }

    fn category_has_matches(&self, category_index: usize) -> bool {
        let Some(category) = self.categories.get(category_index) else {
            return false;
        };
        category.sections.iter().any(|section| {
            section
                .items
                .iter()
                .any(|item| matches_item(category, section, item, &self.search_query))
        })
    }

    fn section_has_matches(&self, category_index: usize, section_index: usize) -> bool {
        let Some(category) = self.categories.get(category_index) else {
            return false;
        };
        let Some(section) = category.sections.get(section_index) else {
            return false;
        };
        section
            .items
            .iter()
            .any(|item| matches_item(category, section, item, &self.search_query))
    }

    fn visible_navbar_entries(&self) -> impl Iterator<Item = (usize, &NavBarEntry)> {
        let has_query = self.has_query();
        let mut index = 0;
        std::iter::from_fn(move || {
            while index < self.navbar_entries.len() {
                let entry_index = index;
                let entry = &self.navbar_entries[entry_index];
                index += 1;

                if entry.is_root {
                    if !self.category_has_matches(entry.category_index) {
                        while index < self.navbar_entries.len()
                            && !self.navbar_entries[index].is_root
                        {
                            index += 1;
                        }
                        continue;
                    }
                    if !entry.expanded && !has_query {
                        while index < self.navbar_entries.len()
                            && !self.navbar_entries[index].is_root
                        {
                            index += 1;
                        }
                    }
                    return Some((entry_index, entry));
                }

                if let Some(section_index) = entry.section_index
                    && self.section_has_matches(entry.category_index, section_index)
                {
                    return Some((entry_index, entry));
                }
            }
            None
        })
    }

    fn toggle_navbar_entry(&mut self, nav_entry_index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.navbar_entries.get_mut(nav_entry_index) else {
            return;
        };
        if !entry.is_root {
            return;
        }
        entry.expanded = !entry.expanded;
        self.selected_nav_index = nav_entry_index;
        cx.notify();
    }

    fn select_navbar_entry(&mut self, nav_entry_index: usize, cx: &mut Context<Self>) {
        if let Some(entry) = self.navbar_entries.get(nav_entry_index) {
            let prev_category = self
                .navbar_entries
                .get(self.selected_nav_index)
                .map(|e| e.category_index);
            let category_changed = prev_category != Some(entry.category_index);
            self.selected_nav_index = nav_entry_index;
            if category_changed {
                self.content_scroll_handle
                    .set_offset(gpui::point(px(0.0), px(0.0)));
            }
            cx.notify();
        }
    }

    fn active_category(&self) -> Option<&SettingsCategory> {
        let entry = self.navbar_entries.get(self.selected_nav_index)?;
        if !self.category_has_matches(entry.category_index) {
            return None;
        }
        self.categories.get(entry.category_index)
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = !self.search_query.is_empty();

        h_flex()
            .id("settings-ui-search")
            .py_1()
            .pl_1p5()
            .pr_0p5()
            .h_7()
            .mb_3()
            .gap_1p5()
            .items_center()
            .rounded_sm()
            .bg(cx.theme().colors().editor_background)
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(Icon::new(IconName::MagnifyingGlass).color(Color::Muted))
            .child(div().flex_1().min_w_0().child(self.search_input.clone()))
            .when(has_query, |this| {
                this.child(
                    IconButton::new("clear-search-btn", IconName::Close)
                        .icon_color(Color::Muted)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Clear"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.clear_search(window, cx);
                        })),
                )
            })
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = self.has_query();
        let entries: Vec<(usize, NavBarEntry)> = self
            .visible_navbar_entries()
            .map(|(idx, entry)| (idx, entry.clone()))
            .collect();

        v_flex()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .p_2p5()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().panel_background)
            .child(self.render_search(cx))
            .child(
                v_flex()
                    .id("settings-ui-nav")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(entries.into_iter().map(|(entry_index, entry)| {
                        let is_selected = self.selected_nav_index == entry_index;
                        TreeViewItem::new(("settings-ui-navbar-entry", entry_index), entry.title)
                            .root_item(entry.is_root)
                            .toggle_state(is_selected)
                            .when(entry.is_root, |item| {
                                item.expanded(entry.expanded || has_query).on_toggle(
                                    cx.listener(move |this, _, _window, cx| {
                                        this.toggle_navbar_entry(entry_index, cx);
                                    }),
                                )
                            })
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                if entry.is_root && event.click_count() == 2 {
                                    this.toggle_navbar_entry(entry_index, cx);
                                } else {
                                    this.select_navbar_entry(entry_index, cx);
                                }
                            }))
                    })),
            )
    }

    fn render_no_results(&self) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(Label::new("No Results"))
            .child(
                Label::new(format!("No settings match \"{}\"", self.search_query))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    fn render_setting_item(
        &self,
        item: &SettingItem,
        is_last_in_section: bool,
        app_settings: &AppSettings,
        settings_entity: Option<&Entity<SettingsModel>>,
        cx: &App,
    ) -> impl IntoElement {
        let (is_modified, reset_handler, control_el): (
            bool,
            Box<dyn Fn(&mut App) + 'static>,
            gpui::AnyElement,
        ) = match item.control {
            SettingControl::Switch { get, set, default } => {
                let current_value = get(app_settings);
                let is_modified = current_value != default;
                let switch_entity = settings_entity.cloned();
                let reset_entity = settings_entity.cloned();
                let reset_fn = Box::new(move |cx: &mut App| {
                    if let Some(entity) = &reset_entity {
                        entity.update(cx, |model, cx| {
                            set(model, default, cx);
                        });
                    }
                });
                let control = Switch::new(
                    format!("{}-switch", item.id),
                    ToggleState::from(current_value),
                )
                .color(SwitchColor::Accent)
                .disabled(settings_entity.is_none())
                .on_click(move |state, _window, cx| {
                    if let Some(entity) = &switch_entity {
                        let next = *state == ToggleState::Selected;
                        entity.update(cx, |model, cx| {
                            set(model, next, cx);
                        });
                    }
                })
                .into_any_element();
                (is_modified, reset_fn, control)
            }
            SettingControl::Slider {
                get,
                commit,
                default,
                format,
                ..
            } => {
                let current_value = get(app_settings);
                let is_modified = (current_value - default).abs() > 0.005;
                let reset_entity = settings_entity.cloned();
                let reset_fn = Box::new(move |cx: &mut App| {
                    if let Some(entity) = &reset_entity {
                        entity.update(cx, |model, cx| {
                            commit(model, default, cx);
                        });
                    }
                });
                let formatted = format(current_value);
                let slider_el = self
                    .sliders
                    .get(item.id)
                    .cloned()
                    .map(|s| s.into_any_element())
                    .unwrap_or_else(|| div().into_any_element());
                let control = h_flex()
                    .w(px(210.0))
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().child(slider_el))
                    .child(
                        div().w(px(40.0)).flex().justify_end().child(
                            Label::new(formatted)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .buffer_font(cx),
                        ),
                    )
                    .into_any_element();
                (is_modified, reset_fn, control)
            }
            SettingControl::ActionLink {
                button_text,
                on_click,
            } => {
                let reset_fn = Box::new(|_cx: &mut App| {});
                let control = Button::new(format!("{}-action", item.id), button_text)
                    .style(ButtonStyle::Outlined)
                    .on_click(move |_, window, cx| {
                        on_click(window, cx);
                    })
                    .into_any_element();
                (false, reset_fn, control)
            }
        };

        let padding_bottom = if is_last_in_section {
            |el: gpui::Stateful<gpui::Div>| el.pb_10()
        } else {
            |el: gpui::Stateful<gpui::Div>| el.pb_4()
        };

        v_flex()
            .group("setting-item")
            .px_8()
            .child(
                h_flex()
                    .id(item.id)
                    .w_full()
                    .min_w_0()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .pt_4()
                    .map(padding_bottom)
                    .child(
                        v_flex()
                            .relative()
                            .w_full()
                            .max_w_2_3()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_1()
                                    .child(Label::new(item.title))
                                    .when(is_modified, |this| {
                                        this.child(
                                            IconButton::new(
                                                format!("{}-reset", item.id),
                                                IconName::Undo,
                                            )
                                            .icon_color(Color::Muted)
                                            .icon_size(IconSize::Small)
                                            .tooltip(Tooltip::text("Reset to Default"))
                                            .on_click(move |_, _, cx| {
                                                reset_handler(cx);
                                            }),
                                        )
                                    }),
                            )
                            .child(
                                Label::new(item.description)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(control_el),
            )
            .when(!is_last_in_section, |this| this.child(Divider::horizontal()))
    }

    fn render_page(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings_entity = cx.try_global::<GlobalSettings>().map(|g| g.0.clone());
        let app_settings = settings_entity
            .as_ref()
            .map(|entity| entity.read(cx).settings().clone())
            .unwrap_or_default();

        let content = if let Some(category) = self.active_category() {
            let matching_sections: Vec<(&SettingsSection, Vec<&SettingItem>)> = category
                .sections
                .iter()
                .filter_map(|section| {
                    let items: Vec<&SettingItem> = section
                        .items
                        .iter()
                        .filter(|item| matches_item(category, section, item, &self.search_query))
                        .collect();
                    if items.is_empty() {
                        None
                    } else {
                        Some((section, items))
                    }
                })
                .collect();

            if matching_sections.is_empty() {
                self.render_no_results().into_any_element()
            } else {
                v_flex()
                    .id("settings-page-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll_handle)
                    .child(
                        div()
                            .px_8()
                            .child(Label::new(category.title).size(LabelSize::Large).mt_2().mb_3()),
                    )
                    .children(matching_sections.into_iter().map(|(section, items)| {
                        let item_count = items.len();
                        v_flex()
                            .w_full()
                            .child(
                                v_flex()
                                    .w_full()
                                    .px_8()
                                    .gap_1p5()
                                    .child(
                                        Label::new(section.title)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .buffer_font(cx),
                                    )
                                    .child(Divider::horizontal().color(DividerColor::BorderFaded)),
                            )
                            .children(items.into_iter().enumerate().map(|(idx, item)| {
                                let is_last_in_section = idx + 1 == item_count;
                                self.render_setting_item(
                                    item,
                                    is_last_in_section,
                                    &app_settings,
                                    settings_entity.as_ref(),
                                    cx,
                                )
                            }))
                    }))
                    .into_any_element()
            }
        } else {
            self.render_no_results().into_any_element()
        };

        v_flex()
            .id("settings-ui-page")
            .pt_6()
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(cx.theme().colors().editor_background)
            .vertical_scrollbar_for(&self.content_scroll_handle, window, cx)
            .child(content)
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme::theme_settings(cx).ui_font(cx).clone();

        div()
            .id("settings-window")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_row()
            .size_full()
            .font(ui_font)
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(self.render_nav(cx))
            .child(self.render_page(window, cx))
    }
}
