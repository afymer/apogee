use std::sync::{Arc, Mutex};

use celestial::{
    DailyTargetVisibility, HorizonProfile, StarMetadata,
    compute_target_visibility_with_horizon,
    coords::{Angle, canonical_spherical_to_cartesian},
    sample_day_equatorial_track,
};
use glam::DVec3;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Pixels, Point, Render, Rgba, Window, div, fill, hsla, point, prelude::*, px, rgb, svg,
};
use horizon::GlobalHorizons;
use observer::{
    GlobalObserver, ObserverModel, paint_circle, paint_twilight_backdrop, sample_day_profile,
};
use planetarium::PickedStar;
use status_bar::{format_altaz, format_radec};
use ui::{
    ActiveTheme, ButtonCommon, Clickable, IconButton, IconName, IconSize, StyledTypography,
    Tooltip, h_flex, v_flex,
};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
};

const GRAPH_HEIGHT_PX: f32 = 156.0;
const GRAPH_PAD_Y: f32 = 8.0;
const GRAPH_MIN_ALT_DEG: f64 = -25.0;
const GRAPH_MAX_ALT_DEG: f64 = 90.0;

const ICON_HORIZON_OVERLAY: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 17.5L7.5 10.5L11.8 15L16.2 8.5L21.5 17.5" stroke="black" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M2.5 19.5H21.5" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

#[derive(Clone, Debug, PartialEq)]
pub struct SelectedStar {
    pub catalog_index: u32,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub mag: f32,
    pub bp_rp: f32,
    pub srgb: [f32; 3],
    pub spectral_class: &'static str,
    pub estimated_teff_k: u32,
    pub metadata: Option<StarMetadata>,
}

impl SelectedStar {
    pub fn from_picked(star: &PickedStar) -> Self {
        Self {
            catalog_index: star.catalog_index,
            ra_deg: star.ra_deg,
            dec_deg: star.dec_deg,
            mag: star.mag,
            bp_rp: star.bp_rp,
            srgb: star.srgb,
            spectral_class: star.spectral_class,
            estimated_teff_k: star.estimated_teff_k,
            metadata: star.metadata.clone(),
        }
    }

    pub fn icrs_dir(&self) -> DVec3 {
        canonical_spherical_to_cartesian(
            Angle::from_degrees(self.ra_deg),
            Angle::from_degrees(self.dec_deg),
        )
        .normalize_or_zero()
    }

    pub fn display_title_and_subtitle(&self) -> (String, Option<String>) {
        match self.metadata.as_ref() {
            Some(meta) => meta.display_title_and_subtitle(self.spectral_class),
            None => (format!("{}-type Star", self.spectral_class), None),
        }
    }

    pub fn catalog_ids_summary(&self) -> Option<String> {
        let meta = self.metadata.as_ref()?;
        let mut ids = Vec::new();
        if let Some(hip) = meta.hip {
            ids.push(format!("HIP {hip}"));
        }
        if let Some(hd) = meta.hd {
            ids.push(format!("HD {hd}"));
        }
        if let Some(hr) = meta.hr {
            ids.push(format!("HR {hr}"));
        }
        if let Some(tyc) = meta.tyc.as_deref().filter(|s| !s.is_empty()) {
            ids.push(format!("TYC {tyc}"));
        }
        if ids.is_empty() {
            None
        } else {
            Some(ids.join(" · "))
        }
    }
}

#[derive(Default)]
pub struct StarSelectionModel {
    selected: Option<SelectedStar>,
    activation_nonce: u64,
}

impl StarSelectionModel {
    pub fn select(&mut self, star: SelectedStar, cx: &mut Context<Self>) {
        self.selected = Some(star);
        self.activation_nonce = self.activation_nonce.wrapping_add(1);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.selected.take().is_some() {
            cx.notify();
        }
    }

    pub fn selected(&self) -> Option<&SelectedStar> {
        self.selected.as_ref()
    }

    pub fn activation_nonce(&self) -> u64 {
        self.activation_nonce
    }
}

#[derive(Clone)]
pub struct GlobalStarSelection(pub Entity<StarSelectionModel>);

impl gpui::Global for GlobalStarSelection {}

