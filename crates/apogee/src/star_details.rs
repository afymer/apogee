use std::sync::{Arc, Mutex};

use celestial::{
    DailyTargetVisibility, HorizonProfile, SolarSystemBody, StarMetadata,
    compute_solar_system_visibility_with_horizon, compute_target_visibility_with_horizon,
    coords::{Angle, canonical_spherical_to_cartesian},
    sample_day_equatorial_track, sample_day_solar_system_track,
};
use glam::DVec3;
use gpui::{
    App, AppContext, BorderStyle, Bounds, Context, Corners, Edges, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage, Rgba, Window, actions, div,
    fill, hsla, point, prelude::*, px, rgb, svg,
};

actions!(planetarium, [ClearSelection]);
use horizon::GlobalHorizons;
use observer::{GlobalObserver, ObserverModel, paint_twilight_backdrop, sample_day_profile};
pub use observer::{GlobalVisibilityGraphPrefs, VisibilityGraphCenter, VisibilityGraphPrefsModel};
use observer::{GraphCenterToggleStyle, graph_axis_labels, graph_center_toggle};
use planetarium::{PickedSolarSystemBody, PickedStar};
use settings::{CenterOnSelection, ToggleObjectDetails};
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

const ICON_CENTER_TARGET: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="5.5" stroke="black" stroke-width="1.6"/>
  <circle cx="12" cy="12" r="1.4" fill="black"/>
  <path d="M12 3V6.5M12 17.5V21M3 12H6.5M17.5 12H21" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
</svg>"##;

const ICON_TRAJECTORY: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3.5 17.5C6.5 8 17.5 8 20.5 17.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <circle cx="12" cy="10.4" r="2.1" fill="black"/>
  <circle cx="5.5" cy="14" r="1.2" fill="black"/>
  <circle cx="18.5" cy="14" r="1.2" fill="black"/>
</svg>"##;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectedObjectId {
    Star(u32),
    SolarSystem(SolarSystemBody),
}

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

/// Unified selected celestial object in the planetarium (either a catalog star or a solar-system body).
#[derive(Clone, Debug, PartialEq)]
pub enum SelectedObject {
    Star(SelectedStar),
    SolarSystem(SolarSystemBody),
}

impl SelectedObject {
    pub fn from_picked_star(star: &PickedStar) -> Self {
        Self::Star(SelectedStar::from_picked(star))
    }

    pub fn from_picked_solar_system(picked: &PickedSolarSystemBody) -> Self {
        Self::SolarSystem(picked.body)
    }

    pub fn id(&self) -> SelectedObjectId {
        match self {
            Self::Star(s) => SelectedObjectId::Star(s.catalog_index),
            Self::SolarSystem(body) => SelectedObjectId::SolarSystem(*body),
        }
    }

    pub fn category_badge(&self) -> &'static str {
        match self {
            Self::Star(_) => "Star",
            Self::SolarSystem(body) => body.category_label(),
        }
    }

    pub fn srgb(&self) -> [f32; 3] {
        match self {
            Self::Star(s) => s.srgb,
            Self::SolarSystem(body) => body.srgb(),
        }
    }
}

pub struct StarSelectionModel {
    selected: Option<SelectedObject>,
    activation_nonce: u64,
    /// Bumped whenever the viewport's tracking should be re-evaluated.
    center_nonce: u64,
    /// Whether the viewport should center & follow the selection at `center_nonce`.
    center_requested: bool,
    show_trajectory: bool,
}

impl Default for StarSelectionModel {
    fn default() -> Self {
        Self {
            selected: None,
            activation_nonce: 0,
            center_nonce: 0,
            center_requested: false,
            show_trajectory: true,
        }
    }
}

impl StarSelectionModel {
    pub fn select_object(&mut self, object: SelectedObject, cx: &mut Context<Self>) {
        self.selected = Some(object);
        self.show_trajectory = true;
        self.activation_nonce = self.activation_nonce.wrapping_add(1);
        // Selecting without centering releases any tracking of the previous object.
        self.center_nonce = self.center_nonce.wrapping_add(1);
        self.center_requested = false;
        cx.notify();
    }

    /// Selects the object and also asks the viewport to center and follow it.
    pub fn select_and_center(&mut self, object: SelectedObject, cx: &mut Context<Self>) {
        self.selected = Some(object);
        self.show_trajectory = true;
        self.activation_nonce = self.activation_nonce.wrapping_add(1);
        self.center_nonce = self.center_nonce.wrapping_add(1);
        self.center_requested = true;
        cx.notify();
    }

    /// Asks the viewport to center and follow the current selection.
    pub fn recenter(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.center_nonce = self.center_nonce.wrapping_add(1);
            self.center_requested = true;
            cx.notify();
        }
    }

    pub fn hide_trajectory(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() && self.show_trajectory {
            self.show_trajectory = false;
            cx.notify();
        }
    }

    pub fn toggle_trajectory(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            self.show_trajectory = !self.show_trajectory;
            cx.notify();
        }
    }

    pub fn show_trajectory(&self) -> bool {
        self.show_trajectory
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.selected.take().is_some() {
            self.show_trajectory = true;
            self.activation_nonce = self.activation_nonce.wrapping_add(1);
            self.center_nonce = self.center_nonce.wrapping_add(1);
            self.center_requested = false;
            cx.notify();
        }
    }

    pub fn selected_object(&self) -> Option<&SelectedObject> {
        self.selected.as_ref()
    }

    pub fn activation_nonce(&self) -> u64 {
        self.activation_nonce
    }

    pub fn center_nonce(&self) -> u64 {
        self.center_nonce
    }

    pub fn center_requested(&self) -> bool {
        self.center_requested
    }
}

