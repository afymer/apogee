use gpui::{
    App, Context, DEFAULT_ADDITIONAL_WINDOW_SIZE, Entity, FocusHandle, Focusable, IntoElement,
    Pixels, Render, ScrollHandle, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px,
};
use persistence::AppSettings;
use ui::{
    ActiveTheme, Color, Divider, DividerColor, Icon, IconButton, IconName, IconSize, Label,
    LabelCommon, LabelSize, Switch, SwitchColor, ToggleState, Tooltip, TreeViewItem,
    WithScrollbar, h_flex, prelude::*, v_flex,
};

use components::{TextField, TextFieldEvent};

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
    vec![SettingsCategory {
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
    }]
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

        let mut subscriptions = Vec::new();

        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            subscriptions.push(cx.observe(&settings_entity, |_this, _model, cx| {
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
    ) -> impl IntoElement {
        let SettingControl::Switch { get, set, default } = item.control;
        let current_value = get(app_settings);
        let is_modified = current_value != default;

        let switch_entity = settings_entity.cloned();
        let reset_entity = settings_entity.cloned();

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
                                                if let Some(entity) = &reset_entity {
                                                    entity.update(cx, |model, cx| {
                                                        set(model, default, cx);
                                                    });
                                                }
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
                    .child(
                        Switch::new(
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
                        }),
                    ),
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
