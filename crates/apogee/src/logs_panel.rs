//! Bottom "Logs" panel.
//!
//! Every `tracing` event (the same stream that is printed to the console) is captured by
//! [`LogCaptureLayer`] into a process-wide queue. The [`LogsPanel`] periodically drains that
//! queue into its own bounded buffer and renders it with a level dropdown, a domain (target)
//! tree dropdown and a text search.

use std::{
    collections::{BTreeMap, HashSet},
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Duration,
};

use components::{TextField, TextFieldEvent};
use gpui::{
    Anchor, App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    IntoElement, MouseButton, Pixels, Render, ScrollStrategy, SharedString, Stateful,
    Subscription, UniformListScrollHandle, Window, anchored, deferred, div, point, prelude::*, px,
    uniform_list,
};
use settings::ToggleLogs;
use tracing::{Level, field::Field};
use tracing_subscriber::{Layer, layer::Context as LayerContext};
use ui::{
    ActiveTheme, Button, ButtonCommon, ButtonLike, ButtonStyle, Clickable, Color, Icon,
    IconButton, IconButtonShape, IconName, IconSize, Label, LabelCommon, LabelSize,
    StyledTypography, Tooltip, h_flex, v_flex,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    workbench::Workbench,
};

/// Maximum number of entries retained by the panel (and by the not-yet-drained queue).
const MAX_ENTRIES: usize = 10_000;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const ROW_HEIGHT: Pixels = px(20.);
const MENU_OFFSET: Pixels = px(28.);

const LEVELS: [Level; 5] = [
    Level::ERROR,
    Level::WARN,
    Level::INFO,
    Level::DEBUG,
    Level::TRACE,
];

fn level_index(level: Level) -> usize {
    LEVELS.iter().position(|l| *l == level).unwrap_or(2)
}

fn level_color(level: Level, cx: &App) -> Hsla {
    let status = cx.theme().status();
    match level {
        Level::ERROR => status.error,
        Level::WARN => status.warning,
        Level::INFO => status.info,
        Level::DEBUG => status.hint,
        Level::TRACE => cx.theme().colors().text_muted,
    }
}

// ---------------------------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------------------------

pub struct LogEntry {
    pub level: Level,
    pub target: String,
    pub message: String,
    pub timestamp: String,
    /// Lower-cased `target + message`, precomputed for search.
    haystack: String,
}

static PENDING: Mutex<Vec<Arc<LogEntry>>> = Mutex::new(Vec::new());

/// `tracing` layer that forwards every event to the Logs panel.
pub struct LogCaptureLayer;

