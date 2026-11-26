use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use celestial::coords::{Angle, canonical_spherical_to_cartesian};
use celestial::equatorial_az_alt_deg;
use celestial::solar::{lunar_ephemeris, solar_altitude_deg};
use client::GlobalDaemonState;
use glam::DVec3;
use gpui::{
    App, Bounds, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Pixels, Render, Window, canvas, div, fill, hsla, point, prelude::*, px, rgb,
    size,
};
use hifitime::Epoch;
use horizon::GlobalHorizons;
use jiff::Span;
use jiff::civil::Date;
use jiff::tz::Offset;
use observer::icons::{ICON_MOON, render_svg_icon};
use observer::{GlobalObserver, solar_altitude_to_sky_color};
use ui::{ActiveTheme, Button, ButtonStyle, Color, Label, LabelSize, Tooltip, prelude::*};

use crate::state::{
    GlobalNightPlan, format_duration_compact, format_unix_hm, is_session_locked,
};

const GRAPH_SAMPLES: usize = 97;
const GRAPH_HEIGHT_PX: f32 = 190.0;
const BLOCK_TRACK_HEIGHT_PX: f32 = 34.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TimelineWindowMode {
    #[default]
    Night16h,
    FullDay24h,
}

impl TimelineWindowMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Night16h => "16h",
            Self::FullDay24h => "24h",
        }
    }

    #[must_use]
    pub fn toggle(self) -> Self {
        match self {
            Self::Night16h => Self::FullDay24h,
            Self::FullDay24h => Self::Night16h,
        }
    }
}

/// Computes `(window_start_unix, window_end_unix)` centered on the midnight following evening `night_date`.
pub fn compute_night_window_unix_secs(
    night_date: &str,
    utc_offset_secs: i32,
    fallback_midnight_unix: f64,
    mode: TimelineWindowMode,
) -> (f64, f64) {
    let center_midnight = night_date
        .trim()
        .parse::<Date>()
        .ok()
        .and_then(|evening| evening.checked_add(Span::new().days(1)).ok())
        .and_then(|next_day| {
            let offset = Offset::from_seconds(utc_offset_secs).ok()?;
            let dt = next_day.at(0, 0, 0, 0);
            let zoned = dt.to_zoned(offset.to_time_zone()).ok()?;
            Some(zoned.timestamp().as_second() as f64)
        })
        .unwrap_or(fallback_midnight_unix);

    let half_span = match mode {
        TimelineWindowMode::Night16h => 8.0 * 3600.0,
        TimelineWindowMode::FullDay24h => 12.0 * 3600.0,
    };
    (center_midnight - half_span, center_midnight + half_span)
}

/// Snaps `unix_secs` to `step_secs` and clamps to `[window_start, window_end - 60]`.
pub fn snap_unix_secs(unix_secs: f64, step_secs: i64, window_start: f64, window_end: f64) -> i64 {
    let step = f64::from(step_secs.max(1) as i32);
    let snapped = (unix_secs / step).round() * step;
    let min_s = window_start.round() as i64;
    let max_s = (window_end.round() as i64).saturating_sub(60).max(min_s);
    (snapped.round() as i64).clamp(min_s, max_s)
}

