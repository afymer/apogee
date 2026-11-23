use std::rc::Rc;

use gpui::{
    App, Context, DEFAULT_ADDITIONAL_WINDOW_SIZE, Entity, FocusHandle, Focusable, IntoElement,
    KeyBinding, KeybindingKeystroke, Keystroke, KeystrokeEvent, MouseButton, Render, ScrollHandle,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowId, WindowOptions, actions, div,
    prelude::*, px,
};
use persistence::AppSettings;
use ui::{
    ActiveTheme, Button, ButtonStyle, Color, Divider, DividerColor, Icon, IconButton, IconName,
    IconSize, Label, LabelCommon, LabelSize, Tooltip, WithScrollbar, h_flex, prelude::*, v_flex,
};

use components::{TextField, TextFieldEvent};

use crate::state::GlobalSettings;
use crate::window::open_settings_window;

const MAX_CHORD_KEYSTROKES: usize = 2;
const KEYSTROKE_COL_WIDTH: f32 = 170.0;
const SOURCE_COL_WIDTH: f32 = 90.0;
const ACTIONS_COL_WIDTH: f32 = 96.0;

actions!(
    apogee,
    [
        /// Opens the Settings window.
        OpenSettings,
        /// Opens the Keyboard Shortcuts window.
        OpenKeyboardShortcuts,
        /// Opens the Sky Object Search palette.
        OpenSkySearch,
        /// Toggles the Object Details side panel.
        ToggleObjectDetails,
        /// Centers the view on the selected object and follows it.
        CenterOnSelection,
        /// Toggles between Alt/Az and RA/Dec view modes.
        ToggleViewMode,
        /// Moves time backward by a normal increment.
        TimeStepBackward,
        /// Moves time forward by a normal increment.
        TimeStepForward,
        /// Moves time backward by a fine increment.
        TimeStepBackwardFine,
        /// Moves time forward by a fine increment.
        TimeStepForwardFine,
        /// Moves time backward by a coarse increment.
        TimeStepBackwardCoarse,
        /// Moves time forward by a coarse increment.
        TimeStepForwardCoarse,
        /// Snaps time to dawn on the current day.
        SnapToDawn,
        /// Snaps time to dusk on the current day.
        SnapToDusk,
    ]
);

/// Declarative descriptor for a shortcut-bindable action in Apogee.
pub struct ShortcutSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub default_binding: Option<&'static str>,
    pub make_binding: fn(&str) -> KeyBinding,
}

