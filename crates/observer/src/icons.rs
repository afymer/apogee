use gpui::{Hsla, IntoElement, Pixels, Styled, svg};
use persistence::ObservatoryIcon;

/// Astronomical observatory dome with slit and protruding telescope tube.
pub const ICON_DOME: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M4 20H20" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M6 20V14.5H18V20" stroke="black" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M5 14.5A7 7 0 0 1 19 14.5H5Z" stroke="black" stroke-width="1.6" stroke-linejoin="round"/>
  <path d="M12.5 11L17.5 6" stroke="black" stroke-width="1.8" stroke-linecap="round"/>
  <path d="M16.2 4.8L18.8 7.4" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M5 4.5V7.5M3.5 6H6.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

/// Refracting telescope on a tripod over rolling meadow hills.
pub const ICON_FIELD: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 19.5C6.5 17 10 18.8 13.5 18.2C16.8 17.6 19.2 18.6 21.5 17.8" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M12 12L8.5 19M12 12L15.5 19M12 12V18.2" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M7 11.2L17.5 6.8" stroke="black" stroke-width="2.1" stroke-linecap="round"/>
  <path d="M5.5 11.8L7.4 11" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <circle cx="5.5" cy="5.5" r="0.9" fill="black"/>
  <circle cx="19.5" cy="11.5" r="0.8" fill="black"/>
</svg>"##;

/// High-altitude twin alpine peaks under a crescent moon.
pub const ICON_MOUNTAIN: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2 20L8.5 8.5L13 15.8L16.5 10.5L22 20H2Z" stroke="black" stroke-width="1.6" stroke-linejoin="round"/>
  <path d="M6.4 12.4L8.5 13.8L10.4 12.2" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M18.6 4.2A2.6 2.6 0 1 0 21.4 7.4A2.1 2.1 0 0 1 18.6 4.2Z" stroke="black" stroke-width="1.4" stroke-linejoin="round"/>
  <circle cx="5.5" cy="5.5" r="0.9" fill="black"/>
</svg>"##;

/// Architectural rooftop parapet terrace with mounted telescope.
pub const ICON_ROOFTOP: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 20H21" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M4.5 20V15.5H19.5V20" stroke="black" stroke-width="1.6" stroke-linejoin="round"/>
  <path d="M8 15.5V17.5M16 15.5V17.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <path d="M12 15.5V11.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M8 12.5L16.5 8.2" stroke="black" stroke-width="2" stroke-linecap="round"/>
  <path d="M6.5 5.5V7.5M5.5 6.5H7.5" stroke="black" stroke-width="1.3" stroke-linecap="round"/>
</svg>"##;

/// Balcony balustrade railing with compact mounted telescope.
pub const ICON_BALCONY: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 14H21M3 19.5H21" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M5 14V19.5M9 14V19.5M13 14V19.5M17 14V19.5M19.5 14V19.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <path d="M11 14V10.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M7.5 11L16 6.8" stroke="black" stroke-width="2" stroke-linecap="round"/>
  <circle cx="18.5" cy="5.5" r="0.9" fill="black"/>
</svg>"##;

/// Remote observatory dome with live radio telemetry arcs.
pub const ICON_REMOTE: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3.5 20H14.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M5 20V15.5H13V20" stroke="black" stroke-width="1.5" stroke-linejoin="round"/>
  <path d="M4.5 15.5A4.5 4.5 0 0 1 13.5 15.5H4.5Z" stroke="black" stroke-width="1.5" stroke-linejoin="round"/>
  <path d="M9 11V8.5" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <circle cx="9" cy="7.5" r="1" fill="black"/>
  <path d="M14.5 9A4.5 4.5 0 0 1 18.5 13" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M15 5.5A8 8 0 0 1 21.5 13" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
</svg>"##;

/// Sun micro-icon for daylight phase.
pub const ICON_SUN: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="4.2" stroke="black" stroke-width="1.7"/>
  <path d="M12 2.5V5M12 19V21.5M2.5 12H5M19 12H21.5M5.3 5.3L7.1 7.1M16.9 16.9L18.7 18.7M18.7 5.3L16.9 7.1M7.1 16.9L5.3 18.7" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
</svg>"##;

/// Crescent moon and star micro-icon for astronomical night.
pub const ICON_MOON: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5A7.5 7.5 0 1 0 19.5 14.5Z" stroke="black" stroke-width="1.7" stroke-linejoin="round"/>
  <path d="M17.5 4.5V7.5M16 6H19" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
</svg>"##;

/// Sun sinking below the horizon micro-icon for twilight phases.
pub const ICON_TWILIGHT: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 16.5H21" stroke="black" stroke-width="1.7" stroke-linecap="round"/>
  <path d="M7.5 16.5A4.5 4.5 0 0 1 16.5 16.5" stroke="black" stroke-width="1.7"/>
  <path d="M12 6.5V9M5.8 9.2L7.5 11M18.2 9.2L16.5 11" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M6.5 20H17.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

/// Latitude globe parallels micro-icon.
pub const ICON_LATITUDE: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="9.5" stroke="black" stroke-width="2"/>
  <path d="M3.2 8.5H20.8M3.2 15.5H20.8" stroke="black" stroke-width="2" stroke-linecap="round"/>
</svg>"##;

/// Longitude globe meridians micro-icon.
pub const ICON_LONGITUDE: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="9.5" stroke="black" stroke-width="2"/>
  <ellipse cx="12" cy="12" rx="4" ry="9.5" stroke="black" stroke-width="2"/>
</svg>"##;

/// Elevation mountain-altitude micro-icon.
pub const ICON_ELEVATION: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 19.5L9.5 9L14 15.5L17 11.5L21 19.5H3Z" stroke="black" stroke-width="1.8" stroke-linejoin="round"/>
  <path d="M18.5 8.5V4M18.5 4L16.5 6M18.5 4L20.5 6" stroke="black" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##;

/// Custom horizon ridge silhouette micro-icon.
pub const ICON_HORIZON: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 16.5L7.5 11.5L11 14.5L15.5 9L21.5 16.5" stroke="black" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M2.5 19H21.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <circle cx="7" cy="6.5" r="1.1" fill="black"/>
</svg>"##;

/// Returns the embedded SVG bytes for a given [`ObservatoryIcon`].
pub fn observatory_icon_svg(icon: ObservatoryIcon) -> &'static [u8] {
    match icon {
        ObservatoryIcon::Dome => ICON_DOME,
        ObservatoryIcon::Field => ICON_FIELD,
        ObservatoryIcon::Mountain => ICON_MOUNTAIN,
        ObservatoryIcon::Rooftop => ICON_ROOFTOP,
        ObservatoryIcon::Balcony => ICON_BALCONY,
        ObservatoryIcon::Remote => ICON_REMOTE,
    }
}

/// Renders an embedded monochrome SVG with explicit dimensions and tint color.
pub fn render_svg_icon(bytes: &'static [u8], size: Pixels, color: Hsla) -> impl IntoElement {
    svg().data(bytes).size(size).flex_none().text_color(color)
}