pub fn angular_separation_deg(a: DVec3, b: DVec3) -> f64 {
    a.normalize_or_zero()
        .dot(b.normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

#[derive(Clone, PartialEq, Eq)]
struct SampleCacheKey {
    plan_revision: u64,
    night_date: String,
    lat_bits: u64,
    lon_bits: u64,
    elev_bits: u64,
    utc_offset_secs: i32,
    horizon_key: Option<(String, u64)>,
    mode: TimelineWindowMode,
}

#[derive(Clone)]
struct TargetCurve {
    target_id: String,
    hue_deg: f32,
    alts: Vec<f64>,
    visible: Vec<bool>,
    moon_sep_deg: Vec<f64>,
    meridian_transit_idx: Option<usize>,
    min_moon_sep_deg: f64,
}

struct SampledNightData {
    window_start_secs: f64,
    window_end_secs: f64,
    solar_alts: Vec<f64>,
    moon_alts: Vec<f64>,
    moon_illum_pct: f64,
    targets: Vec<TargetCurve>,
}

enum TimelineDrag {
    MoveBlocks {
        base_starts: Vec<(String, i64)>,
        anchor_cursor_unix: i64,
    },
    MarqueeSelect {
        start_x: f32,
        current_x: f32,
    },
}

pub struct NightTimelineView {
    mode: TimelineWindowMode,
    show_moon_sep: bool,
    cache_key: Option<SampleCacheKey>,
    sampled: Option<Arc<SampledNightData>>,
    track_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    drag: Option<TimelineDrag>,
}

impl NightTimelineView {
    pub fn build(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                cx.observe(&plan, |this: &mut Self, _m, cx| {
                    this.ensure_samples_fresh(cx);
                    cx.notify();
                })
                .detach();
            }
            if let Some(obs) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
                cx.observe(&obs, |this: &mut Self, _m, cx| {
                    this.ensure_samples_fresh(cx);
                    cx.notify();
                })
                .detach();
            }
            if let Some(hor) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
                cx.observe(&hor, |this: &mut Self, _m, cx| {
                    this.ensure_samples_fresh(cx);
                    cx.notify();
                })
                .detach();
            }
            if let Some(daemon) = cx.try_global::<GlobalDaemonState>().map(|g| g.0.clone()) {
                cx.observe(&daemon, |_this: &mut Self, _m, cx| {
                    cx.notify();
                })
                .detach();
            }

            let mut view = Self {
                mode: TimelineWindowMode::Night16h,
                show_moon_sep: true,
                cache_key: None,
                sampled: None,
                track_bounds: Arc::new(Mutex::new(None)),
                drag: None,
            };
            view.ensure_samples_fresh(cx);
            view
        })
    }

    fn ensure_samples_fresh(&mut self, cx: &App) {
        let Some(plan_model) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.read(cx)) else {
            return;
        };
        let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.read(cx)) else {
            return;
        };

        let plan = plan_model.plan();
        let loc = observer.geodetic_position();
        let utc_offset_secs = observer.effective_utc_offset_secs();
        let horizon_info = observer.active_horizon_id().and_then(|id| {
            let horizons = cx.try_global::<GlobalHorizons>()?.0.read(cx);
            let prof = horizons.profile_for(id)?;
            Some(((id.to_string(), horizons.live_generation()), prof))
        });

        let next_key = SampleCacheKey {
            plan_revision: plan_model.revision(),
            night_date: plan.night_date.clone(),
            lat_bits: loc.latitude_deg().to_bits(),
            lon_bits: loc.longitude_deg().to_bits(),
            elev_bits: loc.elevation_m().to_bits(),
            utc_offset_secs,
            horizon_key: horizon_info.as_ref().map(|(k, _)| k.clone()),
            mode: self.mode,
        };

        if self.cache_key.as_ref() == Some(&next_key) {
            return;
        }

        let fallback_midnight = observer.local_midnight_unix_seconds();
        let (window_start_secs, window_end_secs) = compute_night_window_unix_secs(
            &plan.night_date,
            utc_offset_secs,
            fallback_midnight,
            self.mode,
        );
        let horizon_profile = horizon_info.map(|(_, p)| p);

        let denom = (GRAPH_SAMPLES - 1) as f64;
        let span = (window_end_secs - window_start_secs).max(60.0);

        let mut solar_alts = Vec::with_capacity(GRAPH_SAMPLES);
        let mut moon_alts = Vec::with_capacity(GRAPH_SAMPLES);
        let mut moon_dirs = Vec::with_capacity(GRAPH_SAMPLES);
        let mut epochs = Vec::with_capacity(GRAPH_SAMPLES);

        for i in 0..GRAPH_SAMPLES {
            let t = window_start_secs + (i as f64 / denom) * span;
            let epoch = Epoch::from_unix_seconds(t);
            epochs.push(epoch);
            solar_alts.push(solar_altitude_deg(loc, epoch));
            let moon = lunar_ephemeris(loc, epoch);
            moon_alts.push(moon.alt_deg);
            moon_dirs.push(
                canonical_spherical_to_cartesian(
                    Angle::from_degrees(moon.ra_deg),
                    Angle::from_degrees(moon.dec_deg),
                )
                .normalize_or_zero(),
            );
        }

        let mid_epoch = Epoch::from_unix_seconds(f64::midpoint(window_start_secs, window_end_secs));
        let moon_illum_pct = lunar_ephemeris(loc, mid_epoch).illuminated_fraction * 100.0;

        let mut targets = Vec::with_capacity(plan.targets.len());
        for target in &plan.targets {
            let ra = Angle::from_degrees(target.ra_deg);
            let dec = Angle::from_degrees(target.dec_deg);
            let target_dir = canonical_spherical_to_cartesian(ra, dec).normalize_or_zero();

            let mut alts = Vec::with_capacity(GRAPH_SAMPLES);
            let mut visible = Vec::with_capacity(GRAPH_SAMPLES);
            let mut moon_sep_deg = Vec::with_capacity(GRAPH_SAMPLES);
            let mut max_alt = f64::NEG_INFINITY;
            let mut max_idx = None;
            let mut min_moon_sep_deg = 180.0_f64;

            for (i, &epoch) in epochs.iter().enumerate() {
                let (az, alt) = equatorial_az_alt_deg(loc, epoch, ra, dec);
                let is_vis = horizon_profile
                    .as_ref()
                    .map_or(alt >= 0.0, |h| alt >= h.altitude_at_azimuth_deg(az));
                let sep = angular_separation_deg(target_dir, moon_dirs[i]);
                min_moon_sep_deg = min_moon_sep_deg.min(sep);
                if alt > max_alt {
                    max_alt = alt;
                    max_idx = Some(i);
                }
                alts.push(alt);
                visible.push(is_vis);
                moon_sep_deg.push(sep);
            }

            targets.push(TargetCurve {
                target_id: target.id.clone(),
                hue_deg: target.color_hue_deg,
                alts,
                visible,
                moon_sep_deg,
                meridian_transit_idx: max_idx.filter(|_| max_alt > 0.0),
                min_moon_sep_deg,
            });
        }

        self.sampled = Some(Arc::new(SampledNightData {
            window_start_secs,
            window_end_secs,
            solar_alts,
            moon_alts,
            moon_illum_pct,
            targets,
        }));
        self.cache_key = Some(next_key);
    }

    fn alt_to_y(alt_deg: f64, bounds: Bounds<Pixels>) -> f32 {
        let top = f32::from(bounds.origin.y) + 6.0;
        let h = (f32::from(bounds.size.height) - 12.0).max(10.0);
        let norm = (alt_deg / 90.0).clamp(0.0, 1.0) as f32;
        top + h * (1.0 - norm)
    }

    fn paint_graph(
        sampled: &SampledNightData,
        primary_selected_id: Option<&str>,
        selected_ids: &HashSet<String>,
        show_moon_sep: bool,
        now_unix_secs: Option<f64>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w <= 4.0 || h <= 4.0 {
            return;
        }
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);

        // 1. Twilight background slices
        let n_slices = sampled.solar_alts.len().saturating_sub(1).max(1);
        let slice_w = w / n_slices as f32;
        for i in 0..n_slices {
            let mid_alt = f64::midpoint(sampled.solar_alts[i], sampled.solar_alts[i + 1]);
            let col = solar_altitude_to_sky_color(mid_alt);
            let x = origin_x + i as f32 * slice_w;
            window.paint_quad(fill(
                Bounds::new(point(px(x), px(origin_y)), size(px(slice_w + 0.6), px(h))),
                col,
            ));
        }

        // 2. Reference altitude lines (0, 30, 60 deg)
        for &ref_alt in &[0.0, 30.0, 60.0] {
            let y = Self::alt_to_y(ref_alt, bounds);
            window.paint_quad(fill(
                Bounds::new(point(px(origin_x), px(y)), size(px(w), px(1.0))),
                hsla(0.0, 0.0, 1.0, if ref_alt == 0.0 { 0.22 } else { 0.10 }),
            ));
        }

        // 3. Vertical 2-hour gridlines
        let span_hours = ((sampled.window_end_secs - sampled.window_start_secs) / 3600.0).max(1.0);
        let step_hours = 2.0_f64;
        let mut h_off = step_hours;
        while h_off < span_hours {
            let x = origin_x + (h_off / span_hours) as f32 * w;
            window.paint_quad(fill(
                Bounds::new(point(px(x), px(origin_y)), size(px(1.0), px(h))),
                hsla(0.0, 0.0, 1.0, 0.08),
            ));
            h_off += step_hours;
        }

        let x_of = |i: usize| -> f32 {
            let denom = (sampled.solar_alts.len().saturating_sub(1)).max(1) as f32;
            origin_x + (i as f32 / denom) * w
        };

        // 4. Moon altitude curve
        if sampled.moon_alts.len() >= 2 {
            let mut moon_path = PathBuilder::stroke(px(1.2));
            moon_path.move_to(point(
                px(x_of(0)),
                px(Self::alt_to_y(sampled.moon_alts[0], bounds)),
            ));
            for i in 1..sampled.moon_alts.len() {
                moon_path.line_to(point(
                    px(x_of(i)),
                    px(Self::alt_to_y(sampled.moon_alts[i], bounds)),
                ));
            }
            if let Ok(path) = moon_path.build() {
                window.paint_path(path, rgb(0xd4dce8).alpha(0.42));
            }
        }

        // 5. Color-coded target altitude curves (opacity encodes above-horizon visibility)
        for curve in &sampled.targets {
            if curve.alts.len() < 2 {
                continue;
            }
            let is_selected = selected_ids.contains(&curve.target_id);
            let is_primary = primary_selected_id == Some(curve.target_id.as_str());
            let hue_norm = curve.hue_deg.rem_euclid(360.0) / 360.0;
            let vis_alpha = if is_selected { 0.95 } else { 0.72 };
            let dim_alpha = if is_selected { 0.24 } else { 0.14 };
            let vis_width = if is_selected { px(2.3) } else { px(1.5) };
            let dim_width = px(1.2);

            let mut vis_path = PathBuilder::stroke(vis_width);
            let mut dim_path = PathBuilder::stroke(dim_width);
            let mut has_vis = false;
            let mut has_dim = false;

            for i in 1..curve.alts.len() {
                let p0 = point(px(x_of(i - 1)), px(Self::alt_to_y(curve.alts[i - 1], bounds)));
                let p1 = point(px(x_of(i)), px(Self::alt_to_y(curve.alts[i], bounds)));
                if curve.visible[i - 1] && curve.visible[i] {
                    vis_path.move_to(p0);
                    vis_path.line_to(p1);
                    has_vis = true;
                } else {
                    dim_path.move_to(p0);
                    dim_path.line_to(p1);
                    has_dim = true;
                }
            }

            if has_dim && let Ok(path) = dim_path.build() {
                window.paint_path(path, hsla(hue_norm, 0.75, 0.60, dim_alpha));
            }
            if has_vis && let Ok(path) = vis_path.build() {
                window.paint_path(path, hsla(hue_norm, 0.80, 0.62, vis_alpha));
            }

            if let Some(idx) = curve.meridian_transit_idx {
                let tx = x_of(idx);
                let ty = Self::alt_to_y(curve.alts[idx], bounds);
                let r = if is_selected { 3.5 } else { 2.5 };
                window.paint_quad(fill(
                    Bounds::new(point(px(tx - r), px(ty - r)), size(px(r * 2.0), px(r * 2.0))),
                    hsla(hue_norm, 0.85, 0.70, 0.95),
                ));
            }

            // Primary selected target's Moon separation curve (dashed, mapped sep/2 onto [0..90] deg)
            if is_primary && show_moon_sep && curve.moon_sep_deg.len() >= 2 {
                let mut sep_path = PathBuilder::stroke(px(1.3));
                let mut has_sep = false;
                for i in (1..curve.moon_sep_deg.len()).step_by(2) {
                    let y0 = Self::alt_to_y(curve.moon_sep_deg[i - 1] * 0.5, bounds);
                    let y1 = Self::alt_to_y(curve.moon_sep_deg[i] * 0.5, bounds);
                    sep_path.move_to(point(px(x_of(i - 1)), px(y0)));
                    sep_path.line_to(point(px(x_of(i)), px(y1)));
                    has_sep = true;
                }
                if has_sep && let Ok(path) = sep_path.build() {
                    window.paint_path(path, hsla(hue_norm, 0.45, 0.78, 0.55));
                }
            }
        }

        // 6. Now cursor line
        if let Some(now) = now_unix_secs
            && now >= sampled.window_start_secs
            && now <= sampled.window_end_secs
        {
            let frac = ((now - sampled.window_start_secs)
                / (sampled.window_end_secs - sampled.window_start_secs).max(1.0))
                as f32;
            let x = origin_x + frac * w;
            window.paint_quad(fill(
                Bounds::new(point(px(x - 0.5), px(origin_y)), size(px(1.5), px(h))),
                rgb(0xfbbf24).alpha(0.85),
            ));
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (selected_id, selected_count) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.selected_target_id().map(str::to_string),
                    m.selected_target_ids().len(),
                )
            })
            .unwrap_or((None, 0));

        let moon_illum = self
            .sampled
            .as_ref()
            .map_or(0.0, |s| s.moon_illum_pct);
        let selected_min_sep = self.sampled.as_ref().and_then(|s| {
            let sid = selected_id.as_deref()?;
            s.targets
                .iter()
                .find(|t| t.target_id == sid)
                .map(|t| t.min_moon_sep_deg)
        });

        let moon_tooltip = match selected_min_sep {
            Some(sep) => format!(
                "Moon {:.0}% - Min sep {:.0} deg (toggle separation curve)",
                moon_illum, sep
            ),
            None => format!("Moon {:.0}% (toggle separation curve)", moon_illum),
        };
        let colors = cx.theme().colors();
        let moon_icon_color = if self.show_moon_sep {
            colors.text_accent
        } else {
            colors.text_muted
        };
        let moon_active_bg = colors.text_accent.opacity(0.16);
        let moon_hover_bg = colors.ghost_element_hover;

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Label::new("Night Timeline")
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .when(selected_count > 1, |el| {
                        el.child(
                            Label::new(format!("{selected_count} selected"))
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        )
                    }),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("timeline_moon_sep_btn")
                            .size(px(22.0))
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .when(self.show_moon_sep, |el| el.bg(moon_active_bg))
                            .hover(move |s| s.bg(moon_hover_bg))
                            .tooltip(Tooltip::text(moon_tooltip))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_moon_sep = !this.show_moon_sep;
                                cx.notify();
                            }))
                            .child(render_svg_icon(ICON_MOON, px(14.0), moon_icon_color)),
                    )
                    .child(
                        Button::new("timeline_mode_btn", self.mode.label())
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .tooltip(Tooltip::text("Toggle 16h night / 24h day timeline window"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mode = this.mode.toggle();
                                this.ensure_samples_fresh(cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_axis_ticks(&self, cx: &App) -> gpui::AnyElement {
        let Some(sampled) = &self.sampled else {
            return div().into_any_element();
        };
        let utc_offset = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());
        let span = sampled.window_end_secs - sampled.window_start_secs;
        let ticks = 5usize;

        h_flex()
            .w_full()
            .justify_between()
            .px_1()
            .children((0..ticks).map(|i| {
                let frac = i as f64 / (ticks - 1) as f64;
                let t = (sampled.window_start_secs + frac * span).round() as i64;
                Label::new(format_unix_hm(t, utc_offset))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
            }))
            .into_any_element()
    }

    fn render_target_blocks_track(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(sampled) = self.sampled.clone() else {
            return div().h(px(BLOCK_TRACK_HEIGHT_PX)).into_any_element();
        };
        let Some(plan_model) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.read(cx)) else {
            return div().h(px(BLOCK_TRACK_HEIGHT_PX)).into_any_element();
        };
        let utc_offset = cx
            .try_global::<GlobalObserver>()
            .map_or(0, |g| g.0.read(cx).effective_utc_offset_secs());
        let plan = plan_model.plan();
        let selected_ids = plan_model.selected_target_ids().clone();

        let span = (sampled.window_end_secs - sampled.window_start_secs).max(60.0);
        let locked = is_session_locked(cx);
        let bounds_store = self.track_bounds.clone();
        let track_bounds_snapshot = bounds_store.lock().ok().and_then(|g| *g);
        let track_width_px = track_bounds_snapshot
            .map_or(600.0, |b| f32::from(b.size.width))
            .max(100.0);

        let blocks: Vec<_> = plan
            .targets
            .iter()
            .enumerate()
            .map(|(idx, target)| {
                let start_f = ((target.start_unix_secs as f64 - sampled.window_start_secs) / span)
                    .clamp(0.0, 1.0) as f32;
                let end_f = ((target.end_unix_secs() as f64 - sampled.window_start_secs) / span)
                    .clamp(0.0, 1.0) as f32;
                let left_px = start_f * track_width_px;
                let raw_w_px = (end_f - start_f) * track_width_px;
                let width_px =
                    raw_w_px.max(22.0).min(track_width_px - left_px.min(track_width_px - 22.0));
                let hue = target.color_hue_deg.rem_euclid(360.0) / 360.0;
                let is_sel = selected_ids.contains(&target.id);
                let dur_text = format_duration_compact(target.estimated_duration());
                let start_text = format_unix_hm(target.start_unix_secs, utc_offset);

                (
                    idx,
                    target.id.clone(),
                    target.name.clone(),
                    start_text,
                    dur_text,
                    left_px,
                    width_px,
                    hue,
                    is_sel,
                    target.enabled,
                )
            })
            .collect();

        let marquee_rect = if let Some(TimelineDrag::MarqueeSelect { start_x, current_x }) = &self.drag
        {
            let x0 = start_x.min(*current_x);
            let w = (start_x - current_x).abs().max(2.0);
            Some((x0, w))
        } else {
            None
        };

        let colors = cx.theme().colors();
        let sampled_bg_down = sampled.clone();
        let bounds_bg_down = self.track_bounds.clone();

        div()
            .id("timeline_blocks_track")
            .relative()
            .w_full()
            .h(px(BLOCK_TRACK_HEIGHT_PX))
            .rounded_sm()
            .bg(colors.surface_background)
            .border_1()
            .border_color(colors.border)
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        if let Ok(mut guard) = bounds_store.lock() {
                            *guard = Some(bounds);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                    let Some(bounds) = bounds_bg_down.lock().ok().and_then(|g| *g) else {
                        return;
                    };
                    let width = f32::from(bounds.size.width).max(1.0);
                    let local_x = (f32::from(ev.position.x) - f32::from(bounds.origin.x))
                        .clamp(0.0, width);
                    this.drag = Some(TimelineDrag::MarqueeSelect {
                        start_x: local_x,
                        current_x: local_x,
                    });
                    let frac = f64::from(local_x / width);
                    let span = sampled_bg_down.window_end_secs - sampled_bg_down.window_start_secs;
                    let t = (sampled_bg_down.window_start_secs + frac * span).round() as i64;
                    if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone()) {
                        plan.update(cx, |m, cx| {
                            let ids: Vec<String> = m
                                .plan()
                                .targets
                                .iter()
                                .filter(|tgt| {
                                    tgt.start_unix_secs <= t && t <= tgt.footprint_end_unix_secs()
                                })
                                .map(|tgt| tgt.id.clone())
                                .collect();
                            m.set_selected_targets(ids, None, cx);
                        });
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                let Some(sampled) = &this.sampled else {
                    return;
                };
                let Some(bounds) = this.track_bounds.lock().ok().and_then(|g| *g) else {
                    return;
                };
                let width = f32::from(bounds.size.width).max(1.0);
                let local_x =
                    (f32::from(ev.position.x) - f32::from(bounds.origin.x)).clamp(0.0, width);
                let frac = f64::from(local_x / width);
                let span = sampled.window_end_secs - sampled.window_start_secs;

                match &mut this.drag {
                    Some(TimelineDrag::MoveBlocks {
                        base_starts,
                        anchor_cursor_unix,
                    }) => {
                        if is_session_locked(cx) {
                            this.drag = None;
                            return;
                        }
                        let cursor_unix = (sampled.window_start_secs + frac * span).round() as i64;
                        let raw_delta = cursor_unix.saturating_sub(*anchor_cursor_unix);
                        let step = if ev.modifiers.shift { 60 } else { 300 };
                        let snapped_delta = ((raw_delta as f64 / step as f64).round() as i64) * step;
                        let base_snapshot = base_starts.clone();
                        let win_start = sampled.window_start_secs.round() as i64;
                        let win_end = sampled.window_end_secs.round() as i64;
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            plan.update(cx, |m, cx| {
                                m.move_targets_from_baseline(
                                    &base_snapshot,
                                    snapped_delta,
                                    win_start,
                                    win_end,
                                    cx,
                                );
                            });
                        }
                    }
                    Some(TimelineDrag::MarqueeSelect { start_x, current_x }) => {
                        *current_x = local_x;
                        let x_lo = start_x.min(*current_x);
                        let x_hi = start_x.max(*current_x);
                        let t_lo = (sampled.window_start_secs + f64::from(x_lo / width) * span)
                            .round() as i64;
                        let t_hi = (sampled.window_start_secs + f64::from(x_hi / width) * span)
                            .round() as i64;
                        if let Some(plan) = cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                        {
                            plan.update(cx, |m, cx| {
                                let ids: Vec<String> = m
                                    .plan()
                                    .targets
                                    .iter()
                                    .filter(|tgt| {
                                        tgt.start_unix_secs <= t_hi
                                            && tgt.footprint_end_unix_secs() >= t_lo
                                    })
                                    .map(|tgt| tgt.id.clone())
                                    .collect();
                                m.set_selected_targets(ids, None, cx);
                            });
                        }
                        cx.notify();
                    }
                    None => {}
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            }))
            .children(blocks.into_iter().map(
                |(
                    idx,
                    target_id,
                    name,
                    start_text,
                    dur_text,
                    left_px,
                    width_px,
                    hue,
                    is_sel,
                    enabled,
                )| {
                    let bg_alpha = if enabled { 0.78 } else { 0.28 };
                    let border_col = if is_sel {
                        hsla(hue, 0.92, 0.78, 1.0)
                    } else {
                        hsla(hue, 0.65, 0.50, 0.70)
                    };
                    let sampled_for_down = sampled.clone();
                    let bounds_for_down = self.track_bounds.clone();
                    let show_time = width_px >= 120.0;
                    let tip = format!("{name}  {start_text} ({dur_text})");
                    div()
                        .id(("timeline_block", idx))
                        .absolute()
                        .left(px(left_px))
                        .top(px(4.0))
                        .w(px(width_px))
                        .h(px(BLOCK_TRACK_HEIGHT_PX - 8.0))
                        .px_1p5()
                        .rounded_sm()
                        .bg(hsla(hue, 0.68, if is_sel { 0.42 } else { 0.34 }, bg_alpha))
                        .border_1()
                        .border_color(border_col)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .overflow_hidden()
                        .cursor_pointer()
                        .tooltip(Tooltip::text(tip))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                let mods = ev.modifiers;
                                let is_mod = mods.control || mods.platform || mods.shift;
                                let Some(plan) =
                                    cx.try_global::<GlobalNightPlan>().map(|g| g.0.clone())
                                else {
                                    return;
                                };

                                if is_mod {
                                    let tid = target_id.clone();
                                    plan.update(cx, |m, cx| m.toggle_target_selection(&tid, cx));
                                    cx.notify();
                                    return;
                                }

                                let already_sel = plan.read(cx).is_target_selected(&target_id);
                                if !already_sel {
                                    let tid = target_id.clone();
                                    plan.update(cx, |m, cx| m.select_target(Some(tid), cx));
                                }

                                if !locked
                                    && let Some(bounds) =
                                        bounds_for_down.lock().ok().and_then(|g| *g)
                                {
                                    let width = f32::from(bounds.size.width).max(1.0);
                                    let frac = ((f32::from(ev.position.x)
                                        - f32::from(bounds.origin.x))
                                        / width)
                                        .clamp(0.0, 1.0)
                                        as f64;
                                    let span = sampled_for_down.window_end_secs
                                        - sampled_for_down.window_start_secs;
                                    let anchor_cursor_unix =
                                        (sampled_for_down.window_start_secs + frac * span).round()
                                            as i64;
                                    let base_starts: Vec<(String, i64)> = {
                                        let m = plan.read(cx);
                                        m.plan()
                                            .targets
                                            .iter()
                                            .filter(|t| m.is_target_selected(&t.id))
                                            .map(|t| (t.id.clone(), t.start_unix_secs))
                                            .collect()
                                    };
                                    this.drag = Some(TimelineDrag::MoveBlocks {
                                        base_starts,
                                        anchor_cursor_unix,
                                    });
                                }
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .text_color(cx.theme().colors().text)
                                .child(name),
                        )
                        .when(show_time, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .font_buffer(cx)
                                    .text_size(px(11.0))
                                    .text_color(cx.theme().colors().text_muted)
                                    .child(start_text),
                            )
                        })
                },
            ))
            .when_some(marquee_rect, |el, (x0, w)| {
                el.child(
                    div()
                        .absolute()
                        .left(px(x0))
                        .top(px(2.0))
                        .w(px(w))
                        .h(px(BLOCK_TRACK_HEIGHT_PX - 4.0))
                        .rounded_sm()
                        .bg(hsla(0.58, 0.75, 0.65, 0.18))
                        .border_1()
                        .border_color(hsla(0.58, 0.85, 0.72, 0.80)),
                )
            })
            .into_any_element()
    }
}

