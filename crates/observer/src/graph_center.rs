use gpui::{
    App, Context, Div, ElementId, Entity, IntoElement, MouseButton, Stateful, div, hsla,
    prelude::*, px, svg,
};
use ui::{ActiveTheme, StyledTypography, Tooltip, h_flex};

pub const ICON_GRAPH_CENTER_NOON: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="4.2" stroke="black" stroke-width="1.65"/>
  <path d="M12 3V5.2M12 18.8V21M3 12H5.2M18.8 12H21M5.6 5.6L7.2 7.2M16.8 16.8L18.4 18.4M18.4 5.6L16.8 7.2M7.2 16.8L5.6 18.4" stroke="black" stroke-width="1.55" stroke-linecap="round"/>
</svg>"##;

pub const ICON_GRAPH_CENTER_MIDNIGHT: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M18.5 14.5A6.8 6.8 0 0 1 9.5 5.5A7.2 7.2 0 1 0 18.5 14.5Z" stroke="black" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##;

/// Global horizontal centering mode shared by all 24-hour graphs
/// (visibility graphs and the observer sky time slider).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VisibilityGraphCenter {
    /// Noon in the middle (`00h` – `12h` – `24h`).
    #[default]
    Noon,
    /// Midnight in the middle (`12h` – `00h` – `12h`).
    Midnight,
}

impl VisibilityGraphCenter {
    /// Reads the active global centering preference (defaults to noon when unset).
    pub fn current(cx: &App) -> Self {
        cx.try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.read(cx).center())
            .unwrap_or_default()
    }

    pub fn start_hour(self) -> f64 {
        match self {
            Self::Noon => 0.0,
            Self::Midnight => 12.0,
        }
    }

    /// Computes the continuous 24-hour window start timestamp (Unix seconds) containing
    /// the observer's active local time.
    /// - `Noon`: `[00:00 today .. 24:00 today]`
    /// - `Midnight`: `[12:00 today .. 12:00 tomorrow]` when `local_hour_of_day >= 12.0`,
    ///   or `[12:00 yesterday .. 12:00 today]` when `local_hour_of_day < 12.0`.
    pub fn window_start_unix_seconds(
        self,
        local_midnight_unix_secs: f64,
        local_hour_of_day: f64,
    ) -> f64 {
        match self {
            Self::Noon => local_midnight_unix_secs,
            Self::Midnight => {
                if local_hour_of_day >= 12.0 {
                    local_midnight_unix_secs + 12.0 * 3600.0
                } else {
                    local_midnight_unix_secs - 12.0 * 3600.0
                }
            }
        }
    }

    /// Converts a window-relative hour offset `[0..24]` into local clock hour `[0..24)`.
    pub fn clock_hour(self, offset_hour: f64) -> f64 {
        (offset_hour + self.start_hour()).rem_euclid(24.0)
    }

    #[must_use]
    pub fn toggle(self) -> Self {
        match self {
            Self::Noon => Self::Midnight,
            Self::Midnight => Self::Noon,
        }
    }

    pub fn axis_labels(self) -> [&'static str; 5] {
        match self {
            Self::Noon => ["00h", "06h", "12h", "18h", "24h"],
            Self::Midnight => ["12h", "18h", "00h", "06h", "12h"],
        }
    }

    pub fn icon(self) -> &'static [u8] {
        match self {
            Self::Noon => ICON_GRAPH_CENTER_NOON,
            Self::Midnight => ICON_GRAPH_CENTER_MIDNIGHT,
        }
    }

    pub fn tooltip_text(self) -> &'static str {
        match self {
            Self::Noon => "Centered on noon (00h–24h) · Click to center on midnight",
            Self::Midnight => "Centered on midnight (12h–12h) · Click to center on noon",
        }
    }
}

#[derive(Default)]
pub struct VisibilityGraphPrefsModel {
    center: VisibilityGraphCenter,
}

impl VisibilityGraphPrefsModel {
    pub fn center(&self) -> VisibilityGraphCenter {
        self.center
    }

    pub fn toggle_center(&mut self, cx: &mut Context<Self>) {
        self.center = self.center.toggle();
        cx.notify();
    }
}

#[derive(Clone)]
pub struct GlobalVisibilityGraphPrefs(pub Entity<VisibilityGraphPrefsModel>);

impl gpui::Global for GlobalVisibilityGraphPrefs {}

/// Visual treatment of [`graph_center_toggle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphCenterToggleStyle {
    /// Small translucent dark chip meant to be positioned over a graph canvas.
    Overlay,
    /// Themed bordered button meant to sit in a toolbar row.
    Inline,
}

/// Button toggling the global noon/midnight graph centering. Callers position it
/// (e.g. `.absolute().top().left()` for [`GraphCenterToggleStyle::Overlay`]).
pub fn graph_center_toggle(
    id: impl Into<ElementId>,
    style: GraphCenterToggleStyle,
    cx: &App,
) -> Stateful<Div> {
    let center = VisibilityGraphCenter::current(cx);
    let colors = cx.theme().colors();

    let (size, icon_size, icon_color) = match style {
        GraphCenterToggleStyle::Overlay => (20.0, 12.5, hsla(0.0, 0.0, 1.0, 0.78)),
        GraphCenterToggleStyle::Inline => (22.0, 12.5, colors.text_muted),
    };

    let base = div()
        .id(id)
        .w(px(size))
        .h(px(size))
        .flex_none()
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(Tooltip::text(center.tooltip_text()));

    let base = match style {
        GraphCenterToggleStyle::Overlay => base
            .bg(hsla(0.0, 0.0, 0.0, 0.28))
            .hover(|h| h.bg(hsla(0.0, 0.0, 0.0, 0.52)))
            // Keep clicks from reaching a scrubbable canvas underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        GraphCenterToggleStyle::Inline => {
            let hover = colors.element_hover;
            base.border_1()
                .bg(colors.element_background.opacity(0.45))
                .border_color(colors.border_variant)
                .hover(move |s| s.bg(hover))
        }
    };

    base.on_click(|_, _, cx| {
        if let Some(prefs) = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.clone())
        {
            prefs.update(cx, |model, cx| model.toggle_center(cx));
        }
    })
    .child(
        svg()
            .data(center.icon())
            .size(px(icon_size))
            .flex_none()
            .text_color(icon_color),
    )
}

/// Five-label hour axis row (`00h … 24h` or `12h … 12h`) under a 24-hour graph.
pub fn graph_axis_labels(center: VisibilityGraphCenter, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .px_0p5()
        .font_buffer(cx)
        .text_size(px(9.5))
        .text_color(cx.theme().colors().text_muted)
        .children(center.axis_labels())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midnight_window_starts_at_noon_of_day_n() {
        let midnight = 1_000_000.0;
        let c = VisibilityGraphCenter::Midnight;
        // Afternoon: window is [12:00 today .. 12:00 tomorrow].
        assert_eq!(
            c.window_start_unix_seconds(midnight, 15.0),
            midnight + 43_200.0
        );
        // Early morning: window is [12:00 yesterday .. 12:00 today].
        assert_eq!(
            c.window_start_unix_seconds(midnight, 3.0),
            midnight - 43_200.0
        );
        assert_eq!(
            VisibilityGraphCenter::Noon.window_start_unix_seconds(midnight, 3.0),
            midnight
        );
    }
}