#[derive(Clone)]
pub struct GlobalStarSelection(pub Entity<StarSelectionModel>);

impl gpui::Global for GlobalStarSelection {}

#[derive(Clone)]
pub struct GlobalStarDetailsPanel(pub Entity<StarDetailsPanel>);

impl gpui::Global for GlobalStarDetailsPanel {}

pub fn init(cx: &mut App) {
    let selection = cx.new(|_| StarSelectionModel::default());
    cx.set_global(GlobalStarSelection(selection));

    let graph_prefs = cx.new(|_| VisibilityGraphPrefsModel::default());
    cx.set_global(GlobalVisibilityGraphPrefs(graph_prefs));

    cx.bind_keys([KeyBinding::new("escape", ClearSelection, None)]);
    cx.on_action(|_: &ClearSelection, cx| {
        if let Some(selection) = cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone()) {
            selection.update(cx, |model, cx| {
                model.clear(cx);
            });
        }
    });

    cx.on_action(|_: &CenterOnSelection, cx| {
        if let Some(selection) = cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone()) {
            selection.update(cx, |model, cx| {
                model.recenter(cx);
            });
        }
    });

    cx.on_action(|_: &ToggleObjectDetails, cx| {
        if let Some(panel) = cx
            .try_global::<GlobalStarDetailsPanel>()
            .map(|g| g.0.clone())
        {
            panel.update(cx, |this, cx| {
                this.toggle_panel(cx);
            });
        }
    });

    cx.observe_new(
        |workbench: &mut workspace::workbench::Workbench, window, cx| {
            if let Some(window) = window {
                let panel = StarDetailsPanel::build(cx);
                cx.set_global(GlobalStarDetailsPanel(panel.clone()));
                cx.observe(&panel, |_this, _panel, cx| {
                    cx.notify();
                })
                .detach();
                workbench.add_panel(panel, window, cx);
            }
        },
    )
    .detach();
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TargetDayCacheKey {
    pub target_id: SelectedObjectId,
    pub lat_bits: u64,
    pub lon_bits: u64,
    pub elev_bits: u64,
    pub local_midnight_secs: i64,
    pub horizon_key: Option<(String, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GraphImageKey {
    phys_w: u32,
    phys_h: u32,
    profile_gen: u64,
    show_horizon: bool,
    rgb_bits: [u32; 3],
}

struct CachedGraphImage {
    key: GraphImageKey,
    image: Arc<RenderImage>,
}

pub struct StarDetailsPanel {
    focus_handle: FocusHandle,
    last_activation_nonce: u64,
    cache_key: Option<TargetDayCacheKey>,
    profile_gen: u64,
    cached_solar_profile: Arc<Vec<f64>>,
    cached_star_profile: Arc<Vec<f64>>,
    cached_horizon_profile: Arc<Vec<f64>>,
    cached_visibility: Option<DailyTargetVisibility>,
    cached_has_custom_horizon: bool,
    cached_active_horizon: Option<HorizonProfile>,
    cached_window_start_secs: f64,
    show_horizon_overlay: bool,
    graph_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    cached_graph_image: Arc<Mutex<Option<CachedGraphImage>>>,
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
                let has_selection = model.read(cx).selected_object().is_some();
                this.ensure_profiles_fresh(cx);
                if nonce != this.last_activation_nonce {
                    this.last_activation_nonce = nonce;
                    if has_selection {
                        cx.emit(PanelEvent::Activate);
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

            if let Some(prefs_entity) = cx
                .try_global::<GlobalVisibilityGraphPrefs>()
                .map(|g| g.0.clone())
            {
                cx.observe(&prefs_entity, |this: &mut Self, _model, cx| {
                    this.ensure_profiles_fresh(cx);
                    cx.notify();
                })
                .detach();
            }

            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                last_activation_nonce: initial_nonce,
                cache_key: None,
                profile_gen: 0,
                cached_solar_profile: Arc::new(Vec::new()),
                cached_star_profile: Arc::new(Vec::new()),
                cached_horizon_profile: Arc::new(Vec::new()),
                cached_visibility: None,
                cached_has_custom_horizon: false,
                cached_active_horizon: None,
                cached_window_start_secs: 0.0,
                show_horizon_overlay: true,
                graph_bounds: Arc::new(Mutex::new(None)),
                cached_graph_image: Arc::new(Mutex::new(None)),
                is_scrubbing: false,
            };
            panel.ensure_profiles_fresh(cx);
            panel
        })
    }

    fn ensure_profiles_fresh(&mut self, cx: &App) {
        let Some(selected) = cx
            .try_global::<GlobalStarSelection>()
            .and_then(|g| g.0.read(cx).selected_object().cloned())
        else {
            if self.cache_key.take().is_some() {
                self.profile_gen = self.profile_gen.wrapping_add(1);
            }
            self.cached_solar_profile = Arc::new(Vec::new());
            self.cached_star_profile = Arc::new(Vec::new());
            self.cached_horizon_profile = Arc::new(Vec::new());
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

        let graph_center = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.read(cx).center())
            .unwrap_or_default();
        let loc = observer.geodetic_position();
        let window_start = graph_center.window_start_unix_seconds(
            observer.local_midnight_unix_seconds(),
            observer.local_hour_of_day(),
        );
        let next_key = TargetDayCacheKey {
            target_id: selected.id(),
            lat_bits: loc.latitude_deg().to_bits(),
            lon_bits: loc.longitude_deg().to_bits(),
            elev_bits: loc.elevation_m().to_bits(),
            local_midnight_secs: window_start.round() as i64,
            horizon_key,
        };

        if self.cache_key.as_ref() == Some(&next_key) {
            return;
        }

        self.cached_solar_profile = Arc::new(celestial::solar::sample_day_solar_profile(
            loc,
            window_start,
            97,
        ));
        let (track, visibility) = match &selected {
            SelectedObject::Star(star) => {
                let ra = Angle::from_degrees(star.ra_deg);
                let dec = Angle::from_degrees(star.dec_deg);
                let track = sample_day_equatorial_track(loc, window_start, ra, dec, 97);
                let vis = compute_target_visibility_with_horizon(
                    loc,
                    window_start,
                    ra,
                    dec,
                    active_profile.as_ref(),
                );
                (track, vis)
            }
            SelectedObject::SolarSystem(body) => {
                let track = sample_day_solar_system_track(loc, window_start, *body, 97);
                let vis = compute_solar_system_visibility_with_horizon(
                    loc,
                    window_start,
                    *body,
                    active_profile.as_ref(),
                );
                (track, vis)
            }
        };

        self.cached_star_profile = Arc::new(track.iter().map(|(_, alt)| *alt).collect());
        self.cached_horizon_profile = Arc::new(if let Some(ref prof) = active_profile {
            track
                .iter()
                .map(|(az, _)| prof.altitude_at_azimuth_deg(*az))
                .collect()
        } else {
            Vec::new()
        });
        self.cached_visibility = Some(visibility);
        self.cached_has_custom_horizon = active_profile.is_some();
        self.cached_active_horizon = active_profile;
        self.cached_window_start_secs = window_start;
        self.cache_key = Some(next_key);
        self.profile_gen = self.profile_gen.wrapping_add(1);
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
        let snapped_offset_hour = (total_mins / 60.0).clamp(0.0, 23.0 + 59.0 / 60.0);
        let target_unix_secs = self.cached_window_start_secs + snapped_offset_hour * 3600.0;

        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            observer.update(cx, |model, cx| {
                model.set_manual_unix_seconds(target_unix_secs, cx);
            });
        }
    }

    fn jump_to_window_offset_hour(&self, offset_hour: f64, cx: &mut Context<Self>) {
        let target_unix_secs =
            self.cached_window_start_secs + offset_hour.clamp(0.0, 24.0) * 3600.0;
        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            observer.update(cx, |model, cx| {
                model.set_manual_unix_seconds(target_unix_secs, cx);
            });
        }
    }

    fn close_panel(&self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Close);
    }

    fn toggle_panel(&self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Toggle);
    }
}