/// Returns all shortcut-capable actions in display order.
pub fn shortcut_specs() -> Vec<ShortcutSpec> {
    vec![
        ShortcutSpec {
            id: "apogee::OpenSettings",
            title: "Open Settings",
            category: "Application",
            description: "Open the Settings window.",
            // `secondary-,` resolves to `ctrl-,` on Linux/Windows (matching the unshifted
            // comma key on both AZERTY and QWERTY layouts) and `cmd-,` on macOS.
            default_binding: Some("secondary-,"),
            make_binding: |binding| KeyBinding::new(binding, OpenSettings, None),
        },
        ShortcutSpec {
            id: "apogee::OpenKeyboardShortcuts",
            title: "Open Keyboard Shortcuts",
            category: "Application",
            description: "Open the Keyboard Shortcuts window.",
            default_binding: None,
            make_binding: |binding| KeyBinding::new(binding, OpenKeyboardShortcuts, None),
        },
        ShortcutSpec {
            id: "apogee::OpenSkySearch",
            title: "Search Sky Objects",
            category: "Planetarium",
            description: "Search for stars and solar-system objects in the sky.",
            default_binding: Some("secondary-f"),
            make_binding: |binding| KeyBinding::new(binding, OpenSkySearch, None),
        },
        ShortcutSpec {
            id: "apogee::ToggleObjectDetails",
            title: "Toggle Object Details",
            category: "Planetarium",
            description: "Show or hide the Object Details side panel.",
            default_binding: Some("secondary-b"),
            make_binding: |binding| KeyBinding::new(binding, ToggleObjectDetails, None),
        },
        ShortcutSpec {
            id: "apogee::CenterOnSelection",
            title: "Center on Selected Object",
            category: "Planetarium",
            description: "Center the view on the selected object and follow it.",
            default_binding: Some("c"),
            // Scoped to the viewport so typing `c` in text fields is unaffected.
            make_binding: |binding| {
                KeyBinding::new(binding, CenterOnSelection, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::ToggleViewMode",
            title: "Toggle Coordinate Mode",
            category: "Planetarium",
            description: "Switch the view between Alt/Az (horizontal) and RA/Dec (equatorial).",
            default_binding: Some("m"),
            make_binding: |binding| {
                KeyBinding::new(binding, ToggleViewMode, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepBackward",
            title: "Time Backward",
            category: "Time",
            description: "Move time backward by 10 minutes.",
            default_binding: Some("alt-left"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepBackward, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepForward",
            title: "Time Forward",
            category: "Time",
            description: "Move time forward by 10 minutes.",
            default_binding: Some("alt-right"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepForward, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepBackwardFine",
            title: "Time Backward (Fine)",
            category: "Time",
            description: "Move time backward by 1 minute.",
            default_binding: Some("alt-shift-left"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepBackwardFine, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepForwardFine",
            title: "Time Forward (Fine)",
            category: "Time",
            description: "Move time forward by 1 minute.",
            default_binding: Some("alt-shift-right"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepForwardFine, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepBackwardCoarse",
            title: "Time Backward (Coarse)",
            category: "Time",
            description: "Move time backward by 1 hour.",
            default_binding: Some("alt-ctrl-left"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepBackwardCoarse, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::TimeStepForwardCoarse",
            title: "Time Forward (Coarse)",
            category: "Time",
            description: "Move time forward by 1 hour.",
            default_binding: Some("alt-ctrl-right"),
            make_binding: |binding| {
                KeyBinding::new(binding, TimeStepForwardCoarse, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::SnapToDawn",
            title: "Snap to Dawn",
            category: "Time",
            description: "Jump to astronomical dawn (or sunrise) on the current day.",
            default_binding: Some("alt-down"),
            make_binding: |binding| {
                KeyBinding::new(binding, SnapToDawn, Some("PlanetariumViewport"))
            },
        },
        ShortcutSpec {
            id: "apogee::SnapToDusk",
            title: "Snap to Dusk",
            category: "Time",
            description: "Jump to astronomical dusk (or sunset) on the current day.",
            default_binding: Some("alt-up"),
            make_binding: |binding| {
                KeyBinding::new(binding, SnapToDusk, Some("PlanetariumViewport"))
            },
        },
    ]
}

/// Looks up the built-in default keybinding string for an action ID.
pub fn default_binding_for_action(action_id: &str) -> Option<&'static str> {
    shortcut_specs()
        .into_iter()
        .find(|spec| spec.id == action_id)
        .and_then(|spec| spec.default_binding)
}

/// Parses a whitespace-separated GPUI keystroke sequence (e.g. `"ctrl-,"` or `"ctrl-k ctrl-s"`).
pub fn parse_keystrokes(binding: &str) -> Option<Vec<KeybindingKeystroke>> {
    let mut parsed = Vec::new();
    for token in binding.split_whitespace() {
        let ks = Keystroke::parse(token).ok()?;
        parsed.push(KeybindingKeystroke::from_keystroke(ks));
    }
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

/// Formats a sequence of keystrokes into its canonical GPUI keybinding string.
pub fn format_keystrokes(keystrokes: &[KeybindingKeystroke]) -> String {
    keystrokes
        .iter()
        .map(|ks| ks.inner().unparse())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses and normalizes a keybinding string into canonical form (e.g. `"secondary-,"` -> `"ctrl-,"` on Linux).
pub fn canonicalize_binding(binding: &str) -> Option<String> {
    let keystrokes = parse_keystrokes(binding)?;
    Some(format_keystrokes(&keystrokes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindingSource {
    Default,
    User,
}

impl BindingSource {
    fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::User => "User",
        }
    }
}

fn effective_binding(spec: &ShortcutSpec, settings: &AppSettings) -> (Option<String>, BindingSource) {
    if let Some(override_val) = settings.keymap.overrides.get(spec.id) {
        let canonical = override_val.as_deref().and_then(canonicalize_binding);
        (canonical, BindingSource::User)
    } else {
        let canonical = spec.default_binding.and_then(canonicalize_binding);
        (canonical, BindingSource::Default)
    }
}

/// Registers global handlers for shortcut actions.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| open_settings_window(cx));
    cx.on_action(|_: &OpenKeyboardShortcuts, cx| open_shortcuts_window(cx));
}

/// Synchronizes GPUI's active keymap with the effective bindings from [`AppSettings`].
pub fn sync_keybindings(settings: &AppSettings, cx: &mut App) {
    cx.clear_key_bindings();
    let mut bindings = Vec::new();
    for spec in shortcut_specs() {
        let (effective, _) = effective_binding(&spec, settings);
        if let Some(binding_str) = effective
            && parse_keystrokes(&binding_str).is_some()
        {
            bindings.push((spec.make_binding)(&binding_str));
        }
    }
    cx.bind_keys(bindings);
}

/// Opens the native Keyboard Shortcuts window, or activates it if already open.
pub fn open_shortcuts_window(cx: &mut App) {
    let existing = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ShortcutsWindow>());

    if let Some(existing) = existing {
        let _ = existing.update(cx, |_, window, _| {
            window.activate_window();
        });
        return;
    }

    cx.defer(|cx| {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Apogee — Keyboard Shortcuts".into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_movable: true,
            kind: gpui::WindowKind::Normal,
            window_background: cx.theme().window_background_appearance(),
            window_min_size: Some(gpui::Size {
                width: px(560.0),
                height: px(320.0),
            }),
            window_bounds: Some(WindowBounds::centered(DEFAULT_ADDITIONAL_WINDOW_SIZE, cx)),
            ..Default::default()
        };

        if let Err(err) = cx.open_window(options, |window, cx| {
            cx.new(|cx| ShortcutsWindow::new(window, cx))
        }) {
            tracing::error!("Failed to open keyboard shortcuts window: {err:?}");
        }
    });
}

struct EditModalState {
    spec_index: usize,
    recorded: Vec<KeybindingKeystroke>,
}

/// Zed-inspired Keyboard Shortcuts window with live search, keystroke filtering, and rebinding modal.
pub struct ShortcutsWindow {
    specs: Vec<ShortcutSpec>,
    selected_index: Option<usize>,
    search_input: Entity<TextField>,
    search_query: String,
    keystroke_search_active: bool,
    keystroke_filter: Vec<KeybindingKeystroke>,
    edit_modal: Option<EditModalState>,
    window_id: WindowId,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for ShortcutsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ShortcutsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_id = window.window_handle().window_id();
        let search_input = cx.new(|cx| {
            TextField::new("", window, cx)
                .placeholder("Search actions or keybindings…")
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
                    this.search_query.clone_from(text);
                    this.scroll_handle.set_offset(gpui::point(px(0.0), px(0.0)));
                    cx.notify();
                }
                TextFieldEvent::Cancel if !this.search_query.is_empty() => {
                    this.clear_text_search(window, cx);
                }
                _ => {}
            },
        ));

        // Intercept keystrokes when this window is active and either the Edit modal
        // or keystroke-search recording is open.
        let this_weak = cx.entity().downgrade();
        subscriptions.push(cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let Some(entity) = this_weak.upgrade() else {
                return;
            };
            let should_capture = entity.read(cx).is_capturing_keystrokes();
            if !should_capture {
                return;
            }
            cx.stop_propagation();
            entity.update(cx, |this, cx| {
                this.handle_captured_keystroke(&event.keystroke, cx);
            });
        }));

        subscriptions.push(cx.on_window_closed(|cx, _window_id| {
            if let Some(existing_window) = cx
                .windows()
                .into_iter()
                .find_map(|window| window.downcast::<ShortcutsWindow>())
                && cx.windows().len() == 1
            {
                let _ = existing_window.update(cx, |_, window, _| {
                    window.remove_window();
                });
            }
        }));

        Self {
            specs: shortcut_specs(),
            selected_index: Some(0),
            search_input,
            search_query: String::new(),
            keystroke_search_active: false,
            keystroke_filter: Vec::new(),
            edit_modal: None,
            window_id,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn is_capturing_keystrokes(&self) -> bool {
        self.edit_modal.is_some() || self.keystroke_search_active
    }

    fn handle_captured_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        // Ignore bare modifier presses if GPUI emits them with an empty key.
        if keystroke.key.is_empty() {
            return;
        }

        if let Some(modal) = &mut self.edit_modal {
            let no_mods = !keystroke.modifiers.modified();
            if no_mods && keystroke.key == "escape" {
                self.edit_modal = None;
                cx.notify();
                return;
            }
            if no_mods && keystroke.key == "enter" && !modal.recorded.is_empty() {
                self.save_edit_modal(cx);
                return;
            }
            if no_mods && keystroke.key == "backspace" && !modal.recorded.is_empty() {
                modal.recorded.pop();
                cx.notify();
                return;
            }

            let kb_ks = KeybindingKeystroke::from_keystroke(normalize_captured_keystroke(keystroke));
            if modal.recorded.len() >= MAX_CHORD_KEYSTROKES {
                modal.recorded.clear();
            }
            modal.recorded.push(kb_ks);
            cx.notify();
            return;
        }

        if self.keystroke_search_active {
            let no_mods = !keystroke.modifiers.modified();
            if no_mods && keystroke.key == "escape" {
                self.keystroke_search_active = false;
                self.keystroke_filter.clear();
                cx.notify();
                return;
            }
            if no_mods && keystroke.key == "backspace" {
                self.keystroke_filter.pop();
                cx.notify();
                return;
            }

            let kb_ks = KeybindingKeystroke::from_keystroke(normalize_captured_keystroke(keystroke));
            if self.keystroke_filter.len() >= MAX_CHORD_KEYSTROKES {
                self.keystroke_filter.clear();
            }
            self.keystroke_filter.push(kb_ks);
            cx.notify();
        }
    }

    fn clear_text_search(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.search_input.update(cx, |field, cx| {
            field.set_text("", cx);
        });
        self.search_query.clear();
        cx.notify();
    }

    fn toggle_keystroke_search(&mut self, cx: &mut Context<Self>) {
        self.keystroke_search_active = !self.keystroke_search_active;
        if !self.keystroke_search_active {
            self.keystroke_filter.clear();
        }
        cx.notify();
    }

    fn open_edit_modal(&mut self, spec_index: usize, app_settings: &AppSettings, cx: &mut Context<Self>) {
        let Some(spec) = self.specs.get(spec_index) else {
            return;
        };
        let (effective, _) = effective_binding(spec, app_settings);
        let recorded = effective
            .as_deref()
            .and_then(parse_keystrokes)
            .unwrap_or_default();
        self.selected_index = Some(spec_index);
        self.edit_modal = Some(EditModalState {
            spec_index,
            recorded,
        });
        cx.notify();
    }

    fn save_edit_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.edit_modal.take() else {
            return;
        };
        let Some(spec) = self.specs.get(modal.spec_index) else {
            return;
        };
        let new_binding = if modal.recorded.is_empty() {
            None
        } else {
            Some(format_keystrokes(&modal.recorded))
        };
        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings_entity.update(cx, |model, cx| {
                model.set_keybinding_override(spec.id, new_binding.as_deref(), cx);
            });
        }
        cx.notify();
    }

    fn unbind_action(&mut self, spec_index: usize, cx: &mut Context<Self>) {
        let Some(spec) = self.specs.get(spec_index) else {
            return;
        };
        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings_entity.update(cx, |model, cx| {
                model.set_keybinding_override(spec.id, None, cx);
            });
        }
        self.edit_modal = None;
        cx.notify();
    }

    fn reset_action(&mut self, spec_index: usize, cx: &mut Context<Self>) {
        let Some(spec) = self.specs.get(spec_index) else {
            return;
        };
        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings_entity.update(cx, |model, cx| {
                model.reset_keybinding(spec.id, cx);
            });
        }
        cx.notify();
    }

    fn reset_all(&mut self, cx: &mut Context<Self>) {
        if let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
            settings_entity.update(cx, |model, cx| {
                model.reset_all_keybindings(cx);
            });
        }
        cx.notify();
    }

    fn matches_filters(
        &self,
        spec: &ShortcutSpec,
        binding: Option<&str>,
        source: BindingSource,
    ) -> bool {
        if !self.keystroke_filter.is_empty() {
            let filter_str = format_keystrokes(&self.keystroke_filter);
            let Some(binding_str) = binding else {
                return false;
            };
            if !binding_str.starts_with(&filter_str) {
                return false;
            }
        }

        let trimmed = self.search_query.trim();
        if trimmed.is_empty() {
            return true;
        }

        let binding_text = binding.unwrap_or("unbound").to_lowercase();
        let haystack = format!(
            "{} {} {} {} {} {}",
            spec.title.to_lowercase(),
            spec.category.to_lowercase(),
            spec.description.to_lowercase(),
            spec.id.to_lowercase(),
            binding_text,
            source.label().to_lowercase(),
        );

        trimmed
            .split_whitespace()
            .all(|term| haystack.contains(&term.to_lowercase()))
    }

    fn conflicting_actions(
        &self,
        editing_index: usize,
        recorded: &[KeybindingKeystroke],
        app_settings: &AppSettings,
    ) -> Vec<&'static str> {
        if recorded.is_empty() {
            return Vec::new();
        }
        let target = format_keystrokes(recorded);
        self.specs
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != editing_index)
            .filter_map(|(_, spec)| {
                let (effective, _) = effective_binding(spec, app_settings);
                (effective.as_deref() == Some(target.as_str())).then_some(spec.title)
            })
            .collect()
    }

    fn render_toolbar(
        &self,
        has_overrides: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_query = !self.search_query.is_empty();
        let colors = cx.theme().colors();

        v_flex()
            .w_full()
            .px_6()
            .pt_5()
            .pb_3()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new("Keyboard Shortcuts").size(LabelSize::Large))
                            .child(
                                Label::new("Double-click a shortcut or click the edit icon to rebind it.")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .when(has_overrides, |this| {
                        this.child(
                            Button::new("reset-all-shortcuts", "Reset All")
                                .style(ButtonStyle::Outlined)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reset_all(cx);
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .id("shortcuts-search")
                            .flex_1()
                            .min_w_0()
                            .h_8()
                            .px_2()
                            .gap_1p5()
                            .items_center()
                            .rounded_sm()
                            .bg(colors.editor_background)
                            .border_1()
                            .border_color(colors.border)
                            .child(Icon::new(IconName::MagnifyingGlass).color(Color::Muted))
                            .child(div().flex_1().min_w_0().child(self.search_input.clone()))
                            .when(has_query, |this| {
                                this.child(
                                    IconButton::new("shortcuts-clear-search", IconName::Close)
                                        .icon_color(Color::Muted)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("Clear"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.clear_text_search(window, cx);
                                        })),
                                )
                            }),
                    )
                    .child(
                        IconButton::new("shortcuts-keystroke-search-toggle", IconName::Keyboard)
                            .icon_size(IconSize::Small)
                            .toggle_state(self.keystroke_search_active)
                            .tooltip(Tooltip::text("Search by Keystrokes"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_keystroke_search(cx);
                            })),
                    ),
            )
            .when(self.keystroke_search_active, |this| {
                let recorded_el = if self.keystroke_filter.is_empty() {
                    Label::new("Recording keystrokes… Press a key combination to filter (Esc to exit)")
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .into_any_element()
                } else {
                    ui::KeyBinding::from_keystrokes(
                        Rc::from(self.keystroke_filter.as_slice()),
                        false,
                    )
                    .into_any_element()
                };

                this.child(
                    h_flex()
                        .id("shortcuts-keystroke-filter-bar")
                        .w_full()
                        .h_8()
                        .px_2p5()
                        .gap_2()
                        .items_center()
                        .justify_between()
                        .rounded_sm()
                        .bg(colors.panel_background)
                        .border_1()
                        .border_color(colors.border_focused)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Keyboard)
                                        .size(IconSize::Small)
                                        .color(Color::Accent),
                                )
                                .child(recorded_el),
                        )
                        .when(!self.keystroke_filter.is_empty(), |bar| {
                            bar.child(
                                IconButton::new("shortcuts-clear-keystroke-filter", IconName::Close)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text("Clear Keystroke Filter"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.keystroke_filter.clear();
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
    }

    fn render_table_header(&self, cx: &App) -> impl IntoElement {
        let colors = cx.theme().colors();

        h_flex()
            .w_full()
            .px_6()
            .py_1p5()
            .gap_4()
            .items_center()
            .bg(colors.panel_background)
            .border_y_1()
            .border_color(colors.border)
            .child(
                div().flex_1().min_w_0().child(
                    Label::new("Action")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(KEYSTROKE_COL_WIDTH)).flex_none().child(
                    Label::new("Keystrokes")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(
                div().w(px(SOURCE_COL_WIDTH)).flex_none().child(
                    Label::new("Source")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .child(div().w(px(ACTIONS_COL_WIDTH)).flex_none())
    }

    fn render_row(
        &self,
        display_idx: usize,
        spec_index: usize,
        effective: Option<&str>,
        source: BindingSource,
        app_settings: &AppSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let spec = &self.specs[spec_index];
        let colors = cx.theme().colors();
        let is_selected = self.selected_index == Some(spec_index);
        let is_striped = display_idx % 2 == 1;
        let has_binding = effective.is_some();
        let is_user = source == BindingSource::User;

        let keystrokes_el = match effective.and_then(parse_keystrokes) {
            Some(keystrokes) => {
                ui::KeyBinding::from_keystrokes(Rc::from(keystrokes), false).into_any_element()
            }
            None => Label::new("Unbound")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        };

        let source_color = if is_user {
            Color::Accent
        } else {
            Color::Muted
        };

        let row_bg = if is_selected {
            colors.element_selected
        } else if is_striped {
            colors.panel_background.opacity(0.45)
        } else {
            colors.editor_background
        };

        let settings_for_dblclick = app_settings.clone();
        let settings_for_edit_btn = app_settings.clone();

        v_flex()
            .w_full()
            .child(
                h_flex()
                    .id(("shortcut-row", spec_index))
                    .w_full()
                    .px_6()
                    .py_2p5()
                    .gap_4()
                    .items_center()
                    .bg(row_bg)
                    .hover(|style| style.bg(colors.element_hover))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.selected_index = Some(spec_index);
                        if event.click_count() == 2 {
                            this.open_edit_modal(spec_index, &settings_for_dblclick, cx);
                        } else {
                            cx.notify();
                        }
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Label::new(spec.title))
                                    .child(
                                        Label::new(spec.id)
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted)
                                            .buffer_font(cx),
                                    ),
                            )
                            .child(
                                Label::new(spec.description)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex()
                            .w(px(KEYSTROKE_COL_WIDTH))
                            .flex_none()
                            .items_center()
                            .child(keystrokes_el),
                    )
                    .child(
                        h_flex()
                            .w(px(SOURCE_COL_WIDTH))
                            .flex_none()
                            .items_center()
                            .child(
                                Label::new(source.label())
                                    .size(LabelSize::Small)
                                    .color(source_color),
                            ),
                    )
                    .child(
                        h_flex()
                            .w(px(ACTIONS_COL_WIDTH))
                            .flex_none()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .child(
                                IconButton::new(("shortcut-edit", spec_index), IconName::Pencil)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text("Edit Keybinding"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_edit_modal(
                                            spec_index,
                                            &settings_for_edit_btn,
                                            cx,
                                        );
                                    })),
                            )
                            .when(is_user, |this| {
                                this.child(
                                    IconButton::new(("shortcut-reset", spec_index), IconName::Undo)
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .tooltip(Tooltip::text("Reset to Default"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.reset_action(spec_index, cx);
                                        })),
                                )
                            })
                            .when(has_binding, |this| {
                                this.child(
                                    IconButton::new(
                                        ("shortcut-unbind", spec_index),
                                        IconName::Trash,
                                    )
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text("Unbind"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.unbind_action(spec_index, cx);
                                    })),
                                )
                            }),
                    ),
            )
            .child(Divider::horizontal().color(DividerColor::BorderFaded))
    }

    fn render_edit_modal(
        &self,
        modal: &EditModalState,
        app_settings: &AppSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let spec = &self.specs[modal.spec_index];
        let spec_index = modal.spec_index;
        let conflicts = self.conflicting_actions(spec_index, &modal.recorded, app_settings);

        let recorded_el = if modal.recorded.is_empty() {
            Label::new("Press desired key combination…")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element()
        } else {
            ui::KeyBinding::from_keystrokes(Rc::from(modal.recorded.as_slice()), false)
                .into_any_element()
        };

        div()
            .id("shortcut-edit-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.edit_modal = None;
                    cx.notify();
                }),
            )
            .child(
                v_flex()
                    .id("shortcut-edit-modal")
                    .w(px(460.0))
                    .rounded_lg()
                    .bg(colors.elevated_surface_background)
                    .border_1()
                    .border_color(colors.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        v_flex()
                            .w_full()
                            .px_5()
                            .pt_4()
                            .pb_3()
                            .gap_0p5()
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(Label::new(spec.title))
                            .child(
                                Label::new(spec.description)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .px_5()
                            .py_4()
                            .gap_2()
                            .child(Label::new("Edit Keystroke").size(LabelSize::Small))
                            .child(
                                h_flex()
                                    .w_full()
                                    .h_10()
                                    .px_3()
                                    .items_center()
                                    .justify_between()
                                    .rounded_md()
                                    .bg(colors.editor_background)
                                    .border_1()
                                    .border_color(colors.border_focused)
                                    .child(recorded_el)
                                    .when(!modal.recorded.is_empty(), |this| {
                                        this.child(
                                            IconButton::new(
                                                "shortcut-modal-clear",
                                                IconName::Close,
                                            )
                                            .icon_size(IconSize::Small)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text("Clear"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(modal) = &mut this.edit_modal {
                                                    modal.recorded.clear();
                                                    cx.notify();
                                                }
                                            })),
                                        )
                                    }),
                            )
                            .child(
                                Label::new(
                                    "Press a key combination (up to 2-key chord). Enter to save, Esc to cancel.",
                                )
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            )
                            .when(!conflicts.is_empty(), |this| {
                                let names = conflicts.join(", ");
                                this.child(
                                    h_flex()
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .gap_1p5()
                                        .items_center()
                                        .rounded_sm()
                                        .bg(cx.theme().status().warning_background)
                                        .border_1()
                                        .border_color(cx.theme().status().warning_border)
                                        .child(
                                            Icon::new(IconName::Warning)
                                                .size(IconSize::Small)
                                                .color(Color::Warning),
                                        )
                                        .child(
                                            Label::new(format!(
                                                "Also bound to: {names}"
                                            ))
                                            .size(LabelSize::Small)
                                            .color(Color::Warning),
                                        ),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .px_5()
                            .py_3()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(colors.border_variant)
                            .child(
                                Button::new("shortcut-modal-unbind", "Unbind")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.unbind_action(spec_index, cx);
                                    })),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("shortcut-modal-cancel", "Cancel")
                                            .style(ButtonStyle::Subtle)
                                            .label_size(LabelSize::Small)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.edit_modal = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("shortcut-modal-save", "Save")
                                            .style(ButtonStyle::Filled)
                                            .label_size(LabelSize::Small)
                                            .disabled(modal.recorded.is_empty())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save_edit_modal(cx);
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

fn normalize_captured_keystroke(ks: &Keystroke) -> Keystroke {
    Keystroke {
        modifiers: ks.modifiers,
        key: ks.key.to_ascii_lowercase(),
        key_char: None,
    }
}

impl Render for ShortcutsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme::theme_settings(cx).ui_font(cx).clone();
        let editor_bg = cx.theme().colors().editor_background;
        let text_color = cx.theme().colors().text;

        let app_settings = cx
            .try_global::<GlobalSettings>()
            .map(|g| g.0.read(cx).settings().clone())
            .unwrap_or_default();
        let has_overrides = !app_settings.keymap.overrides.is_empty();

        let mut rows = Vec::new();
        for (spec_index, spec) in self.specs.iter().enumerate() {
            let (effective, source) = effective_binding(spec, &app_settings);
            if self.matches_filters(spec, effective.as_deref(), source) {
                rows.push((spec_index, effective, source));
            }
        }

        let body = if rows.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .child(Label::new("No Matching Shortcuts"))
                .child(
                    Label::new("Try a different search term or clear your keystroke filter.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else {
            let rendered_rows: Vec<_> = rows
                .into_iter()
                .enumerate()
                .map(|(display_idx, (spec_index, effective, source))| {
                    self.render_row(
                        display_idx,
                        spec_index,
                        effective.as_deref(),
                        source,
                        &app_settings,
                        cx,
                    )
                    .into_any_element()
                })
                .collect();

            v_flex()
                .id("shortcuts-table-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .children(rendered_rows)
                .into_any_element()
        };

        let _ = self.window_id;

        div()
            .id("shortcuts-window")
            .relative()
            .track_focus(&self.focus_handle)
            .size_full()
            .font(ui_font)
            .bg(editor_bg)
            .text_color(text_color)
            .child(
                v_flex()
                    .size_full()
                    .child(self.render_toolbar(has_overrides, cx))
                    .child(self.render_table_header(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .vertical_scrollbar_for(&self.scroll_handle, window, cx)
                            .child(body),
                    ),
            )
            .when_some(self.edit_modal.as_ref(), |this, modal| {
                this.child(self.render_edit_modal(modal, &app_settings, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use persistence::DocumentStore;

    use super::*;
    use crate::state::init_with_store;

    #[test]
    fn open_settings_default_binding_parses_comma() {
        let default = default_binding_for_action("apogee::OpenSettings")
            .expect("OpenSettings should have a default binding");
        let parsed = parse_keystrokes(default).expect("default binding should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].inner().key, ",");
        #[cfg(not(target_os = "macos"))]
        {
            assert!(parsed[0].inner().modifiers.control);
            assert!(!parsed[0].inner().modifiers.shift);
            assert_eq!(format_keystrokes(&parsed), "ctrl-,");
        }
    }

    #[test]
    fn toggle_object_details_default_binding_is_ctrl_b() {
        let default = default_binding_for_action("apogee::ToggleObjectDetails")
            .expect("ToggleObjectDetails should have a default binding");
        let parsed = parse_keystrokes(default).expect("default binding should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].inner().key, "b");
        #[cfg(not(target_os = "macos"))]
        {
            assert!(parsed[0].inner().modifiers.control);
            assert!(!parsed[0].inner().modifiers.shift);
            assert_eq!(format_keystrokes(&parsed), "ctrl-b");
        }
    }

    #[gpui::test]
    fn keybinding_overrides_persist_and_sync_to_gpui_keymap(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);

            // Default binding is registered in GPUI keymap.
            let keymap = cx.key_bindings();
            let bindings: Vec<_> = keymap
                .borrow()
                .bindings_for_action(&OpenSettings)
                .cloned()
                .collect();
            assert_eq!(bindings.len(), 1);
            assert_eq!(bindings[0].keystrokes()[0].inner().key, ",");

            // Rebind OpenSettings to ctrl-p.
            let settings_entity = cx.global::<GlobalSettings>().0.clone();
            settings_entity.update(cx, |model, cx| {
                model.set_keybinding_override("apogee::OpenSettings", Some("ctrl-p"), cx);
            });

            let bindings: Vec<_> = keymap
                .borrow()
                .bindings_for_action(&OpenSettings)
                .cloned()
                .collect();
            assert_eq!(bindings.len(), 1);
            assert_eq!(bindings[0].keystrokes()[0].inner().key, "p");

            // Unbind OpenSettings.
            settings_entity.update(cx, |model, cx| {
                model.set_keybinding_override("apogee::OpenSettings", None, cx);
            });
            assert_eq!(keymap.borrow().bindings_for_action(&OpenSettings).count(), 0);

            // Reset back to default.
            settings_entity.update(cx, |model, cx| {
                model.reset_keybinding("apogee::OpenSettings", cx);
            });
            assert!(!settings_entity.read(cx).has_keybinding_overrides());
            let bindings: Vec<_> = keymap
                .borrow()
                .bindings_for_action(&OpenSettings)
                .cloned()
                .collect();
            assert_eq!(bindings.len(), 1);
            assert_eq!(bindings[0].keystrokes()[0].inner().key, ",");
        });

        // Verify persistence round-trip.
        let persisted: AppSettings = store.load().expect("settings persisted");
        assert!(persisted.keymap.overrides.is_empty());
    }
}
