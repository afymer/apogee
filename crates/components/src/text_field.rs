use std::ops::Range;

use gpui::{
    App, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use ui::{ActiveTheme, StyledTypography, h_flex};

/// Increment granularity driven by Blender-style modifier keys (`Shift` = fine, `Ctrl` = coarse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepPrecision {
    Fine,
    Normal,
    Coarse,
}

impl StepPrecision {
    pub fn from_modifiers(shift: bool, ctrl_or_cmd: bool) -> Self {
        if shift {
            Self::Fine
        } else if ctrl_or_cmd {
            Self::Coarse
        } else {
            Self::Normal
        }
    }

    /// Standard whole-number step size (`1` fine, `10` normal, `100` coarse).
    pub fn integer_step(self) -> i64 {
        self.choose(1, 10, 100)
    }

    /// Picks between fine, normal, and coarse step values.
    pub fn choose<T>(self, fine: T, normal: T, coarse: T) -> T {
        match self {
            Self::Fine => fine,
            Self::Normal => normal,
            Self::Coarse => coarse,
        }
    }
}

/// Events emitted by [`TextField`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextFieldEvent {
    /// Emitted whenever the text content changes.
    Change(String),
    /// Emitted when Up/Down arrow keys nudge the field value.
    Step {
        direction: i32,
        precision: StepPrecision,
    },
    /// Emitted when Enter is pressed inside the field.
    Submit(String),
    /// Emitted when focus leaves the field.
    Blur(String),
    /// Emitted when Escape is pressed inside the field.
    Cancel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TextBuffer {
    text: String,
    cursor: usize,
    anchor: usize,
}

impl TextBuffer {
    fn new(initial: impl Into<String>) -> Self {
        let text = initial.into();
        let len = text.len();
        Self {
            text,
            cursor: len,
            anchor: len,
        }
    }

    fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        let len = self.text.len();
        self.cursor = len;
        self.anchor = len;
    }

    fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    fn selection_range(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn selected_text(&self) -> &str {
        &self.text[self.selection_range()]
    }

    fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    fn collapse_selection(&mut self) {
        self.anchor = self.cursor;
    }

    fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let range = self.selection_range();
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = range.start;
        true
    }

    fn insert_str(&mut self, input: &str) -> bool {
        let clean: String = input
            .chars()
            .filter(|c| *c != '\r' && *c != '\n' && !c.is_control())
            .collect();
        if clean.is_empty() && !self.has_selection() {
            return false;
        }
        let had_sel = self.delete_selection();
        if clean.is_empty() {
            return had_sel;
        }
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        self.anchor = self.cursor;
        true
    }

    fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let prev = prev_char_boundary(&self.text, self.cursor);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.anchor = prev;
        true
    }

    fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor >= self.text.len() {
            return false;
        }
        let next = next_char_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
        self.anchor = self.cursor;
        true
    }

    fn move_left(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            self.cursor = self.selection_range().start;
            self.anchor = self.cursor;
            return;
        }
        self.cursor = prev_char_boundary(&self.text, self.cursor);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_right(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            self.cursor = self.selection_range().end;
            self.anchor = self.cursor;
            return;
        }
        self.cursor = next_char_boundary(&self.text, self.cursor);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_home(&mut self, extend: bool) {
        self.cursor = 0;
        if !extend {
            self.anchor = 0;
        }
    }

    fn move_end(&mut self, extend: bool) {
        self.cursor = self.text.len();
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_word_left(&mut self, extend: bool) {
        self.cursor = prev_word_boundary(&self.text, self.cursor);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_word_right(&mut self, extend: bool) {
        self.cursor = next_word_boundary(&self.text, self.cursor);
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn delete_word_backward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let prev = prev_word_boundary(&self.text, self.cursor);
        if prev == self.cursor {
            return false;
        }
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.anchor = prev;
        true
    }

    fn delete_word_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let next = next_word_boundary(&self.text, self.cursor);
        if next == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..next, "");
        self.anchor = self.cursor;
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Punct,
}