#[inline]
pub(crate) fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[inline]
pub(crate) fn alt_to_y_fraction(alt_deg: f64, pad_y: f32, height: f32) -> f32 {
    let usable_h = (height - 2.0 * pad_y).max(4.0);
    let norm = ((alt_deg - GRAPH_MIN_ALT_DEG) / (GRAPH_MAX_ALT_DEG - GRAPH_MIN_ALT_DEG))
        .clamp(0.0, 1.0) as f32;
    pad_y + usable_h * (1.0 - norm)
}

#[inline]
fn alt_to_y(alt_deg: f64, bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.origin.y)
        + alt_to_y_fraction(alt_deg, GRAPH_PAD_Y, f32::from(bounds.size.height))
}

/// Paints a circle as a GPU-instanced rounded quad (avoiding per-frame CPU polygon tessellation).
pub(crate) fn paint_disc(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    fill_color: Rgba,
    border_width: f32,
    border_color: Rgba,
) {
    let outer_r = radius + border_width;
    let diameter = outer_r * 2.0;
    window.paint_quad(gpui::quad(
        Bounds::new(
            point(center.x - px(outer_r), center.y - px(outer_r)),
            gpui::size(px(diameter), px(diameter)),
        ),
        Corners::all(px(outer_r)),
        fill_color,
        Edges::all(px(border_width)),
        border_color,
        BorderStyle::Solid,
    ));
}