#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: String,
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            let _ = write!(self.fields, "{}={value:?}", field.name());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogCaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let mut message = visitor.message;
        if !visitor.fields.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&visitor.fields);
        }

        let meta = event.metadata();
        let target = meta.target().to_string();
        let haystack = format!("{target} {message}").to_lowercase();
        let entry = Arc::new(LogEntry {
            level: *meta.level(),
            target,
            message,
            timestamp: jiff::Zoned::now().strftime("%H:%M:%S%.3f").to_string(),
            haystack,
        });

        if let Ok(mut pending) = PENDING.lock() {
            if pending.len() >= MAX_ENTRIES {
                let excess = pending.len() + 1 - MAX_ENTRIES;
                pending.drain(..excess);
            }
            pending.push(entry);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Domain tree
// ---------------------------------------------------------------------------------------------

/// Node of the `::`-separated target tree (e.g. `bevy_render` → `bevy_render::batching`).
#[derive(Default)]
struct DomainNode {
    name: String,
    path: String,
    /// Whether events were logged with exactly this target.
    is_target: bool,
    children: BTreeMap<String, DomainNode>,
}

impl DomainNode {
    fn insert(&mut self, target: &str) {
        let mut node = self;
        let mut path = String::new();
        for segment in target.split("::") {
            if !path.is_empty() {
                path.push_str("::");
            }
            path.push_str(segment);
            let child_path = path.clone();
            node = node
                .children
                .entry(segment.to_string())
                .or_insert_with(|| DomainNode {
                    name: segment.to_string(),
                    path: child_path,
                    ..DomainNode::default()
                });
        }
        node.is_target = true;
    }

    fn find(&self, path: &str) -> Option<&DomainNode> {
        let mut node = self;
        for segment in path.split("::") {
            node = node.children.get(segment)?;
        }
        Some(node)
    }

    fn collect_targets(&self, out: &mut Vec<String>) {
        if self.is_target {
            out.push(self.path.clone());
        }
        for child in self.children.values() {
            child.collect_targets(out);
        }
    }

    /// Returns `(enabled, total)` target counts for this subtree.
    fn counts(&self, disabled: &HashSet<String>) -> (usize, usize) {
        let mut enabled = 0;
        let mut total = 0;
        if self.is_target {
            total += 1;
            if !disabled.contains(&self.path) {
                enabled += 1;
            }
        }
        for child in self.children.values() {
            let (e, t) = child.counts(disabled);
            enabled += e;
            total += t;
        }
        (enabled, total)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckState {
    On,
    Off,
    Mixed,
}

impl CheckState {
    fn from_counts(enabled: usize, total: usize) -> Self {
        if enabled == total {
            Self::On
        } else if enabled == 0 {
            Self::Off
        } else {
            Self::Mixed
        }
    }
}

struct DomainRow {
    path: String,
    name: String,
    depth: usize,
    has_children: bool,
    expanded: bool,
    state: CheckState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterMenu {
    Levels,
    Domains,
}

struct DropdownSpec {
    id: &'static str,
    label: String,
    menu: FilterMenu,
    /// Highlights the trigger when the dropdown's filter is narrowing the results.
    active: bool,
    width: Pixels,
}

fn check_box(state: CheckState, cx: &App) -> Div {
    let colors = cx.theme().colors().clone();
    let base = div()
        .flex_none()
        .size(px(14.))
        .rounded_sm()
        .border_1()
        .border_color(if state == CheckState::Off {
            colors.border
        } else {
            colors.text_accent
        })
        .flex()
        .items_center()
        .justify_center();
    match state {
        CheckState::On => base.child(
            Icon::new(IconName::Check)
                .size(IconSize::XSmall)
                .color(Color::Accent),
        ),
        CheckState::Mixed => base.child(div().size(px(6.)).rounded_xs().bg(colors.text_accent)),
        CheckState::Off => base,
    }
}

// ---------------------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------------------

pub struct LogsPanel {
    focus_handle: FocusHandle,
    entries: Vec<Arc<LogEntry>>,
    /// Indices into `entries` that pass the current filters.
    filtered: Vec<usize>,
    level_enabled: [bool; 5],
    domain_tree: DomainNode,
    known_targets: HashSet<String>,
    /// Targets hidden by the domain filter. New targets are enabled by default.
    disabled_targets: HashSet<String>,
    expanded_domains: HashSet<String>,
    open_menu: Option<FilterMenu>,
    query_lower: String,
    follow: bool,
    search_input: Entity<TextField>,
    scroll_handle: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

struct GlobalLogsPanel(Entity<LogsPanel>);

impl gpui::Global for GlobalLogsPanel {}

impl EventEmitter<PanelEvent> for LogsPanel {}

impl Focusable for LogsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl LogsPanel {
    pub fn build(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let search_input = cx.new(|cx| {
                TextField::new("", window, cx)
                    .placeholder("Search logs…")
                    .monospace(true)
            });
            let subscription = cx.subscribe_in(
                &search_input,
                window,
                |this: &mut Self, _, event: &TextFieldEvent, _window, cx| {
                    if let TextFieldEvent::Change(text) = event {
                        this.query_lower = text.to_lowercase();
                        this.refilter();
                        cx.notify();
                    }
                },
            );

            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL_INTERVAL).await;
                    if this.update(cx, |this, cx| this.drain_pending(cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();

            Self {
                focus_handle: cx.focus_handle(),
                entries: Vec::new(),
                filtered: Vec::new(),
                level_enabled: [true; 5],
                domain_tree: DomainNode::default(),
                known_targets: HashSet::new(),
                disabled_targets: HashSet::new(),
                expanded_domains: HashSet::new(),
                open_menu: None,
                query_lower: String::new(),
                follow: true,
                search_input,
                scroll_handle: UniformListScrollHandle::new(),
                _subscriptions: vec![subscription],
            }
        })
    }

    fn matches(&self, entry: &LogEntry) -> bool {
        self.level_enabled[level_index(entry.level)]
            && !self.disabled_targets.contains(&entry.target)
            && (self.query_lower.is_empty() || entry.haystack.contains(&self.query_lower))
    }

    fn refilter(&mut self) {
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.matches(entry))
            .map(|(ix, _)| ix)
            .collect();
    }

    /// Adds a newly seen target to the tree. If the nearest known ancestor domain is entirely
    /// disabled, the new target starts disabled too so a hidden subtree stays hidden.
    fn register_target(&mut self, target: &str) {
        if self.known_targets.contains(target) {
            return;
        }
        let segments: Vec<&str> = target.split("::").collect();
        for len in (1..segments.len()).rev() {
            if let Some(ancestor) = self.domain_tree.find(&segments[..len].join("::")) {
                let (enabled, total) = ancestor.counts(&self.disabled_targets);
                if total > 0 && enabled == 0 {
                    self.disabled_targets.insert(target.to_string());
                }
                break;
            }
        }
        self.known_targets.insert(target.to_string());
        self.domain_tree.insert(target);
    }

    fn drain_pending(&mut self, cx: &mut Context<Self>) {
        let new_entries = match PENDING.lock() {
            Ok(mut pending) if !pending.is_empty() => std::mem::take(&mut *pending),
            _ => return,
        };

        for entry in &new_entries {
            self.register_target(&entry.target);
        }

        let start = self.entries.len();
        self.entries.extend(new_entries);

        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
            self.refilter();
        } else {
            for ix in start..self.entries.len() {
                if self.matches(&self.entries[ix]) {
                    self.filtered.push(ix);
                }
            }
        }

        self.scroll_to_end_if_following();
        cx.notify();
    }

    fn scroll_to_end_if_following(&self) {
        if self.follow && !self.filtered.is_empty() {
            self.scroll_handle
                .scroll_to_item(self.filtered.len() - 1, ScrollStrategy::Bottom);
        }
    }

    fn filters_changed(&mut self, cx: &mut Context<Self>) {
        self.refilter();
        self.scroll_to_end_if_following();
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: FilterMenu, cx: &mut Context<Self>) {
        self.open_menu = if self.open_menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    fn set_all_levels(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.level_enabled = [enabled; 5];
        self.filters_changed(cx);
    }

    fn toggle_level(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.level_enabled[ix] = !self.level_enabled[ix];
        self.filters_changed(cx);
    }

    fn set_all_domains(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled {
            self.disabled_targets.clear();
        } else {
            self.disabled_targets.clone_from(&self.known_targets);
        }
        self.filters_changed(cx);
    }

    /// Toggles a domain and all of its sub-domains: if everything below is enabled, disable
    /// it all, otherwise enable it all.
    fn toggle_domain(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(node) = self.domain_tree.find(path) else {
            return;
        };
        let (enabled, total) = node.counts(&self.disabled_targets);
        let mut targets = Vec::new();
        node.collect_targets(&mut targets);
        if enabled == total {
            self.disabled_targets.extend(targets);
        } else {
            for target in targets {
                self.disabled_targets.remove(&target);
            }
        }
        self.filters_changed(cx);
    }

    fn toggle_domain_expanded(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded_domains.remove(path) {
            self.expanded_domains.insert(path.to_string());
        }
        cx.notify();
    }

    fn domain_rows(&self) -> Vec<DomainRow> {
        fn walk(
            node: &DomainNode,
            depth: usize,
            panel: &LogsPanel,
            rows: &mut Vec<DomainRow>,
        ) {
            for child in node.children.values() {
                let (enabled, total) = child.counts(&panel.disabled_targets);
                let has_children = !child.children.is_empty();
                let expanded = has_children && panel.expanded_domains.contains(&child.path);
                rows.push(DomainRow {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    has_children,
                    expanded,
                    state: CheckState::from_counts(enabled, total),
                });
                if expanded {
                    walk(child, depth + 1, panel, rows);
                }
            }
        }
        let mut rows = Vec::new();
        walk(&self.domain_tree, 0, self, &mut rows);
        rows
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        self.filtered.clear();
        cx.notify();
    }

    fn toggle_panel(&self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Toggle);
    }

    // -----------------------------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------------------------

    /// Wraps a dropdown trigger and its (upward opening) menu. While the menu is open a
    /// transparent window-sized backdrop closes it on outside clicks.
    fn render_dropdown(
        &self,
        spec: DropdownSpec,
        content: impl IntoElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let DropdownSpec {
            id,
            label,
            menu,
            active,
            width,
        } = spec;
        let colors = cx.theme().colors().clone();
        let viewport = window.viewport_size();
        let is_open = self.open_menu == Some(menu);

        let trigger = ButtonLike::new(id)
            .style(if is_open || active {
                ButtonStyle::Filled
            } else {
                ButtonStyle::Subtle
            })
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Label::new(label).size(LabelSize::XSmall))
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
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_menu(menu, cx);
            }));

        div()
            .relative()
            .child(trigger)
            .when(is_open, |el| {
                el.child(self.render_backdrop(viewport, cx)).child(deferred(
                    v_flex()
                        .id(SharedString::from(format!("{id}_menu")))
                        .absolute()
                        .bottom(MENU_OFFSET)
                        .left_0()
                        .w(width)
                        .occlude()
                        .rounded_md()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_md()
                        .p_1()
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(content),
                ))
            })
            .into_any_element()
    }

    fn render_backdrop(
        &self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        deferred(
            anchored()
                .position(point(px(0.), px(0.)))
                .anchor(Anchor::TopLeft)
                .child(
                    div()
                        .id("logs_menu_backdrop")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                this.open_menu = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _window, cx| {
                                this.open_menu = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                ),
        )
        .into_any_element()
    }

    fn render_menu_header(
        &self,
        id: &'static str,
        on_all: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        on_none: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_1()
            .pb_1()
            .child(
                Button::new(SharedString::from(format!("{id}_all")), "All")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _event, _window, cx| on_all(this, cx))),
            )
            .child(
                Button::new(SharedString::from(format!("{id}_none")), "None")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _event, _window, cx| on_none(this, cx))),
            )
            .into_any_element()
    }

    fn render_levels_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let rows = LEVELS.iter().enumerate().map(|(ix, level)| {
            let state = if self.level_enabled[ix] {
                CheckState::On
            } else {
                CheckState::Off
            };
            h_flex()
                .id(SharedString::from(format!("logs_level_row_{ix}")))
                .w_full()
                .px_1()
                .py_0p5()
                .gap_2()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(|style| style.bg(colors.ghost_element_hover))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_level(ix, cx);
                }))
                .child(check_box(state, cx))
                .child(
                    div()
                        .text_xs()
                        .font_buffer(cx)
                        .text_color(level_color(*level, cx))
                        .child(level.as_str()),
                )
        });
        let rows: Vec<_> = rows.collect();

        v_flex()
            .child(self.render_menu_header(
                "logs_levels",
                |this, cx| this.set_all_levels(true, cx),
                |this, cx| this.set_all_levels(false, cx),
                cx,
            ))
            .children(rows)
            .into_any_element()
    }

    fn render_domains_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors().clone();
        let rows: Vec<_> = self.domain_rows();

        let body = if rows.is_empty() {
            div()
                .p_2()
                .text_xs()
                .text_color(colors.text_muted)
                .child("No domains yet")
                .into_any_element()
        } else {
            let items = rows.into_iter().map(|row| {
                let toggle_path = row.path.clone();
                let expand_path = row.path.clone();
                let chevron = if row.has_children {
                    IconButton::new(
                        SharedString::from(format!("logs_domain_chevron_{}", row.path)),
                        if row.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                    )
                    .shape(IconButtonShape::Square)
                    .icon_size(IconSize::XSmall)
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_domain_expanded(&expand_path, cx);
                    }))
                    .into_any_element()
                } else {
                    div().size(px(22.)).flex_none().into_any_element()
                };

                h_flex()
                    .w_full()
                    .items_center()
                    .pl(px(row.depth as f32 * 14.))
                    .child(chevron)
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("logs_domain_row_{}", row.path)))
                            .flex_1()
                            .min_w_0()
                            .px_1()
                            .py_0p5()
                            .gap_2()
                            .items_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|style| style.bg(colors.ghost_element_hover))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_domain(&toggle_path, cx);
                            }))
                            .child(check_box(row.state, cx))
                            .child(
                                div()
                                    .text_xs()
                                    .font_buffer(cx)
                                    .text_color(colors.text)
                                    .truncate()
                                    .child(row.name),
                            ),
                    )
            });
            v_flex()
                .id("logs_domain_tree")
                .max_h(px(280.))
                .overflow_y_scroll()
                .children(items)
                .into_any_element()
        };

        v_flex()
            .child(self.render_menu_header(
                "logs_domains",
                |this, cx| this.set_all_domains(true, cx),
                |this, cx| this.set_all_domains(false, cx),
                cx,
            ))
            .child(body)
            .into_any_element()
    }

    fn render_log_row(&self, row: usize, cx: &App) -> Option<Stateful<Div>> {
        let colors = cx.theme().colors().clone();
        let entry = self.entries.get(*self.filtered.get(row)?)?;
        Some(
            h_flex()
                .id(("log_row", row))
                .w_full()
                .h(ROW_HEIGHT)
                .px_2()
                .gap_2()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_buffer(cx)
                .text_xs()
                .hover(|style| style.bg(colors.ghost_element_hover))
                .child(
                    div()
                        .flex_none()
                        .text_color(colors.text_muted)
                        .child(entry.timestamp.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(40.))
                        .text_color(level_color(entry.level, cx))
                        .child(entry.level.as_str()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(colors.text_muted)
                        .child(entry.target.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(colors.text)
                        .child(entry.message.replace('\n', " ↵ ")),
                ),
        )
    }
}

impl Render for LogsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();

        let enabled_levels = self.level_enabled.iter().filter(|e| **e).count();
        let level_label = if enabled_levels == LEVELS.len() {
            "Levels".to_string()
        } else {
            format!("Levels ({enabled_levels}/{})", LEVELS.len())
        };
        let known = self.known_targets.len();
        let enabled_domains = known - self.disabled_targets.len().min(known);
        let domain_label = if enabled_domains == known {
            "Domains".to_string()
        } else {
            format!("Domains ({enabled_domains}/{known})")
        };

        let levels_menu = self.render_levels_menu(cx);
        let domains_menu = self.render_domains_menu(cx);

        let toolbar = h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(self.render_dropdown(
                DropdownSpec {
                    id: "logs_levels_trigger",
                    label: level_label,
                    menu: FilterMenu::Levels,
                    active: enabled_levels != LEVELS.len(),
                    width: px(140.),
                },
                levels_menu,
                window,
                cx,
            ))
            .child(self.render_dropdown(
                DropdownSpec {
                    id: "logs_domains_trigger",
                    label: domain_label,
                    menu: FilterMenu::Domains,
                    active: enabled_domains != known,
                    width: px(280.),
                },
                domains_menu,
                window,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(24.))
                    .px_1()
                    .child(self.search_input.clone()),
            )
            .child(
                div().text_xs().text_color(colors.text_muted).child(format!(
                    "{} / {}",
                    self.filtered.len(),
                    self.entries.len()
                )),
            )
            .child(
                Button::new("logs_follow", "Follow")
                    .style(if self.follow {
                        ButtonStyle::Filled
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.follow = !this.follow;
                        this.scroll_to_end_if_following();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("logs_clear", "Clear")
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(|this, _event, _window, cx| this.clear(cx))),
            )
            .child(
                IconButton::new("logs_close", IconName::Close)
                    .shape(IconButtonShape::Square)
                    .icon_size(IconSize::Small)
                    .style(ButtonStyle::Subtle)
                    .tooltip(Tooltip::text("Close Logs"))
                    .on_click(cx.listener(|_this, _event, _window, cx| {
                        cx.emit(PanelEvent::Close);
                    })),
            );

        let body = if self.filtered.is_empty() {
            let text = if self.entries.is_empty() {
                "No logs yet"
            } else {
                "No logs match the current filters"
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.text_muted)
                .child(text)
                .into_any_element()
        } else {
            uniform_list(
                "logs_list",
                self.filtered.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|row| this.render_log_row(row, cx))
                        .collect()
                }),
            )
            .track_scroll(&self.scroll_handle)
            .size_full()
            .into_any_element()
        };

        v_flex()
            .id("logs_panel")
            .key_context("LogsPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl Panel for LogsPanel {
    fn persistent_name() -> &'static str {
        "logs_panel"
    }

    fn title(&self, _cx: &App) -> SharedString {
        "Logs".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn is_enabled_in_workspace(&self, _kind: WorkspaceKind, _cx: &App) -> bool {
        true
    }

    fn starts_open(&self, _cx: &App) -> bool {
        false
    }

    fn default_size(&self, _cx: &App) -> Pixels {
        px(260.)
    }

    fn min_size(&self, _cx: &App) -> Pixels {
        px(120.)
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.scroll_to_end_if_following();
        } else {
            self.open_menu = None;
        }
        cx.notify();
    }
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &ToggleLogs, cx| {
        if let Some(panel) = cx.try_global::<GlobalLogsPanel>().map(|g| g.0.clone()) {
            panel.update(cx, |this, cx| this.toggle_panel(cx));
        }
    });

    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let panel = LogsPanel::build(window, cx);
            cx.set_global(GlobalLogsPanel(panel.clone()));
            workbench.add_panel(panel, window, cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(targets: &[&str]) -> DomainNode {
        let mut root = DomainNode::default();
        for target in targets {
            root.insert(target);
        }
        root
    }

    #[test]
    fn tree_groups_targets_by_path_segments() {
        let root = tree(&["bevy_render", "bevy_render::batching", "client"]);
        assert_eq!(root.children.len(), 2);
        let render = root.find("bevy_render").unwrap();
        assert!(render.is_target);
        assert!(render.children.contains_key("batching"));
        assert!(root.find("bevy_render::batching").unwrap().is_target);
        assert!(root.find("bevy_render::missing").is_none());
    }

    #[test]
    fn counts_reflect_disabled_targets() {
        let root = tree(&["a", "a::b", "a::c"]);
        let mut disabled = HashSet::new();
        assert_eq!(root.find("a").unwrap().counts(&disabled), (3, 3));
        disabled.insert("a::b".to_string());
        assert_eq!(root.find("a").unwrap().counts(&disabled), (2, 3));
        assert_eq!(root.find("a::b").unwrap().counts(&disabled), (0, 1));
    }

    #[test]
    fn collect_targets_skips_intermediate_nodes() {
        let root = tree(&["a::b::c"]);
        let mut out = Vec::new();
        root.collect_targets(&mut out);
        assert_eq!(out, vec!["a::b::c".to_string()]);
    }
}