impl Render for NightTimelineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_samples_fresh(cx);
        let sampled = self.sampled.clone();
        let (primary_id, selected_ids) = cx
            .try_global::<GlobalNightPlan>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.selected_target_id().map(str::to_string),
                    m.selected_target_ids().clone(),
                )
            })
            .unwrap_or_default();
        let now_unix = cx
            .try_global::<GlobalObserver>()
            .map(|g| g.0.read(cx).effective_unix_seconds());
        let show_moon_sep = self.show_moon_sep;

        v_flex()
            .w_full()
            .p_3()
            .gap_2()
            .child(self.render_header(cx))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(GRAPH_HEIGHT_PX))
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .overflow_hidden()
                    .when_some(sampled, |el, s| {
                        el.child(
                            canvas(
                                move |_, _, _| {},
                                move |bounds, (), window, _| {
                                    Self::paint_graph(
                                        &s,
                                        primary_id.as_deref(),
                                        &selected_ids,
                                        show_moon_sep,
                                        now_unix,
                                        bounds,
                                        window,
                                    );
                                },
                            )
                            .size_full(),
                        )
                    })
                    .child(
                        v_flex()
                            .absolute()
                            .top_1()
                            .left_1p5()
                            .gap_4()
                            .child(Label::new("60°").size(LabelSize::XSmall).color(Color::Muted))
                            .child(Label::new("30°").size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .when(self.show_moon_sep, |el| {
                        el.child(
                            v_flex()
                                .absolute()
                                .top_1()
                                .right_1p5()
                                .items_end()
                                .gap_4()
                                .child(
                                    Label::new("180°")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new("90°")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    }),
            )
            .child(self.render_axis_ticks(cx))
            .child(self.render_target_blocks_track(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_night_window_centers_on_following_midnight() {
        let (start_16, end_16) = compute_night_window_unix_secs(
            "2026-11-26",
            3600,
            0.0,
            TimelineWindowMode::Night16h,
        );
        assert!((end_16 - start_16 - 16.0 * 3600.0).abs() < 1e-6);

        let (start_24, end_24) = compute_night_window_unix_secs(
            "2026-11-26",
            3600,
            0.0,
            TimelineWindowMode::FullDay24h,
        );
        assert!((end_24 - start_24 - 24.0 * 3600.0).abs() < 1e-6);
        assert!((f64::midpoint(start_16, end_16) - f64::midpoint(start_24, end_24)).abs() < 1e-6);
    }

    #[test]
    fn snap_unix_secs_rounds_and_clamps() {
        let s = snap_unix_secs(10_140.0, 300, 10_000.0, 20_000.0);
        assert_eq!(s, 10_200);
        let clamped_lo = snap_unix_secs(5_000.0, 300, 10_000.0, 20_000.0);
        assert_eq!(clamped_lo, 10_000);
        let clamped_hi = snap_unix_secs(25_000.0, 300, 10_000.0, 20_000.0);
        assert_eq!(clamped_hi, 19_940);
    }

    #[test]
    fn angular_separation_matches_orthogonal_vectors() {
        let a = DVec3::X;
        let b = DVec3::Y;
        assert!((angular_separation_deg(a, b) - 90.0).abs() < 1e-9);
        assert!(angular_separation_deg(a, a).abs() < 1e-9);
    }
}