#[inline]
fn blend_bgra_pixel(buf: &mut [u8], idx: usize, r: f32, g: f32, b: f32, a: f32) {
    if a <= 1e-4 {
        return;
    }
    let dst_b = f32::from(buf[idx]) / 255.0;
    let dst_g = f32::from(buf[idx + 1]) / 255.0;
    let dst_r = f32::from(buf[idx + 2]) / 255.0;
    let dst_a = f32::from(buf[idx + 3]) / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    if out_a <= 1e-5 {
        return;
    }
    let inv = 1.0 / out_a;
    let out_r = (r * a + dst_r * dst_a * (1.0 - a)) * inv;
    let out_g = (g * a + dst_g * dst_a * (1.0 - a)) * inv;
    let out_b = (b * a + dst_b * dst_a * (1.0 - a)) * inv;
    buf[idx] = (out_b.clamp(0.0, 1.0) * 255.0).round() as u8;
    buf[idx + 1] = (out_g.clamp(0.0, 1.0) * 255.0).round() as u8;
    buf[idx + 2] = (out_r.clamp(0.0, 1.0) * 255.0).round() as u8;
    buf[idx + 3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

/// Rasterizes an antialiased polyline stroke into a BGRA buffer using exact segment-distance SDF.
fn rasterize_aa_polyline(
    buf: &mut [u8],
    w: u32,
    h: u32,
    segments: &[((f32, f32), (f32, f32))],
    half_width: f32,
    color: Rgba,
) {
    let pad = half_width + 1.2;
    let w_i = w as i32;
    let h_i = h as i32;
    let mut cov = vec![0.0_f32; (w * h) as usize];

    for &((x0, y0), (x1, y1)) in segments {
        let min_x = ((x0.min(x1) - pad).floor() as i32).clamp(0, w_i - 1);
        let max_x = ((x0.max(x1) + pad).ceil() as i32).clamp(0, w_i - 1);
        let min_y = ((y0.min(y1) - pad).floor() as i32).clamp(0, h_i - 1);
        let max_y = ((y0.max(y1) + pad).ceil() as i32).clamp(0, h_i - 1);
        let vx = x1 - x0;
        let vy = y1 - y0;
        let len_sq = vx * vx + vy * vy;

        for py in min_y..=max_y {
            let cy = py as f32 + 0.5;
            let row = (py as u32 * w) as usize;
            for px_i in min_x..=max_x {
                let cx = px_i as f32 + 0.5;
                let t = if len_sq > 1e-6 {
                    (((cx - x0) * vx + (cy - y0) * vy) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let dx = cx - (x0 + vx * t);
                let dy = cy - (y0 + vy * t);
                let dist = dx.hypot(dy);
                let c = (half_width + 0.5 - dist).clamp(0.0, 1.0);
                if c > 0.0 {
                    let slot = &mut cov[row + px_i as usize];
                    if c > *slot {
                        *slot = c;
                    }
                }
            }
        }
    }

    for (i, &c) in cov.iter().enumerate() {
        if c > 0.0 {
            blend_bgra_pixel(buf, i * 4, color.r, color.g, color.b, color.a * c);
        }
    }
}

/// Rasterizes the static horizon silhouette and elevation curves once into a cached BGRA `RenderImage`.
fn rasterize_graph_overlay(
    phys_w: u32,
    phys_h: u32,
    scale: f32,
    star_profile: &[f64],
    horizon_profile: &[f64],
    star_rgb: Rgba,
) -> Arc<RenderImage> {
    rasterize_graph_overlay_with_options(
        phys_w,
        phys_h,
        scale,
        GRAPH_PAD_Y,
        1.0,
        star_profile,
        horizon_profile,
        star_rgb,
    )
}

/// Rasterizes a horizon silhouette and elevation curve into a BGRA `RenderImage` with configurable padding and stroke scale.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rasterize_graph_overlay_with_options(
    phys_w: u32,
    phys_h: u32,
    scale: f32,
    pad_y_logical: f32,
    stroke_scale: f32,
    star_profile: &[f64],
    horizon_profile: &[f64],
    star_rgb: Rgba,
) -> Arc<RenderImage> {
    let w = phys_w.max(2);
    let h = phys_h.max(2);
    let wf = w as f32;
    let hf = h as f32;
    let pad_y = pad_y_logical * scale;
    let stroke = scale * stroke_scale;
    let mut buf = vec![0u8; (w * h * 4) as usize];

    let y_of = |alt: f64| alt_to_y_fraction(alt, pad_y, hf);

    // 1. Custom horizon obstruction silhouette & smooth top edge
    if horizon_profile.len() >= 2 {
        let sil = rgb(0x02040a).alpha(0.5);
        for px_i in 0..w {
            let hour = (px_i as f64 + 0.5) / f64::from(wf) * 24.0;
            let yh = y_of(sample_day_profile(horizon_profile, hour));
            let start_y = (yh.floor() as i32).clamp(0, h as i32);
            for py in start_y..h as i32 {
                let cov = ((py as f32 + 1.0) - yh).clamp(0.0, 1.0);
                let idx = ((py as u32 * w + px_i) * 4) as usize;
                blend_bgra_pixel(&mut buf, idx, sil.r, sil.g, sil.b, sil.a * cov);
            }
        }

        let denom_h = (horizon_profile.len() - 1) as f32;
        let h_segs: Vec<_> = (1..horizon_profile.len())
            .map(|i| {
                let p0 = ((i - 1) as f32 / denom_h * wf, y_of(horizon_profile[i - 1]));
                let p1 = (i as f32 / denom_h * wf, y_of(horizon_profile[i]));
                (p0, p1)
            })
            .collect();
        rasterize_aa_polyline(
            &mut buf,
            w,
            h,
            &h_segs,
            0.55 * stroke,
            rgb(0xffffff).alpha(0.28),
        );
    }

    // 2. Target elevation curve (faint full track + above-horizon dark halo & crisp stroke)
    if star_profile.len() >= 2 {
        let denom = (star_profile.len() - 1) as f32;
        let mut all_segs = Vec::with_capacity(star_profile.len() - 1);
        let mut above_segs = Vec::with_capacity(star_profile.len() - 1);

        for i in 1..star_profile.len() {
            let a0 = star_profile[i - 1];
            let a1 = star_profile[i];
            let x0 = (i - 1) as f32 / denom * wf;
            let x1 = i as f32 / denom * wf;
            let p0 = (x0, y_of(a0));
            let p1 = (x1, y_of(a1));
            all_segs.push((p0, p1));

            let h0 = horizon_profile.get(i - 1).copied().unwrap_or(0.0);
            let h1 = horizon_profile.get(i).copied().unwrap_or(0.0);
            let c0 = a0 - h0;
            let c1 = a1 - h1;
            if c0 >= 0.0 && c1 >= 0.0 {
                above_segs.push((p0, p1));
            } else if c0 < 0.0 && c1 >= 0.0 {
                let frac = ((-c0) / (c1 - c0).max(1e-6)).clamp(0.0, 1.0) as f32;
                let cross = (
                    x0 + (x1 - x0) * frac,
                    y_of(a0 + (a1 - a0) * f64::from(frac)),
                );
                above_segs.push((cross, p1));
            } else if c0 >= 0.0 && c1 < 0.0 {
                let frac = (c0 / (c0 - c1).max(1e-6)).clamp(0.0, 1.0) as f32;
                let cross = (
                    x0 + (x1 - x0) * frac,
                    y_of(a0 + (a1 - a0) * f64::from(frac)),
                );
                above_segs.push((p0, cross));
            }
        }

        let crisp_color = lerp_rgba(star_rgb, rgb(0xffffff), 0.25).alpha(0.96);
        rasterize_aa_polyline(
            &mut buf,
            w,
            h,
            &all_segs,
            0.75 * stroke,
            star_rgb.alpha(0.38),
        );
        if !above_segs.is_empty() {
            rasterize_aa_polyline(
                &mut buf,
                w,
                h,
                &above_segs,
                1.9 * stroke,
                rgb(0x060a14).alpha(0.55),
            );
            rasterize_aa_polyline(&mut buf, w, h, &above_segs, 1.1 * stroke, crisp_color);
        }
    }

    let img = image::RgbaImage::from_raw(w, h, buf).expect("buffer dimensions match w * h * 4");
    Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
}

/// Rasterizes only a target elevation curve (without drawing any horizon silhouette):
/// segments below the effective horizon are drawn muted gray, while visible segments use `star_rgb`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rasterize_visibility_curve_only(
    phys_w: u32,
    phys_h: u32,
    scale: f32,
    pad_y_logical: f32,
    stroke_scale: f32,
    star_profile: &[f64],
    horizon_profile: &[f64],
    star_rgb: Rgba,
) -> Arc<RenderImage> {
    let w = phys_w.max(2);
    let h = phys_h.max(2);
    let wf = w as f32;
    let hf = h as f32;
    let pad_y = pad_y_logical * scale;
    let stroke = scale * stroke_scale;
    let mut buf = vec![0u8; (w * h * 4) as usize];

    let y_of = |alt: f64| alt_to_y_fraction(alt, pad_y, hf);

    if star_profile.len() >= 2 {
        let denom = (star_profile.len() - 1) as f32;
        let mut hidden_segs = Vec::with_capacity(star_profile.len() - 1);
        let mut visible_segs = Vec::with_capacity(star_profile.len() - 1);

        for i in 1..star_profile.len() {
            let a0 = star_profile[i - 1];
            let a1 = star_profile[i];
            let x0 = (i - 1) as f32 / denom * wf;
            let x1 = i as f32 / denom * wf;
            let p0 = (x0, y_of(a0));
            let p1 = (x1, y_of(a1));

            let h0 = horizon_profile.get(i - 1).copied().unwrap_or(0.0);
            let h1 = horizon_profile.get(i).copied().unwrap_or(0.0);
            let c0 = a0 - h0;
            let c1 = a1 - h1;

            if c0 >= 0.0 && c1 >= 0.0 {
                visible_segs.push((p0, p1));
            } else if c0 < 0.0 && c1 < 0.0 {
                hidden_segs.push((p0, p1));
            } else if c0 < 0.0 && c1 >= 0.0 {
                let frac = ((-c0) / (c1 - c0).max(1e-6)).clamp(0.0, 1.0) as f32;
                let cross = (
                    x0 + (x1 - x0) * frac,
                    y_of(a0 + (a1 - a0) * f64::from(frac)),
                );
                hidden_segs.push((p0, cross));
                visible_segs.push((cross, p1));
            } else {
                let frac = (c0 / (c0 - c1).max(1e-6)).clamp(0.0, 1.0) as f32;
                let cross = (
                    x0 + (x1 - x0) * frac,
                    y_of(a0 + (a1 - a0) * f64::from(frac)),
                );
                visible_segs.push((p0, cross));
                hidden_segs.push((cross, p1));
            }
        }

        if !hidden_segs.is_empty() {
            rasterize_aa_polyline(
                &mut buf,
                w,
                h,
                &hidden_segs,
                0.85 * stroke,
                rgb(0x8a94a8).alpha(0.55),
            );
        }
        if !visible_segs.is_empty() {
            let crisp_color = lerp_rgba(star_rgb, rgb(0xffffff), 0.25).alpha(0.96);
            rasterize_aa_polyline(
                &mut buf,
                w,
                h,
                &visible_segs,
                1.75 * stroke,
                rgb(0x060a14).alpha(0.45),
            );
            rasterize_aa_polyline(&mut buf, w, h, &visible_segs, 1.05 * stroke, crisp_color);
        }
    }

    let img = image::RgbaImage::from_raw(w, h, buf).expect("buffer dimensions match w * h * 4");
    Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
}

#[allow(clippy::too_many_arguments)]
fn paint_visibility_graph(
    bounds: Bounds<Pixels>,
    window_offset_hour: f64,
    solar_profile: &[f64],
    star_profile: &[f64],
    horizon_profile: &[f64],
    visibility: Option<DailyTargetVisibility>,
    star_rgb: Rgba,
    profile_gen: u64,
    image_cache: &Mutex<Option<CachedGraphImage>>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    // 1. Continuous 24-hour twilight sky backdrop across the active window.
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
        Bounds::new(
            point(px(origin_x), px(horizon_y)),
            gpui::size(px(width), px(1.0)),
        ),
        hsla(0.0, 0.0, 1.0, 0.34),
    ));

    // 4. Cached antialiased horizon silhouette + target curves (single textured quad, 0 `paint_path`).
    let scale = window.scale_factor().max(1.0);
    let phys_w = (width * scale).round().max(2.0) as u32;
    let phys_h = (height * scale).round().max(2.0) as u32;
    let key = GraphImageKey {
        phys_w,
        phys_h,
        profile_gen,
        show_horizon: !horizon_profile.is_empty(),
        rgb_bits: [
            star_rgb.r.to_bits(),
            star_rgb.g.to_bits(),
            star_rgb.b.to_bits(),
        ],
    };
    let overlay_img = image_cache.lock().ok().map(|mut guard| {
        if let Some(cached) = guard.as_ref().filter(|c| c.key == key) {
            return cached.image.clone();
        }
        let image = rasterize_graph_overlay(
            phys_w,
            phys_h,
            scale,
            star_profile,
            horizon_profile,
            star_rgb,
        );
        *guard = Some(CachedGraphImage {
            key,
            image: image.clone(),
        });
        image
    });
    if let Some(img) = overlay_img {
        let _ = window.paint_image(bounds, bounds, Corners::default(), img, 0, false);
    }

    let crisp_color = lerp_rgba(star_rgb, rgb(0xffffff), 0.25).alpha(0.96);

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
            paint_disc(
                window,
                point(px(peak_x), px(peak_y)),
                2.5,
                crisp_color,
                1.3,
                rgb(0x090d1a).alpha(0.8),
            );
        }
    }

    // 5. Playhead beam and bordered orb at active time (pure GPU quads).
    let t = (window_offset_hour / 24.0).clamp(0.0, 1.0) as f32;
    let playhead_x = origin_x + t * width;
    let current_alt = sample_day_profile(star_profile, window_offset_hour);
    let handle_y = alt_to_y(current_alt, bounds);
    let handle_pt = point(px(playhead_x), px(handle_y));

    window.paint_quad(fill(
        Bounds::new(
            point(px(playhead_x - 0.5), px(origin_y)),
            gpui::size(px(1.0), px(height)),
        ),
        hsla(0.0, 0.0, 1.0, 0.35),
    ));

    paint_disc(
        window,
        handle_pt,
        4.5,
        rgb(0xf2f6fc).alpha(0.96),
        1.0,
        rgb(0x090d1a).alpha(0.88),
    );
    paint_disc(
        window,
        handle_pt,
        2.9,
        star_rgb.alpha(1.0),
        0.0,
        rgb(0x090d1a).alpha(0.0),
    );
}