pub fn init(cx: &mut App) {
    let selection = cx.new(|_| StarSelectionModel::default());
    cx.set_global(GlobalStarSelection(selection));

    cx.observe_new(|workbench: &mut workspace::workbench::Workbench, window, cx| {
        if let Some(window) = window {
            let panel = StarDetailsPanel::build(cx);
            cx.observe(&panel, |_this, _panel, cx| {
                cx.notify();
            })
            .detach();
            workbench.add_panel(panel, window, cx);
        }
    })
    .detach();
}

#[derive(Clone, PartialEq, Eq)]
struct ProfileCacheKey {
    catalog_index: u32,
    lat_bits: u64,
    lon_bits: u64,
    elev_bits: u64,
    local_midnight_secs: i64,
    horizon_key: Option<(String, u64)>,
}

pub struct StarDetailsPanel {
    focus_handle: FocusHandle,
    last_activation_nonce: u64,
    cache_key: Option<ProfileCacheKey>,
    cached_solar_profile: Vec<f64>,
    cached_star_profile: Vec<f64>,
    cached_horizon_profile: Vec<f64>,
    cached_visibility: Option<DailyTargetVisibility>,
    cached_has_custom_horizon: bool,
    cached_active_horizon: Option<HorizonProfile>,
    show_horizon_overlay: bool,
    graph_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    is_scrubbing: bool,
}

impl EventEmitter<PanelEvent> for StarDetailsPanel {}

impl Focusable for StarDetailsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl StarDetailsPanel {
    pub fn build(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let selection_entity = cx.global::<GlobalStarSelection>().0.clone();
            let initial_nonce = selection_entity.read(cx).activation_nonce();

            cx.observe(&selection_entity, |this: &mut Self, model, cx| {
                let nonce = model.read(cx).activation_nonce();
                let has_selection = model.read(cx).selected().is_some();
                this.ensure_profiles_fresh(cx);
                if nonce != this.last_activation_nonce {
                    this.last_activation_nonce = nonce;
                    if has_selection {
                        cx.emit(PanelEvent::Activate);
                    } else {
                        cx.emit(PanelEvent::Close);
                    }
                }
                cx.notify();
            })
            .detach();

            if let Some(observer_entity) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
                cx.observe(&observer_entity, |this: &mut Self, _model, cx| {
                    this.ensure_profiles_fresh(cx);
                    cx.notify();
                })
                .detach();
            }

            if let Some(horizons_entity) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
                cx.observe(&horizons_entity, |this: &mut Self, _model, cx| {
                    this.ensure_profiles_fresh(cx);
                    cx.notify();
                })
                .detach();
            }

            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                last_activation_nonce: initial_nonce,
                cache_key: None,
                cached_solar_profile: Vec::new(),
                cached_star_profile: Vec::new(),
                cached_horizon_profile: Vec::new(),
                cached_visibility: None,
                cached_has_custom_horizon: false,
                cached_active_horizon: None,
                show_horizon_overlay: true,
                graph_bounds: Arc::new(Mutex::new(None)),
                is_scrubbing: false,
            };
            panel.ensure_profiles_fresh(cx);
            panel
        })
    }

    fn ensure_profiles_fresh(&mut self, cx: &App) {
        let Some(star) = cx
            .try_global::<GlobalStarSelection>()
            .and_then(|g| g.0.read(cx).selected().cloned())
        else {
            self.cache_key = None;
            self.cached_solar_profile.clear();
            self.cached_star_profile.clear();
            self.cached_horizon_profile.clear();
            self.cached_visibility = None;
            self.cached_has_custom_horizon = false;
            self.cached_active_horizon = None;
            return;
        };

        let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.read(cx)) else {
            return;
        };

        let horizon_info = observer.active_horizon_id().and_then(|id| {
            let horizons = cx.try_global::<GlobalHorizons>()?.0.read(cx);
            let prof = horizons.profile_for(id)?;
            Some(((id.to_string(), horizons.live_generation()), prof))
        });
        let horizon_key = horizon_info.as_ref().map(|(k, _)| k.clone());
        let active_profile = horizon_info.map(|(_, p)| p);

        let loc = observer.geodetic_position();
        let midnight = observer.local_midnight_unix_seconds();
        let next_key = ProfileCacheKey {
            catalog_index: star.catalog_index,
            lat_bits: loc.latitude_deg().to_bits(),
            lon_bits: loc.longitude_deg().to_bits(),
            elev_bits: loc.elevation_m().to_bits(),
            local_midnight_secs: midnight.round() as i64,
            horizon_key,
        };

        if self.cache_key.as_ref() == Some(&next_key) {
            return;
        }

        let ra = Angle::from_degrees(star.ra_deg);
        let dec = Angle::from_degrees(star.dec_deg);
        self.cached_solar_profile = observer.solar_day_profile(97);
        let track = sample_day_equatorial_track(loc, midnight, ra, dec, 97);
        self.cached_star_profile = track.iter().map(|(_, alt)| *alt).collect();
        self.cached_horizon_profile = if let Some(ref prof) = active_profile {
            track
                .iter()
                .map(|(az, _)| prof.altitude_at_azimuth_deg(*az))
                .collect()
        } else {
            Vec::new()
        };
        self.cached_visibility = Some(compute_target_visibility_with_horizon(
            loc,
            midnight,
            ra,
            dec,
            active_profile.as_ref(),
        ));
        self.cached_has_custom_horizon = active_profile.is_some();
        self.cached_active_horizon = active_profile;
        self.cache_key = Some(next_key);
    }

    fn scrub_to_position(
        &self,
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

        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            observer.update(cx, |model, cx| {
                model.set_local_hour_of_day(snapped_hour, cx);
            });
        }
    }

    fn jump_to_hour(&self, hour: f64, cx: &mut Context<Self>) {
        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            observer.update(cx, |model, cx| {
                model.set_local_hour_of_day(hour, cx);
            });
        }
    }

    fn close_panel(&self, cx: &mut Context<Self>) {
        if let Some(selection) = cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone()) {
            selection.update(cx, |model, cx| model.clear(cx));
        }
    }
}