fn char_class(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn prev_or_same_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn prev_word_boundary(s: &str, idx: usize) -> usize {
    let mut i = prev_or_same_char_boundary(s, idx);
    while let Some(ch) = s[..i].chars().next_back() {
        if char_class(ch) == CharClass::Whitespace {
            i -= ch.len_utf8();
        } else {
            break;
        }
    }
    if let Some(first) = s[..i].chars().next_back() {
        let target = char_class(first);
        i -= first.len_utf8();
        while let Some(ch) = s[..i].chars().next_back() {
            if char_class(ch) == target {
                i -= ch.len_utf8();
            } else {
                break;
            }
        }
    }
    i
}

fn next_word_boundary(s: &str, idx: usize) -> usize {
    let mut i = prev_or_same_char_boundary(s, idx);
    while let Some(ch) = s[i..].chars().next() {
        if char_class(ch) == CharClass::Whitespace {
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    if let Some(first) = s[i..].chars().next() {
        let target = char_class(first);
        i += first.len_utf8();
        while let Some(ch) = s[i..].chars().next() {
            if char_class(ch) == target {
                i += ch.len_utf8();
            } else {
                break;
            }
        }
    }
    i
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i >= s.len() {
        return s.len();
    }
    i += 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Reusable single-line GPUI text input field supporting selection, clipboard, suffix, and validation state.
pub struct TextField {
    buffer: TextBuffer,
    placeholder: SharedString,
    suffix: Option<SharedString>,
    monospace: bool,
    select_all_on_focus: bool,
    frameless: bool,
    is_invalid: bool,
    focus_handle: FocusHandle,
    _blur_subscription: Subscription,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    /// Creates a new [`TextField`] initialized with `initial`.
    pub fn new(initial: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let blur_subscription = cx.on_blur(&focus_handle, window, |this, _window, cx| {
            this.buffer.collapse_selection();
            cx.emit(TextFieldEvent::Blur(this.buffer.text.clone()));
            cx.notify();
        });

        Self {
            buffer: TextBuffer::new(initial),
            placeholder: SharedString::default(),
            suffix: None,
            monospace: false,
            select_all_on_focus: true,
            frameless: false,
            is_invalid: false,
            focus_handle,
            _blur_subscription: blur_subscription,
        }
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    #[must_use]
    pub fn suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    #[must_use]
    pub fn monospace(mut self, monospace: bool) -> Self {
        self.monospace = monospace;
        self
    }

    #[must_use]
    pub fn select_all_on_focus(mut self, select_all_on_focus: bool) -> Self {
        self.select_all_on_focus = select_all_on_focus;
        self
    }

    #[must_use]
    pub fn frameless(mut self, frameless: bool) -> Self {
        self.frameless = frameless;
        self
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Programmatically updates text without emitting [`TextFieldEvent::Change`].
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let next = text.into();
        if self.buffer.text != next || self.is_invalid {
            self.buffer.set_text(next);
            self.is_invalid = false;
            cx.notify();
        }
    }

    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if self.is_invalid != invalid {
            self.is_invalid = invalid;
            cx.notify();
        }
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        if self.select_all_on_focus {
            self.buffer.select_all();
        }
        cx.notify();
    }

    fn emit_change(&mut self, cx: &mut Context<Self>) {
        self.is_invalid = false;
        cx.emit(TextFieldEvent::Change(self.buffer.text.clone()));
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        let cmd_or_ctrl = mods.control || mods.platform;

        match key {
            "enter" => {
                cx.emit(TextFieldEvent::Submit(self.buffer.text.clone()));
                cx.stop_propagation();
                return;
            }
            "escape" => {
                cx.emit(TextFieldEvent::Cancel);
                cx.stop_propagation();
                return;
            }
            "tab" | "backtab" => {
                return;
            }
            "backspace" => {
                let changed = if mods.control || mods.alt {
                    self.buffer.delete_word_backward()
                } else {
                    self.buffer.backspace()
                };
                if changed {
                    self.emit_change(cx);
                }
                cx.stop_propagation();
                return;
            }
            "delete" => {
                let changed = if mods.control || mods.alt {
                    self.buffer.delete_word_forward()
                } else {
                    self.buffer.delete_forward()
                };
                if changed {
                    self.emit_change(cx);
                }
                cx.stop_propagation();
                return;
            }
            "left" => {
                if mods.control || mods.alt {
                    self.buffer.move_word_left(mods.shift);
                } else {
                    self.buffer.move_left(mods.shift);
                }
                cx.notify();
                cx.stop_propagation();
                return;
            }
            "right" => {
                if mods.control || mods.alt {
                    self.buffer.move_word_right(mods.shift);
                } else {
                    self.buffer.move_right(mods.shift);
                }
                cx.notify();
                cx.stop_propagation();
                return;
            }
            "up" | "down" => {
                let direction = if key == "up" { 1 } else { -1 };
                let precision = StepPrecision::from_modifiers(mods.shift, cmd_or_ctrl);
                cx.emit(TextFieldEvent::Step {
                    direction,
                    precision,
                });
                cx.stop_propagation();
                return;
            }
            "home" => {
                self.buffer.move_home(mods.shift);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            "end" => {
                self.buffer.move_end(mods.shift);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            _ => {}
        }

        if cmd_or_ctrl && !mods.alt {
            match key {
                "a" => {
                    self.buffer.select_all();
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                "c" => {
                    let copied = if self.buffer.has_selection() {
                        self.buffer.selected_text().to_string()
                    } else {
                        self.buffer.text.clone()
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(copied));
                    cx.stop_propagation();
                    return;
                }
                "x" => {
                    if self.buffer.has_selection() {
                        let copied = self.buffer.selected_text().to_string();
                        cx.write_to_clipboard(ClipboardItem::new_string(copied));
                        if self.buffer.delete_selection() {
                            self.emit_change(cx);
                        }
                    }
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if self.buffer.insert_str(&pasted) {
                            self.emit_change(cx);
                        }
                    }
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }

        if !cmd_or_ctrl && !mods.alt {
            if let Some(ch) = event.keystroke.key_char.as_deref() {
                if self.buffer.insert_str(ch) {
                    self.emit_change(cx);
                }
                cx.stop_propagation();
            } else if key == "space" {
                if self.buffer.insert_str(" ") {
                    self.emit_change(cx);
                }
                cx.stop_propagation();
            }
        }
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus_handle.is_focused(window);
        let colors = cx.theme().colors();
        let status = cx.theme().status();

        let border_color = if self.is_invalid {
            status.error
        } else if is_focused {
            colors.text_accent.opacity(0.75)
        } else {
            colors.border_variant
        };

        let bg_color = if is_focused {
            colors.editor_background
        } else {
            colors.element_background.opacity(0.55)
        };

        let content = if is_focused {
            if self.buffer.has_selection() {
                let range = self.buffer.selection_range();
                let before = self.buffer.text[..range.start].to_string();
                let selected = self.buffer.text[range.clone()].to_string();
                let after = self.buffer.text[range.end..].to_string();
                h_flex()
                    .px(px(2.0))
                    .items_center()
                    .whitespace_nowrap()
                    .when(!before.is_empty(), |el| el.child(before))
                    .child(
                        div()
                            .bg(colors.text_accent.opacity(0.28))
                            .rounded_xs()
                            .child(selected),
                    )
                    .when(!after.is_empty(), |el| el.child(after))
                    .into_any_element()
            } else {
                let before = self.buffer.text[..self.buffer.cursor].to_string();
                let after = self.buffer.text[self.buffer.cursor..].to_string();
                let caret_left = if self.buffer.cursor == 0 {
                    px(-1.5)
                } else if self.buffer.cursor == self.buffer.text.len() {
                    px(0.0)
                } else {
                    px(-0.75)
                };
                let caret = div().w(px(0.0)).h(px(14.0)).relative().flex_none().child(
                    div()
                        .absolute()
                        .top_0()
                        .left(caret_left)
                        .w(px(1.5))
                        .h(px(14.0))
                        .bg(colors.text_accent),
                );
                h_flex()
                    .px(px(2.0))
                    .items_center()
                    .whitespace_nowrap()
                    .when(!before.is_empty(), |el| el.child(before))
                    .child(caret)
                    .when(!after.is_empty(), |el| el.child(after))
                    .when(
                        self.buffer.text.is_empty() && !self.placeholder.is_empty(),
                        |el| {
                            el.child(
                                div()
                                    .text_color(colors.text_placeholder)
                                    .child(self.placeholder.clone()),
                            )
                        },
                    )
                    .into_any_element()
            }
        } else if self.buffer.text.is_empty() {
            div()
                .px(px(2.0))
                .whitespace_nowrap()
                .text_color(colors.text_placeholder)
                .child(self.placeholder.clone())
                .into_any_element()
        } else {
            div()
                .px(px(2.0))
                .whitespace_nowrap()
                .text_color(colors.text)
                .child(self.buffer.text.clone())
                .into_any_element()
        };

        h_flex()
            .id(format!("text_field_{:?}", cx.entity_id()))
            .track_focus(&self.focus_handle)
            .w_full()
            .gap_1()
            .items_center()
            .justify_between()
            .when(!self.frameless, |el| {
                el.h(px(26.0))
                    .px(px(6.0))
                    .rounded_md()
                    .bg(bg_color)
                    .border_1()
                    .border_color(border_color)
            })
            .cursor(CursorStyle::IBeam)
            .text_size(px(12.0))
            .text_color(colors.text)
            .when(self.monospace, |el| el.font_buffer(cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    let was_focused = this.focus_handle.is_focused(window);
                    window.focus(&this.focus_handle, cx);
                    if !was_focused && this.select_all_on_focus {
                        this.buffer.select_all();
                    } else {
                        this.buffer.move_end(false);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                if this.focus_handle.is_focused(window) {
                    window.blur(cx);
                }
            }))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(div().flex_1().min_w_0().overflow_hidden().child(content))
            .when_some(self.suffix.clone(), |el, suffix| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(colors.text_muted)
                        .child(suffix),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_buffer_handles_utf8_degree_and_selections() {
        let mut buf = TextBuffer::new("48°50'");
        assert_eq!(buf.cursor, buf.text.len());

        // Backspace removes single character '\'' then '0', '5', '°' without panicking.
        assert!(buf.backspace());
        assert_eq!(buf.text, "48°50");
        buf.move_left(false);
        buf.move_left(false);
        // Cursor is right after "48°" (bytes: '4'=1, '8'=1, '°'=2 -> byte 4).
        assert_eq!(buf.cursor, 4);
        assert!(buf.backspace());
        assert_eq!(buf.text, "4850");

        buf.select_all();
        assert_eq!(buf.selected_text(), "4850");
        assert!(buf.insert_str("42.5\n"));
        assert_eq!(buf.text, "42.5");
        assert!(!buf.has_selection());

        // Shift+Left selects last char, delete_forward / insert replaces cleanly.
        buf.move_left(true);
        assert_eq!(buf.selected_text(), "5");
        assert!(buf.delete_forward());
        assert_eq!(buf.text, "42.");
        buf.move_home(false);
        assert!(buf.insert_str("-"));
        assert_eq!(buf.text, "-42.");
    }

    #[test]
    fn word_navigation_selection_and_deletion_work_across_prose_dms_and_time() {
        let mut buf = TextBuffer::new("Dark Sky Meadow");
        buf.move_word_left(false);
        assert_eq!(&buf.text[buf.cursor..], "Meadow");
        buf.move_word_left(true);
        assert_eq!(buf.selected_text(), "Sky ");
        buf.move_word_left(false);
        assert_eq!(buf.cursor, 0);

        buf.move_word_right(false);
        assert_eq!(&buf.text[..buf.cursor], "Dark");
        buf.move_word_right(true);
        assert_eq!(buf.selected_text(), " Sky");

        // DMS UTF-8 tokens step cleanly across punctuation and digits.
        let mut dms = TextBuffer::new("48°18'30\" N");
        dms.move_word_left(false);
        assert_eq!(&dms.text[dms.cursor..], "N");
        dms.move_word_left(false);
        assert_eq!(&dms.text[dms.cursor..], "\" N");
        assert!(dms.delete_word_backward());
        assert_eq!(dms.text, "48°18'\" N");
        dms.move_home(false);
        assert!(dms.delete_word_forward());
        assert_eq!(dms.text, "°18'\" N");

        // Time separator punctuation cleanly splits HH and MM.
        let mut hm = TextBuffer::new("09:29");
        hm.move_word_left(false);
        assert_eq!(&hm.text[hm.cursor..], "29");
        hm.move_word_left(true);
        assert_eq!(hm.selected_text(), ":");
    }

    #[test]
    fn step_precision_maps_modifiers_to_blender_steps() {
        assert_eq!(
            StepPrecision::from_modifiers(false, false),
            StepPrecision::Normal
        );
        assert_eq!(
            StepPrecision::from_modifiers(true, false),
            StepPrecision::Fine
        );
        assert_eq!(
            StepPrecision::from_modifiers(false, true),
            StepPrecision::Coarse
        );
        // Shift takes precedence when combined with Ctrl.
        assert_eq!(
            StepPrecision::from_modifiers(true, true),
            StepPrecision::Fine
        );

        assert_eq!(StepPrecision::Fine.integer_step(), 1);
        assert_eq!(StepPrecision::Normal.integer_step(), 10);
        assert_eq!(StepPrecision::Coarse.integer_step(), 100);
    }
}