pub(crate) fn format_local_hour_hm(hour: f64) -> String {
    let total_mins = (hour * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", total_mins / 60, total_mins % 60)
}

impl Render for StarDetailsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (selected, show_trajectory) = cx
            .try_global::<GlobalStarSelection>()
            .map(|g| {
                let model = g.0.read(cx);
                (model.selected_object().cloned(), model.show_trajectory())
            })
            .unwrap_or((None, true));

        let Some(selected_obj) = selected else {
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
                                .child("Object Details"),
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
                        .child(
                            "Click a star, the Sun, or the Moon to inspect visibility and details",
                        ),
                )
                .into_any_element();
        };

        let srgb = selected_obj.srgb();
        let star_rgb = Rgba {
            r: srgb[0],
            g: srgb[1],
            b: srgb[2],
            a: 1.0,
        };
        let star_color: Hsla = star_rgb.into();

        let obs_ctx = cx
            .try_global::<GlobalObserver>()
            .map(|g| {
                let model: &ObserverModel = g.0.read(cx);
                model.observation_context()
            })
            .unwrap_or_default();

        let category_badge = selected_obj.category_badge();
        let is_solar_system = matches!(selected_obj, SelectedObject::SolarSystem(_));

        let (title, subtitle, ra_deg, dec_deg, az_deg, alt_deg, solar_eph) = match &selected_obj {
            SelectedObject::Star(star) => {
                let (title, subtitle) = star.display_title_and_subtitle();
                let (az, alt) = obs_ctx.equatorial_to_topocentric_coords(
                    Angle::from_degrees(star.ra_deg),
                    Angle::from_degrees(star.dec_deg),
                );
                (
                    title,
                    subtitle,
                    star.ra_deg,
                    star.dec_deg,
                    az.degrees(),
                    alt.degrees(),
                    None,
                )
            }
            SelectedObject::SolarSystem(body) => {
                let eph = body.ephemeris(obs_ctx.location, obs_ctx.epoch);
                let (title, sub) = eph.display_title_and_subtitle();
                (
                    title,
                    Some(sub),
                    eph.ra_deg,
                    eph.dec_deg,
                    eph.az_deg,
                    eph.alt_deg,
                    Some(eph),
                )
            }
        };

        let local_horizon_alt = self
            .cached_active_horizon
            .as_ref()
            .map(|h| h.altitude_at_azimuth_deg(az_deg));
        let is_visible_now = local_horizon_alt.map_or(alt_deg >= 0.0, |h| alt_deg >= h);
        let alt_readout = format!("{alt_deg:+.1}°");

        let solar_profile = self.cached_solar_profile.clone();
        let star_profile = self.cached_star_profile.clone();
        let horizon_profile = if self.show_horizon_overlay {
            self.cached_horizon_profile.clone()
        } else {
            Arc::new(Vec::new())
        };
        let visibility = self.cached_visibility;
        let profile_gen = self.profile_gen;
        let image_cache = self.cached_graph_image.clone();
        let bounds_store = self.graph_bounds.clone();
        let entity = cx.entity().clone();

        let graph_center = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.read(cx).center())
            .unwrap_or_default();
        let window_offset_hour = ((obs_ctx.epoch_unix_seconds() - self.cached_window_start_secs)
            / 3600.0)
            .clamp(0.0, 24.0);

        let visibility_canvas = gpui::canvas(
            move |bounds, _window, _cx| {
                if let Ok(mut guard) = bounds_store.lock() {
                    *guard = Some(bounds);
                }
            },
            move |bounds, (), window, _cx| {
                paint_visibility_graph(
                    bounds,
                    window_offset_hour,
                    &solar_profile,
                    &star_profile,
                    &horizon_profile,
                    visibility,
                    star_rgb,
                    profile_gen,
                    &image_cache,
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

        let rts_pill =
            |id: &'static str, label: &'static str, value: String, jump_hour: Option<f64>| {
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
                    .when_some(jump_hour, |el, offset_hour| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(colors.element_hover))
                            .tooltip(Tooltip::text(format!(
                                "Jump to {}",
                                format_local_hour_hm(graph_center.clock_hour(offset_hour))
                            )))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.jump_to_window_offset_hour(offset_hour, cx);
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
                    format_local_hour_hm(graph_center.clock_hour(vis.transit_hour)),
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
                        vis.rise_hour.map_or_else(
                            || "—".to_string(),
                            |h| format_local_hour_hm(graph_center.clock_hour(h)),
                        ),
                        vis.rise_hour,
                        transit_val,
                        Some(vis.transit_hour),
                        vis.set_hour.map_or_else(
                            || "—".to_string(),
                            |h| format_local_hour_hm(graph_center.clock_hour(h)),
                        ),
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

        let (ra_text, dec_text) = format_radec(ra_deg, dec_deg);
        let (az_text, alt_text) = format_altaz(az_deg, alt_deg, 10.0);
        let ids_summary = match &selected_obj {
            SelectedObject::Star(star) => star.catalog_ids_summary(),
            SelectedObject::SolarSystem(_) => None,
        };

        let specific_rows = match (&selected_obj, solar_eph) {
            (SelectedObject::Star(star), _) => vec![
                h_flex()
                    .gap_3()
                    .child(metric_cell("Mag", format!("G {:.2}", star.mag)))
                    .child(metric_cell(
                        "Class",
                        format!("{}-type", star.spectral_class),
                    )),
                h_flex()
                    .gap_3()
                    .child(metric_cell("BP-RP", format!("{:+.2}", star.bp_rp)))
                    .child(metric_cell("Teff", format!("~{} K", star.estimated_teff_k))),
            ],
            (SelectedObject::SolarSystem(SolarSystemBody::Sun), Some(eph)) => {
                let dist_au = eph.distance_km / 149_597_870.7;
                let diam_arcmin = eph.angular_radius_deg * 2.0 * 60.0;
                let sky_phase = celestial::SkyPhase::from_solar_altitude_deg(eph.alt_deg).label();
                vec![
                    h_flex()
                        .gap_3()
                        .child(metric_cell(
                            "Mag",
                            format!("V {:.2}", eph.apparent_magnitude),
                        ))
                        .child(metric_cell("Diam", format!("{diam_arcmin:.1}'"))),
                    h_flex()
                        .gap_3()
                        .child(metric_cell("Dist", format!("{dist_au:.4} AU")))
                        .child(metric_cell("Sky", sky_phase.to_string())),
                ]
            }
            (SelectedObject::SolarSystem(SolarSystemBody::Moon), Some(eph)) => {
                let diam_arcmin = eph.angular_radius_deg * 2.0 * 60.0;
                let (phase_label, illum_label, age_label) = if let Some(phase) = eph.lunar_phase {
                    (
                        phase.phase_name().to_string(),
                        format!("{:.1}%", phase.illuminated_fraction * 100.0),
                        format!("{:.1} d", phase.age_days),
                    )
                } else {
                    ("—".to_string(), "—".to_string(), "—".to_string())
                };
                vec![
                    h_flex()
                        .gap_3()
                        .child(metric_cell("Phase", phase_label))
                        .child(metric_cell("Illum", illum_label)),
                    h_flex()
                        .gap_3()
                        .child(metric_cell(
                            "Mag",
                            format!("V {:.2}", eph.apparent_magnitude),
                        ))
                        .child(metric_cell("Age", age_label)),
                    h_flex()
                        .gap_3()
                        .child(metric_cell("Diam", format!("{diam_arcmin:.1}'")))
                        .child(metric_cell("Dist", format!("{:.0} km", eph.distance_km))),
                ]
            }
            (SelectedObject::SolarSystem(_), None) => Vec::new(),
        };

        let details_metrics_section = v_flex()
            .w_full()
            .gap_1p5()
            .children(specific_rows)
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
            });

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
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .px_1p5()
                                            .py(px(1.0))
                                            .rounded_sm()
                                            .bg(if is_solar_system {
                                                star_color.opacity(0.18)
                                            } else {
                                                colors.element_background
                                            })
                                            .border_1()
                                            .border_color(if is_solar_system {
                                                star_color.opacity(0.45)
                                            } else {
                                                colors.border_variant
                                            })
                                            .text_size(px(9.5))
                                            .text_color(if is_solar_system {
                                                star_color
                                            } else {
                                                colors.text_muted
                                            })
                                            .child(category_badge),
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
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("recenter_selected_object_btn")
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.ghost_element_hover))
                                    .tooltip(Tooltip::text("Center & follow object (C)"))
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        if let Some(selection) = cx
                                            .try_global::<GlobalStarSelection>()
                                            .map(|g| g.0.clone())
                                        {
                                            selection.update(cx, |model, cx| {
                                                model.recenter(cx);
                                            });
                                        }
                                    }))
                                    .child(
                                        svg()
                                            .data(ICON_CENTER_TARGET)
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(colors.text_muted),
                                    ),
                            )
                            .child(
                                div()
                                    .id("toggle_trajectory_btn")
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .when(show_trajectory, |s| s.bg(colors.ghost_element_selected))
                                    .hover(|s| s.bg(colors.ghost_element_hover))
                                    .tooltip(Tooltip::text(if show_trajectory {
                                        "Hide trajectory"
                                    } else {
                                        "Show trajectory"
                                    }))
                                    .on_click(cx.listener(|_this, _event, _window, cx| {
                                        if let Some(selection) = cx
                                            .try_global::<GlobalStarSelection>()
                                            .map(|g| g.0.clone())
                                        {
                                            selection.update(cx, |model, cx| {
                                                model.toggle_trajectory(cx);
                                            });
                                        }
                                    }))
                                    .child(
                                        svg()
                                            .data(ICON_TRAJECTORY)
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(if show_trajectory {
                                                colors.text
                                            } else {
                                                colors.text_muted.opacity(0.6)
                                            }),
                                    ),
                            )
                            .child(
                                IconButton::new("close_star_details_btn", IconName::Close)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text("Close details"))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.close_panel(cx);
                                    })),
                            ),
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
                                h_flex().items_baseline().gap_1p5().child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text)
                                        .child("Visibility"),
                                ),
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
                            .child(ref_label(0.0, "0°"))
                            .child(
                                graph_center_toggle(
                                    "star_visibility_center_toggle",
                                    GraphCenterToggleStyle::Overlay,
                                    cx,
                                )
                                .absolute()
                                .top(px(4.0))
                                .left(px(4.0)),
                            )
                            .when(self.cached_has_custom_horizon, |el| {
                                let active = self.show_horizon_overlay;
                                el.child(
                                    div()
                                        .id("star_visibility_horizon_toggle")
                                        .absolute()
                                        .top(px(4.0))
                                        .right(px(4.0))
                                        .w(px(20.0))
                                        .h(px(20.0))
                                        .rounded_sm()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(hsla(0.0, 0.0, 0.0, if active { 0.38 } else { 0.18 }))
                                        .hover(|h| h.bg(hsla(0.0, 0.0, 0.0, 0.55)))
                                        .tooltip(Tooltip::text(if active {
                                            "Hide horizon line"
                                        } else {
                                            "Show horizon line"
                                        }))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_horizon_overlay = !this.show_horizon_overlay;
                                            cx.notify();
                                        }))
                                        .child(
                                            svg()
                                                .data(ICON_HORIZON_OVERLAY)
                                                .size(px(13.0))
                                                .flex_none()
                                                .text_color(hsla(
                                                    0.0,
                                                    0.0,
                                                    1.0,
                                                    if active { 0.85 } else { 0.45 },
                                                )),
                                        ),
                                )
                            }),
                    )
                    .child(graph_axis_labels(graph_center, cx))
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
            // 3. Compact photometry / solar-system ephemeris, coordinates & identifiers section
            .child(details_metrics_section)
            .into_any_element()
    }
}