#[inline]
fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[inline]
fn alt_to_y_fraction(alt_deg: f64, pad_y: f32, height: f32) -> f32 {
    let usable_h = (height - 2.0 * pad_y).max(4.0);
    let norm =
        ((alt_deg - GRAPH_MIN_ALT_DEG) / (GRAPH_MAX_ALT_DEG - GRAPH_MIN_ALT_DEG)).clamp(0.0, 1.0)
            as f32;
    pad_y + usable_h * (1.0 - norm)
}

#[inline]
fn alt_to_y(alt_deg: f64, bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.origin.y)
        + alt_to_y_fraction(alt_deg, GRAPH_PAD_Y, f32::from(bounds.size.height))
}

fn paint_above_horizon_segments(
    profile: &[f64],
    horizon_profile: &[f64],
    bounds: Bounds<Pixels>,
    stroke_width: f32,
    color: Rgba,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let origin_x = f32::from(bounds.origin.x);
    let denom = (profile.len() - 1) as f32;
    let curve_pt = |i: usize| -> Point<Pixels> {
        let x = origin_x + (i as f32 / denom) * width;
        let y = alt_to_y(profile[i], bounds);
        point(px(x), px(y))
    };

    let mut path_builder: Option<PathBuilder> = None;
    for i in 1..profile.len() {
        let a0 = profile[i - 1];
        let a1 = profile[i];
        let h0 = horizon_profile.get(i - 1).copied().unwrap_or(0.0);
        let h1 = horizon_profile.get(i).copied().unwrap_or(0.0);
        let c0 = a0 - h0;
        let c1 = a1 - h1;
        let p0 = curve_pt(i - 1);
        let p1 = curve_pt(i);

        if c0 >= 0.0 && c1 >= 0.0 {
            let builder = path_builder.get_or_insert_with(|| {
                let mut b = PathBuilder::stroke(px(stroke_width));
                b.move_to(p0);
                b
            });
            builder.line_to(p1);
        } else if c0 < 0.0 && c1 >= 0.0 {
            let frac = ((-c0) / (c1 - c0).max(1e-6)).clamp(0.0, 1.0) as f32;
            let cross_alt = a0 + (a1 - a0) * f64::from(frac);
            let cross = point(
                px(f32::from(p0.x) + (f32::from(p1.x) - f32::from(p0.x)) * frac),
                px(alt_to_y(cross_alt, bounds)),
            );
            let builder = path_builder.get_or_insert_with(|| {
                let mut b = PathBuilder::stroke(px(stroke_width));
                b.move_to(cross);
                b
            });
            builder.line_to(p1);
        } else if c0 >= 0.0 && c1 < 0.0 {
            let frac = (c0 / (c0 - c1).max(1e-6)).clamp(0.0, 1.0) as f32;
            let cross_alt = a0 + (a1 - a0) * f64::from(frac);
            let cross = point(
                px(f32::from(p0.x) + (f32::from(p1.x) - f32::from(p0.x)) * frac),
                px(alt_to_y(cross_alt, bounds)),
            );
            if let Some(mut builder) = path_builder.take() {
                builder.line_to(cross);
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
        }
    }
    if let Some(builder) = path_builder {
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_visibility_graph(
    bounds: Bounds<Pixels>,
    hour_of_day: f64,
    solar_profile: &[f64],
    star_profile: &[f64],
    horizon_profile: &[f64],
    visibility: Option<DailyTargetVisibility>,
    star_rgb: Rgba,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    // 1. Twilight sky backdrop identical to observer's dropdown.
    paint_twilight_backdrop(bounds, solar_profile, window);

    // 2. Darker shading below the 0° horizon.
    let horizon_y = alt_to_y(0.0, bounds);
    let below_h = (origin_y + height - horizon_y).max(0.0);
    if below_h > 0.0 {
        window.paint_quad(fill(
            Bounds::new(
                point(px(origin_x), px(horizon_y)),
                gpui::size(px(width), px(below_h)),
            ),
            rgb(0x040710).alpha(0.46),
        ));
    }

    // 3. Reference altitude lines (+60°, +30°, 0°).
    for deg in [60.0_f64, 30.0] {
        let y = alt_to_y(deg, bounds);
        window.paint_quad(fill(
            Bounds::new(point(px(origin_x), px(y)), gpui::size(px(width), px(1.0))),
            hsla(0.0, 0.0, 1.0, 0.12),
        ));
    }
    window.paint_quad(fill(
        Bounds::new(point(px(origin_x), px(horizon_y)), gpui::size(px(width), px(1.0))),
        hsla(0.0, 0.0, 1.0, 0.34),
    ));

    // 3b. Custom horizon obstruction silhouette & curve along the star's daily azimuth track.
    if horizon_profile.len() >= 2 {
        let denom_h = (horizon_profile.len() - 1) as f32;
        let h_pt = |i: usize| -> Point<Pixels> {
            let x = origin_x + (i as f32 / denom_h) * width;
            let y = alt_to_y(horizon_profile[i], bounds);
            point(px(x), px(y))
        };
        let bottom_y = origin_y + height;
        let mut poly = PathBuilder::fill();
        poly.move_to(point(px(origin_x), px(bottom_y)));
        for i in 0..horizon_profile.len() {
            poly.line_to(h_pt(i));
        }
        poly.line_to(point(px(origin_x + width), px(bottom_y)));
        poly.close();
        if let Ok(path) = poly.build() {
            window.paint_path(path, rgb(0x0b1624).alpha(0.62));
        }

        let mut line = PathBuilder::stroke(px(1.5));
        line.move_to(h_pt(0));
        for i in 1..horizon_profile.len() {
            line.line_to(h_pt(i));
        }
        if let Ok(path) = line.build() {
            window.paint_path(path, rgb(0x22b8cf).alpha(0.85));
        }
    }

    let crisp_color = lerp_rgba(star_rgb, rgb(0xffffff), 0.25).alpha(0.96);

    // 4. Star elevation curve + transit peak marker.
    if star_profile.len() >= 2 {
        let denom = (star_profile.len() - 1) as f32;
        let curve_pt = |i: usize| -> Point<Pixels> {
            let x = origin_x + (i as f32 / denom) * width;
            let y = alt_to_y(star_profile[i], bounds);
            point(px(x), px(y))
        };

        let mut base_path = PathBuilder::stroke(px(1.5));
        base_path.move_to(curve_pt(0));
        for i in 1..star_profile.len() {
            base_path.line_to(curve_pt(i));
        }
        if let Ok(path) = base_path.build() {
            window.paint_path(path, star_rgb.alpha(0.38));
        }

        // Dark backing keeps bright curve legible across bright daytime sky gradients.
        paint_above_horizon_segments(
            star_profile,
            horizon_profile,
            bounds,
            3.8,
            rgb(0x060a14).alpha(0.55),
            window,
        );
        paint_above_horizon_segments(
            star_profile,
            horizon_profile,
            bounds,
            2.2,
            crisp_color,
            window,
        );
    }

    if let Some(vis) = visibility {
        let visible_at_peak = if horizon_profile.is_empty() {
            vis.max_altitude_deg >= 0.0
        } else {
            !vis.never_rises
                && vis.max_altitude_deg >= sample_day_profile(horizon_profile, vis.transit_hour)
        };
        if visible_at_peak {
            let peak_x = origin_x + (vis.transit_hour / 24.0).clamp(0.0, 1.0) as f32 * width;
            let peak_y = alt_to_y(vis.max_altitude_deg, bounds);
            let peak_pt = point(px(peak_x), px(peak_y));
            paint_circle(window, peak_pt, 3.8, rgb(0x090d1a).alpha(0.8));
            paint_circle(window, peak_pt, 2.5, crisp_color);
        }
    }

    // 5. Playhead beam and bordered orb at active time.
    let t = (hour_of_day / 24.0).clamp(0.0, 1.0) as f32;
    let playhead_x = origin_x + t * width;
    let current_alt = sample_day_profile(star_profile, hour_of_day);
    let handle_y = alt_to_y(current_alt, bounds);
    let handle_pt = point(px(playhead_x), px(handle_y));

    let mut beam_builder = PathBuilder::stroke(px(1.0));
    beam_builder.move_to(point(px(playhead_x), px(origin_y)));
    beam_builder.line_to(point(px(playhead_x), px(origin_y + height)));
    if let Ok(beam_path) = beam_builder.build() {
        window.paint_path(beam_path, hsla(0.0, 0.0, 1.0, 0.35));
    }

    paint_circle(window, handle_pt, 5.5, rgb(0x090d1a).alpha(0.88));
    paint_circle(window, handle_pt, 4.5, rgb(0xf2f6fc).alpha(0.96));
    paint_circle(window, handle_pt, 2.9, star_rgb.alpha(1.0));
}

fn format_local_hour_hm(hour: f64) -> String {
    let total_mins = (hour * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", total_mins / 60, total_mins % 60)
}

impl Render for StarDetailsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let selected = cx
            .try_global::<GlobalStarSelection>()
            .and_then(|g| g.0.read(cx).selected().cloned());

        let Some(star) = selected else {
            return v_flex()
                .size_full()
                .bg(colors.panel_background)
                .p_3()
                .justify_between()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child("Star Details"),
                        )
                        .child(
                            IconButton::new("close_star_details_empty_btn", IconName::Close)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text("Close panel"))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.close_panel(cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(colors.text_muted)
                        .child("Click a star to inspect visibility and details"),
                )
                .into_any_element();
        };

        let star_rgb = Rgba {
            r: star.srgb[0],
            g: star.srgb[1],
            b: star.srgb[2],
            a: 1.0,
        };
        let star_color: Hsla = star_rgb.into();
        let (title, subtitle) = star.display_title_and_subtitle();

        let observer_snapshot = cx.try_global::<GlobalObserver>().map(|g| {
            let model: &ObserverModel = g.0.read(cx);
            let obs_ctx = model.observation_context();
            let (az, alt) = obs_ctx.equatorial_to_topocentric_coords(
                Angle::from_degrees(star.ra_deg),
                Angle::from_degrees(star.dec_deg),
            );
            let dt = model.effective_local_datetime();
            (
                model.local_hour_of_day(),
                dt.format("%b %d, %Y").to_string(),
                az.degrees(),
                alt.degrees(),
            )
        });

        let (hour_of_day, date_label, az_deg, alt_deg) =
            observer_snapshot.unwrap_or_else(|| (0.0, String::new(), 0.0, 0.0));

        let local_horizon_alt = self
            .cached_active_horizon
            .as_ref()
            .map(|h| h.altitude_at_azimuth_deg(az_deg));
        let is_visible_now = local_horizon_alt.map_or(alt_deg >= 0.0, |h| alt_deg >= h);
        let alt_readout = if is_visible_now {
            format!("{alt_deg:+.1}°")
        } else if alt_deg >= 0.0 {
            if let Some(h) = local_horizon_alt {
                format!("{alt_deg:+.1}° · Obstructed ({h:+.0}°)")
            } else {
                format!("{alt_deg:+.1}° · Below horizon")
            }
        } else {
            format!("{alt_deg:+.1}° · Below horizon")
        };

        let solar_profile = self.cached_solar_profile.clone();
        let star_profile = self.cached_star_profile.clone();
        let horizon_profile = if self.show_horizon_overlay {
            self.cached_horizon_profile.clone()
        } else {
            Vec::new()
        };
        let visibility = self.cached_visibility;
        let bounds_store = self.graph_bounds.clone();
        let entity = cx.entity().clone();

        let visibility_canvas = gpui::canvas(
            move |bounds, _window, _cx| {
                if let Ok(mut guard) = bounds_store.lock() {
                    *guard = Some(bounds);
                }
            },
            move |bounds, (), window, _cx| {
                paint_visibility_graph(
                    bounds,
                    hour_of_day,
                    &solar_profile,
                    &star_profile,
                    &horizon_profile,
                    visibility,
                    star_rgb,
                    window,
                );

                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        e.update(cx, |this, cx| {
                            if this.is_scrubbing {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    this.scrub_to_position(
                                        event.position,
                                        event.modifiers.shift,
                                        bounds,
                                        cx,
                                    );
                                } else {
                                    this.is_scrubbing = false;
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
                            if this.is_scrubbing {
                                this.is_scrubbing = false;
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .size_full();

        let ref_label = |deg: f64, label: &'static str| {
            let y = (alt_to_y_fraction(deg, GRAPH_PAD_Y, GRAPH_HEIGHT_PX) - 6.0).max(2.0);
            div()
                .absolute()
                .left(px(5.0))
                .top(px(y))
                .font_buffer(cx)
                .text_size(px(9.0))
                .text_color(rgb(0xdce6f5).opacity(0.65))
                .child(label)
        };

        let metric_cell = |key: &'static str, val: String| {
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_muted)
                        .child(key),
                )
                .child(
                    div()
                        .font_buffer(cx)
                        .text_size(px(11.5))
                        .text_color(colors.text)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(val),
                )
        };

        let rts_pill = |id: &'static str,
                        label: &'static str,
                        value: String,
                        jump_hour: Option<f64>| {
            v_flex()
                .id(id)
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .gap_0p5()
                .rounded_md()
                .bg(colors.element_background.opacity(0.65))
                .border_1()
                .border_color(colors.border_variant)
                .when_some(jump_hour, |el, hour| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(colors.element_hover))
                        .tooltip(Tooltip::text(format!(
                            "Jump to {}",
                            format_local_hour_hm(hour)
                        )))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.jump_to_hour(hour, cx);
                        }))
                })
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors.text_muted)
                        .child(label),
                )
                .child(
                    div()
                        .font_buffer(cx)
                        .text_size(px(11.0))
                        .text_color(if jump_hour.is_some() {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(value),
                )
        };

        let (rise_text, rise_jump, transit_text, transit_jump, set_text, set_jump) =
            if let Some(vis) = visibility {
                let transit_val = format!(
                    "{} ({:+.0}°)",
                    format_local_hour_hm(vis.transit_hour),
                    vis.max_altitude_deg
                );
                if vis.circumpolar {
                    (
                        "Circumpolar".to_string(),
                        None,
                        transit_val,
                        Some(vis.transit_hour),
                        "—".to_string(),
                        None,
                    )
                } else if vis.never_rises {
                    let (below_label, never_label) =
                        if self.cached_has_custom_horizon && vis.max_altitude_deg >= 0.0 {
                            ("Obstructed", "Blocked")
                        } else {
                            ("Below", "Never rises")
                        };
                    (
                        below_label.to_string(),
                        None,
                        transit_val,
                        Some(vis.transit_hour),
                        never_label.to_string(),
                        None,
                    )
                } else {
                    (
                        vis.rise_hour.map_or_else(|| "—".to_string(), format_local_hour_hm),
                        vis.rise_hour,
                        transit_val,
                        Some(vis.transit_hour),
                        vis.set_hour.map_or_else(|| "—".to_string(), format_local_hour_hm),
                        vis.set_hour,
                    )
                }
            } else {
                (
                    "—".to_string(),
                    None,
                    "—".to_string(),
                    None,
                    "—".to_string(),
                    None,
                )
            };

        let (ra_text, dec_text) = format_radec(star.ra_deg, star.dec_deg);
        let (az_text, alt_text) = format_altaz(az_deg, alt_deg, 10.0);
        let ids_summary = star.catalog_ids_summary();

        v_flex()
            .id("star_details_panel")
            .size_full()
            .bg(colors.panel_background)
            .p_3()
            .gap_3()
            // 1. Header
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .size(px(8.0))
                                            .rounded_full()
                                            .flex_none()
                                            .bg(star_color),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(title),
                                    ),
                            )
                            .when_some(subtitle, |el, sub| {
                                el.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(colors.text_muted)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(sub),
                                )
                            }),
                    )
                    .child(
                        IconButton::new("close_star_details_btn", IconName::Close)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Close star details"))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.close_panel(cx);
                            })),
                    ),
            )
            .child(div().h(px(1.0)).w_full().bg(colors.border_variant))
            // 2. Visibility graph section
            .child(
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_baseline()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text)
                                            .child("Visibility"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(colors.text_muted)
                                            .child(date_label),
                                    )
                                    .when(self.cached_has_custom_horizon, |row| {
                                        let active = self.show_horizon_overlay;
                                        row.child(
                                            div()
                                                .id("star_visibility_horizon_toggle")
                                                .w(px(20.0))
                                                .h(px(20.0))
                                                .rounded_sm()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .when(active, |el| el.bg(rgb(0x22b8cf).alpha(0.16)))
                                                .hover(|h| h.bg(colors.element_hover))
                                                .tooltip(Tooltip::text(if active {
                                                    "Hide horizon line"
                                                } else {
                                                    "Show horizon line"
                                                }))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_horizon_overlay =
                                                        !this.show_horizon_overlay;
                                                    cx.notify();
                                                }))
                                                .child(
                                                    svg()
                                                        .data(ICON_HORIZON_OVERLAY)
                                                        .size(px(13.0))
                                                        .flex_none()
                                                        .text_color(if active {
                                                            rgb(0x22b8cf).into()
                                                        } else {
                                                            colors.text_muted
                                                        }),
                                                ),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(11.0))
                                    .text_color(if is_visible_now {
                                        star_color
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(alt_readout),
                            ),
                    )
                    .child(
                        div()
                            .id("star_visibility_graph")
                            .relative()
                            .w_full()
                            .h(px(GRAPH_HEIGHT_PX))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(colors.border_variant)
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    let bounds = this.graph_bounds.lock().ok().and_then(|g| *g);
                                    if let Some(bounds) = bounds {
                                        this.is_scrubbing = true;
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
                            .child(visibility_canvas)
                            .child(ref_label(60.0, "60°"))
                            .child(ref_label(30.0, "30°"))
                            .child(ref_label(0.0, "0°")),
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
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1p5()
                            .pt_1()
                            .child(rts_pill("star_rise_pill", "Rise", rise_text, rise_jump))
                            .child(rts_pill(
                                "star_transit_pill",
                                "Transit",
                                transit_text,
                                transit_jump,
                            ))
                            .child(rts_pill("star_set_pill", "Set", set_text, set_jump)),
                    ),
            )
            .child(div().h(px(1.0)).w_full().bg(colors.border_variant))
            // 3. Compact photometry, coordinates & identifiers section
            .child(
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(metric_cell("Mag", format!("G {:.2}", star.mag)))
                            .child(metric_cell(
                                "Class",
                                format!("{}-type", star.spectral_class),
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(metric_cell("BP-RP", format!("{:+.2}", star.bp_rp)))
                            .child(metric_cell("Teff", format!("~{} K", star.estimated_teff_k))),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(metric_cell("RA", ra_text))
                            .child(metric_cell("Dec", dec_text)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(metric_cell("Az", az_text))
                            .child(metric_cell("Alt", alt_text)),
                    )
                    .when_some(ids_summary, |el, summary| {
                        el.child(
                            div()
                                .pt_1()
                                .font_buffer(cx)
                                .text_size(px(10.5))
                                .text_color(colors.text_muted)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(summary),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Panel for StarDetailsPanel {
    fn persistent_name() -> &'static str {
        "star_details_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Star Details".into()
    }

    fn position(&self, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Explore)
    }

    fn starts_open(&self, _cx: &App) -> bool {
        false
    }

    fn default_size(&self, _cx: &App) -> Pixels {
        px(320.)
    }

    fn min_size(&self, _cx: &App) -> Pixels {
        px(260.)
    }
}
