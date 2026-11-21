use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Render, Rgba, Size, Window, div,
    fill, hsla, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, size,
};
use ui::{ActiveTheme, StyledTypography, h_flex, v_flex};

/// Events emitted by [`SkyTimeSlider`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SkyTimeSliderEvent {
    /// Emitted when the user clicks or drags across the 24-hour sky ribbon.
    Scrub { hour_of_day: f64 },
}

/// Interactive 24-hour solar-twilight sky arc scrubber.
pub struct SkyTimeSlider {
    hour_of_day: f64,
    is_live: bool,
    solar_profile: Vec<f64>,
    is_dragging: bool,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
}

impl EventEmitter<SkyTimeSliderEvent> for SkyTimeSlider {}

impl SkyTimeSlider {
    pub fn new(hour_of_day: f64, is_live: bool, solar_profile: Vec<f64>) -> Self {
        Self {
            hour_of_day: hour_of_day.clamp(0.0, 23.9997),
            is_live,
            solar_profile,
            is_dragging: false,
            track_bounds: Arc::new(Mutex::new(None)),
        }
    }

    pub fn build(
        hour_of_day: f64,
        is_live: bool,
        solar_profile: Vec<f64>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|_cx| Self::new(hour_of_day, is_live, solar_profile))
    }

    /// Updates the displayed sky curve and playhead without interrupting active dragging.
    pub fn sync(
        &mut self,
        hour_of_day: f64,
        is_live: bool,
        solar_profile: Vec<f64>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.is_live != is_live || self.solar_profile != solar_profile;
        self.is_live = is_live;
        self.solar_profile = solar_profile;
        if !self.is_dragging {
            let clamped = hour_of_day.clamp(0.0, 23.9997);
            if (self.hour_of_day - clamped).abs() > 1e-4 {
                self.hour_of_day = clamped;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn scrub_to_position(
        &mut self,
        pos: Point<Pixels>,
        shift_held: bool,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return;
        }
        let t = ((f32::from(pos.x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0) as f64;
        let step_mins = if shift_held { 15.0 } else { 1.0 };
        let total_mins = (t * 24.0 * 60.0 / step_mins).round() * step_mins;
        let snapped_hour = (total_mins / 60.0).clamp(0.0, 23.0 + 59.0 / 60.0);

        if (self.hour_of_day - snapped_hour).abs() > 1e-5 || self.is_live {
            self.hour_of_day = snapped_hour;
            self.is_live = false;
            cx.emit(SkyTimeSliderEvent::Scrub {
                hour_of_day: snapped_hour,
            });
            cx.notify();
        }
    }
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Maps solar geometric altitude in degrees to the continuous sky-ribbon RGB tint.
pub fn solar_altitude_to_sky_color(alt_deg: f64) -> Rgba {
    const STOPS: &[(f64, u32)] = &[
        (-24.0, 0x070b16),
        (-18.0, 0x0c1428),
        (-12.0, 0x172b54),
        (-6.0, 0x2b4775),
        (-2.0, 0x8e4c3a),
        (1.5, 0xc77d44),
        (9.0, 0x2f6899),
        (45.0, 0x3b7db6),
    ];

    if alt_deg <= STOPS[0].0 {
        return rgb(STOPS[0].1);
    }
    for pair in STOPS.windows(2) {
        let (a0, c0) = pair[0];
        let (a1, c1) = pair[1];
        if alt_deg <= a1 {
            let t = ((alt_deg - a0) / (a1 - a0)) as f32;
            return lerp_rgba(rgb(c0), rgb(c1), t);
        }
    }
    rgb(STOPS[STOPS.len() - 1].1)
}

fn alt_to_y(alt_deg: f64, bounds: Bounds<Pixels>) -> f32 {
    const MIN_ALT: f64 = -65.0;
    const MAX_ALT: f64 = 75.0;
    let pad = 6.0_f32;
    let top = f32::from(bounds.origin.y) + pad;
    let usable_h = (f32::from(bounds.size.height) - 2.0 * pad).max(4.0);
    let norm = ((alt_deg - MIN_ALT) / (MAX_ALT - MIN_ALT)).clamp(0.0, 1.0) as f32;
    top + usable_h * (1.0 - norm)
}

/// Paints a filled antialiased-polygon circle at `center`.
pub fn paint_circle(window: &mut Window, center: Point<Pixels>, radius: f32, color: Rgba) {
    const SEGMENTS: usize = 32;
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let mut points = [Point::default(); SEGMENTS];
    for (i, pt) in points.iter_mut().enumerate() {
        let angle = (i as f32) * (std::f32::consts::TAU / SEGMENTS as f32);
        let (sin_a, cos_a) = angle.sin_cos();
        *pt = point(px(cx + radius * cos_a), px(cy + radius * sin_a));
    }
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Linearly interpolates a daily `[0h..24h]` altitude profile at `hour_of_day`.
pub fn sample_day_profile(profile: &[f64], hour_of_day: f64) -> f64 {
    if profile.is_empty() {
        return -20.0;
    }
    if profile.len() == 1 {
        return profile[0];
    }
    let frac = (hour_of_day / 24.0).clamp(0.0, 1.0) * (profile.len() - 1) as f64;
    let idx = (frac.floor() as usize).min(profile.len() - 2);
    let t = frac - idx as f64;
    profile[idx] * (1.0 - t) + profile[idx + 1] * t
}

/// Paints the 24-hour twilight atmospheric sky gradient and subtle darkness starfield specks.
pub fn paint_twilight_backdrop(bounds: Bounds<Pixels>, solar_profile: &[f64], window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    // 1. Continuous per-pixel atmospheric twilight sky gradient across 96 piecewise linear spans.
    window.paint_quad(fill(bounds, rgb(0x070b16)));
    const RIBBON_SLICES: usize = 96;
    for i in 0..RIBBON_SLICES {
        let t0 = i as f32 / RIBBON_SLICES as f32;
        let t1 = (i + 1) as f32 / RIBBON_SLICES as f32;
        let x0 = origin_x + t0 * width;
        let x1 = origin_x + t1 * width;
        if x1 <= x0 {
            continue;
        }
        let c0 =
            solar_altitude_to_sky_color(sample_day_profile(solar_profile, f64::from(t0) * 24.0));
        let c1 =
            solar_altitude_to_sky_color(sample_day_profile(solar_profile, f64::from(t1) * 24.0));
        let strip_bounds = Bounds::new(
            point(px(x0), px(origin_y)),
            Size::new(px(x1 - x0), px(height)),
        );
        window.paint_quad(fill(
            strip_bounds,
            linear_gradient(
                90.0,
                linear_color_stop(Hsla::from(c0), 0.0),
                linear_color_stop(Hsla::from(c1), 1.0),
            ),
        ));
    }

    // 2. Subtle starfield specks that emerge during astronomical twilight and darkness.
    const STARS: &[(f32, f32, f32)] = &[
        (0.06, 0.22, 1.5),
        (0.11, 0.44, 1.2),
        (0.16, 0.16, 1.8),
        (0.21, 0.34, 1.2),
        (0.28, 0.20, 1.4),
        (0.35, 0.42, 1.2),
        (0.46, 0.18, 1.4),
        (0.58, 0.26, 1.2),
        (0.71, 0.24, 1.4),
        (0.79, 0.40, 1.6),
        (0.86, 0.18, 1.8),
        (0.92, 0.32, 1.3),
        (0.96, 0.20, 1.5),
    ];
    for &(xf, yf, sz) in STARS {
        let alt = sample_day_profile(solar_profile, f64::from(xf) * 24.0);
        if alt < -9.0 {
            let fade = (((-9.0 - alt) / 9.0).clamp(0.0, 1.0) * 0.75) as f32;
            let star_bounds = Bounds::new(
                point(px(origin_x + xf * width), px(origin_y + yf * height)),
                size(px(sz), px(sz)),
            );
            window.paint_quad(fill(star_bounds, rgb(0xe8f1ff).alpha(fade)));
        }
    }
}

fn paint_sky_canvas(
    bounds: Bounds<Pixels>,
    hour_of_day: f64,
    solar_profile: &[f64],
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    paint_twilight_backdrop(bounds, solar_profile, window);

    // 3. Horizon (0°) and astro-dark (-18°) reference lines.
    let horizon_y = alt_to_y(0.0, bounds);
    window.paint_quad(fill(
        Bounds::new(point(px(origin_x), px(horizon_y)), size(px(width), px(1.0))),
        hsla(0.0, 0.0, 1.0, 0.22),
    ));

    let astro_y = alt_to_y(-18.0, bounds);
    window.paint_quad(fill(
        Bounds::new(point(px(origin_x), px(astro_y)), size(px(width), px(1.0))),
        hsla(0.58, 0.7, 0.75, 0.16),
    ));

    // 4. Solar elevation curve (cool blue baseline + warm amber highlight above horizon).
    if solar_profile.len() >= 2 {
        let denom = (solar_profile.len() - 1) as f32;
        let curve_pt = |i: usize| -> Point<Pixels> {
            let x = origin_x + (i as f32 / denom) * width;
            let y = alt_to_y(solar_profile[i], bounds);
            point(px(x), px(y))
        };

        let mut base_path = PathBuilder::stroke(px(1.6));
        base_path.move_to(curve_pt(0));
        for i in 1..solar_profile.len() {
            base_path.line_to(curve_pt(i));
        }
        if let Ok(path) = base_path.build() {
            window.paint_path(path, rgb(0x6eb8f7).alpha(0.62));
        }

        // Overlay daylight segments above 0° in warm solar gold.
        let mut day_path: Option<PathBuilder> = None;
        for i in 1..solar_profile.len() {
            let a0 = solar_profile[i - 1];
            let a1 = solar_profile[i];
            let p0 = curve_pt(i - 1);
            let p1 = curve_pt(i);

            if a0 >= 0.0 && a1 >= 0.0 {
                let builder = day_path.get_or_insert_with(|| {
                    let mut b = PathBuilder::stroke(px(2.0));
                    b.move_to(p0);
                    b
                });
                builder.line_to(p1);
            } else if a0 < 0.0 && a1 >= 0.0 {
                let frac = ((-a0) / (a1 - a0).max(1e-6)) as f32;
                let cross = point(
                    px(f32::from(p0.x) + (f32::from(p1.x) - f32::from(p0.x)) * frac),
                    px(horizon_y),
                );
                let builder = day_path.get_or_insert_with(|| {
                    let mut b = PathBuilder::stroke(px(2.0));
                    b.move_to(cross);
                    b
                });
                builder.line_to(p1);
            } else if a0 >= 0.0 && a1 < 0.0 {
                let frac = (a0 / (a0 - a1).max(1e-6)) as f32;
                let cross = point(
                    px(f32::from(p0.x) + (f32::from(p1.x) - f32::from(p0.x)) * frac),
                    px(horizon_y),
                );
                if let Some(mut builder) = day_path.take() {
                    builder.line_to(cross);
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, rgb(0xf4ad62).alpha(0.95));
                    }
                }
            }
        }
        if let Some(builder) = day_path {
            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(0xf4ad62).alpha(0.95));
            }
        }
    }

    // 5. Subtle playhead beam and fixed-size bordered orb filled with the active sky color.
    let t = (hour_of_day / 24.0).clamp(0.0, 1.0) as f32;
    let playhead_x = origin_x + t * width;
    let current_alt = sample_day_profile(solar_profile, hour_of_day);
    let handle_y = alt_to_y(current_alt, bounds);
    let handle_pt = point(px(playhead_x), px(handle_y));
    let sky_color = solar_altitude_to_sky_color(current_alt);

    let mut beam_builder = PathBuilder::stroke(px(1.0));
    beam_builder.move_to(point(px(playhead_x), px(origin_y)));
    beam_builder.line_to(point(px(playhead_x), px(origin_y + height)));
    if let Ok(beam_path) = beam_builder.build() {
        window.paint_path(beam_path, hsla(0.0, 0.0, 1.0, 0.35));
    }

    paint_circle(window, handle_pt, 5.5, rgb(0x090d1a).alpha(0.86));
    paint_circle(window, handle_pt, 4.6, rgb(0xf2f6fc).alpha(0.96));
    paint_circle(window, handle_pt, 3.0, sky_color);
}

impl Render for SkyTimeSlider {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let bounds_store = self.track_bounds.clone();
        let entity = cx.entity().clone();
        let hour_of_day = self.hour_of_day;
        let solar_profile = self.solar_profile.clone();

        let sky_canvas = gpui::canvas(
            move |bounds, _window, _cx| {
                if let Ok(mut guard) = bounds_store.lock() {
                    *guard = Some(bounds);
                }
            },
            move |bounds, (), window, _cx| {
                paint_sky_canvas(bounds, hour_of_day, &solar_profile, window);

                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        e.update(cx, |this, cx| {
                            if this.is_dragging {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    this.scrub_to_position(
                                        event.position,
                                        event.modifiers.shift,
                                        bounds,
                                        cx,
                                    );
                                } else {
                                    this.is_dragging = false;
                                    cx.notify();
                                }
                            }
                        });
                    }
                });

                let e = entity.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        e.update(cx, |this, cx| {
                            if this.is_dragging {
                                this.is_dragging = false;
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .size_full();

        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .id(format!("sky_time_slider_{:?}", cx.entity_id()))
                    .w_full()
                    .h(px(66.0))
                    .rounded_lg()
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors.border_variant)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            let bounds = this.track_bounds.lock().ok().and_then(|g| *g);
                            if let Some(bounds) = bounds {
                                this.is_dragging = true;
                                this.scrub_to_position(
                                    event.position,
                                    event.modifiers.shift,
                                    bounds,
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(sky_canvas),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .px_0p5()
                    .font_buffer(cx)
                    .text_size(px(9.5))
                    .text_color(colors.text_muted)
                    .child("00h")
                    .child("06h")
                    .child("12h")
                    .child("18h")
                    .child("24h"),
            )
    }
}