impl Panel for StarDetailsPanel {
    fn persistent_name() -> &'static str {
        "star_details_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Object Details".into()
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn hide_and_toggle_trajectory_preserve_selection_and_nonce(cx: &mut TestAppContext) {
        let selection = cx.new(|_| StarSelectionModel::default());
        selection.update(cx, |model, cx| {
            model.select_object(SelectedObject::SolarSystem(SolarSystemBody::Sun), cx);
        });
        let nonce_after_select = selection.read_with(cx, |m, _| {
            assert!(m.show_trajectory());
            assert_eq!(
                m.selected_object(),
                Some(&SelectedObject::SolarSystem(SolarSystemBody::Sun))
            );
            m.activation_nonce()
        });

        // Right-clicking hides trajectory while keeping selection and camera tracking nonce intact.
        selection.update(cx, |model, cx| {
            model.hide_trajectory(cx);
        });
        selection.read_with(cx, |m, _| {
            assert!(!m.show_trajectory());
            assert_eq!(
                m.selected_object(),
                Some(&SelectedObject::SolarSystem(SolarSystemBody::Sun))
            );
            assert_eq!(m.activation_nonce(), nonce_after_select);
        });

        // Toggle button shows it again, then hides it again.
        selection.update(cx, |model, cx| {
            model.toggle_trajectory(cx);
        });
        selection.read_with(cx, |m, _| {
            assert!(m.show_trajectory());
            assert_eq!(m.activation_nonce(), nonce_after_select);
        });

        selection.update(cx, |model, cx| {
            model.toggle_trajectory(cx);
        });
        selection.read_with(cx, |m, _| {
            assert!(!m.show_trajectory());
        });

        // Selecting another object restores trajectory visibility.
        selection.update(cx, |model, cx| {
            model.select_object(SelectedObject::SolarSystem(SolarSystemBody::Moon), cx);
        });
        selection.read_with(cx, |m, _| {
            assert!(m.show_trajectory());
            assert_eq!(
                m.selected_object(),
                Some(&SelectedObject::SolarSystem(SolarSystemBody::Moon))
            );
        });
    }

    #[gpui::test]
    fn clearing_selection_does_not_close_details_panel_and_toggle_emits_toggle(
        cx: &mut TestAppContext,
    ) {
        let events: Rc<RefCell<Vec<PanelEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let events_clone = events.clone();

        let (selection, panel) = cx.update(|cx| {
            init(cx);
            let selection = cx.global::<GlobalStarSelection>().0.clone();
            let panel = StarDetailsPanel::build(cx);
            cx.subscribe(&panel, move |_panel, event: &PanelEvent, _cx| {
                events_clone.borrow_mut().push(*event);
            })
            .detach();
            (selection, panel)
        });

        // Selecting an object activates the panel.
        selection.update(cx, |model, cx| {
            model.select_object(SelectedObject::SolarSystem(SolarSystemBody::Sun), cx);
        });
        cx.run_until_parked();
        assert_eq!(*events.borrow(), vec![PanelEvent::Activate]);

        // Clearing selection (Escape) must NOT emit PanelEvent::Close.
        selection.update(cx, |model, cx| {
            model.clear(cx);
        });
        cx.run_until_parked();
        assert_eq!(*events.borrow(), vec![PanelEvent::Activate]);

        // Toggling emits PanelEvent::Toggle.
        panel.update(cx, |this, cx| {
            this.toggle_panel(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            vec![PanelEvent::Activate, PanelEvent::Toggle]
        );
    }
}
