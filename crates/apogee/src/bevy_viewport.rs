use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use celestial::{
    DailyTrajectory, HorizonProfile, TrajectoryPointKind, compute_equatorial_daily_trajectory,
    compute_solar_system_window_trajectory,
    coords::{Angle, geodetic::GeodeticPosition},
};
use components::{Slider, SliderEvent};
use futures::StreamExt;
use gpui::{
    App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, ScrollDelta, ScrollWheelEvent, Task, Window, div, point, prelude::*, px,
    rgb, svg,
};
use horizon::GlobalHorizons;
use observer::{GlobalObserver, ObserverClock, ObserverModel};
use planetarium::{
    BevyBridge, HorizonImageData, HorizonLayerData, ObserverClockSpec, PickedSolarSystemBody,
    PickedStar, PlanetariumRenderer, PlanetariumViewInfo, StarRenderParams, StarSpatialIndex,
    TrackingTarget, TrajectoryRenderData, ViewMode, pick_solar_system_body,
    rendered_solar_system_disc_radius_px,
};
use settings::{GlobalSettings, SettingsModel};
use status_bar::{
    CursorSkyCoords, GlobalPlanetariumStatus, PlanetariumTelemetry, TelemetryCoordMode,
};
use theme::ActiveTheme;
use ui::{StyledTypography, Tooltip, h_flex, v_flex};
use workspace::{
    WorkspaceKind,
    dock::DockPosition,
    panel::{Panel, PanelEvent},
    workbench::Workbench,
};

use crate::sky_search;
use crate::star_details::{
    GlobalStarSelection, SelectedObject, SelectedObjectId, TargetDayCacheKey, format_local_hour_hm,
    lerp_rgba, paint_disc,
};

const HOVER_PREVIEW_DELAY: Duration = Duration::from_millis(280);
const CLICK_DRAG_THRESHOLD_PX: f32 = 4.0;
const SOLAR_SYSTEM_TRAJ_HALF_WINDOW_HOURS: f64 = 6.0;
const SOLAR_SYSTEM_TRAJ_DURATION_HOURS: f64 = SOLAR_SYSTEM_TRAJ_HALF_WINDOW_HOURS * 2.0;
const TRAJ_EDGE_FADE_FRAC: f32 = 0.22;

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn trajectory_edge_fade(offset_hour: f64, duration_hours: f64, faded: bool) -> f32 {
    if !faded || duration_hours <= 0.0 {
        return 1.0;
    }
    let u = (offset_hour / duration_hours).clamp(0.0, 1.0) as f32;
    smoothstep(0.0, TRAJ_EDGE_FADE_FRAC, u) * smoothstep(0.0, TRAJ_EDGE_FADE_FRAC, 1.0 - u)
}

const ICON_COORDS_HORIZONTAL: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 18H21.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M4 18A8 8 0 0 1 20 18" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5.2 13.8H18.8" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <path d="M12 10V18" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <circle cx="12" cy="6.2" r="1.2" fill="black"/>
</svg>"##;

const ICON_COORDS_EQUATORIAL: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="7.5" stroke="black" stroke-width="1.5"/>
  <path d="M16.75 3.8L7.25 20.2" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5.5 8.25Q9.6 16.15 18.5 15.75" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <path d="M15.75 5.5Q17.55 15.2 8.25 18.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
</svg>"##;

const ICON_LAYER_ATMOSPHERE: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M3 16.5Q12 13.5 21 16.5" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M7.5 15.2A4.5 4.5 0 0 1 16.5 15.2" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M12 5.2V7.8M6.2 7.8L8 9.6M17.8 7.8L16 9.6" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M6.5 19.5H17.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
</svg>"##;

const ICON_LAYER_GROUND: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 10.5H21.5" stroke="black" stroke-width="1.6" stroke-linecap="round"/>
  <path d="M4 10.5A8 8 0 0 0 20 10.5" stroke="black" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5.6 14.8H18.4" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <path d="M12 10.5V18.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
</svg>"##;

const ICON_LAYER_HORIZON: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M2.5 17.5L7.5 10.5L11.8 15L16.2 8.5L21.5 17.5" stroke="black" stroke-width="1.55" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M2.5 19.5H21.5" stroke="black" stroke-width="1.35" stroke-linecap="round"/>
  <circle cx="8" cy="6.2" r="1.2" fill="black"/>
</svg>"##;

const ICON_LAYER_HIPS: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" stroke="black" stroke-width="1.5"/>
  <circle cx="12" cy="12" r="1.4" fill="black"/>
  <path d="M12 10.6C13.8 8.2 17 8.8 17.5 11.2M12 13.4C10.2 15.8 7 15.2 6.5 12.8" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
  <circle cx="7" cy="8" r="0.9" fill="black"/>
  <circle cx="17" cy="16.2" r="0.9" fill="black"/>
</svg>"##;

const ICON_LAYER_STARS: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M11 4.5L12.5 9.5L17.5 11L12.5 12.5L11 17.5L9.5 12.5L4.5 11L9.5 9.5L11 4.5Z" stroke="black" stroke-width="1.45" stroke-linejoin="round"/>
  <circle cx="18.2" cy="6.2" r="1.15" fill="black"/>
  <circle cx="17.5" cy="17.8" r="1.05" fill="black"/>
  <circle cx="6" cy="17.5" r="0.95" fill="black"/>
</svg>"##;

const ICON_LAYER_SOLAR_SYSTEM: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <circle cx="12" cy="12" r="4.2" stroke="black" stroke-width="1.5"/>
  <path d="M12 3.2V5.3M12 18.7V20.8M3.2 12H5.3M18.7 12H20.8M5.8 5.8L7.3 7.3M16.7 16.7L18.2 18.2M18.2 5.8L16.7 7.3M7.3 16.7L5.8 18.2" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

const ICON_GRID_ALTAZ: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M5 8.5H19M4 15.5H20" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M12 3.5V20.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M9.5 4.2Q6.2 12 9.5 19.8M14.5 4.2Q17.8 12 14.5 19.8" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

const ICON_GRID_EQUATORIAL: &[u8] = br##"<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M16 4.5L8 19.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M7.3 5.5L20 12.3M4 11.7L16.7 18.5" stroke="black" stroke-width="1.45" stroke-linecap="round"/>
  <path d="M12.3 3.3Q6.9 9.3 5 17.1M19 6.9Q17.1 14.7 11.7 20.7" stroke="black" stroke-width="1.4" stroke-linecap="round"/>
</svg>"##;

#[derive(Clone, Copy)]
struct LayerPillStyle {
    pill_bg: Hsla,
    pill_border: Hsla,
    active_bg: Hsla,
    hover_bg: Hsla,
    active_icon: Hsla,
    disabled_icon: Hsla,
    popover_ring: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickLayer {
    Atmosphere,
    Ground,
    Horizon,
    HipsSurvey,
    Stars,
    SolarSystem,
    AltAzGrid,
    EquatorialGrid,
}

impl QuickLayer {
    const SKY_LAYERS: [Self; 6] = [
        Self::Atmosphere,
        Self::Ground,
        Self::Horizon,
        Self::HipsSurvey,
        Self::Stars,
        Self::SolarSystem,
    ];
    const GRID_LAYERS: [Self; 2] = [Self::AltAzGrid, Self::EquatorialGrid];

    fn id(self) -> &'static str {
        match self {
            Self::Atmosphere => "quick_layer_atmosphere",
            Self::Ground => "quick_layer_ground",
            Self::Horizon => "quick_layer_horizon",
            Self::HipsSurvey => "quick_layer_hips",
            Self::Stars => "quick_layer_stars",
            Self::SolarSystem => "quick_layer_solar_system",
            Self::AltAzGrid => "quick_layer_altaz_grid",
            Self::EquatorialGrid => "quick_layer_eq_grid",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Atmosphere => "Sky Atmosphere",
            Self::Ground => "Ground Floor",
            Self::Horizon => "Custom Horizon",
            Self::HipsSurvey => "HiPS Survey",
            Self::Stars => "Stars",
            Self::SolarSystem => "Solar System",
            Self::AltAzGrid => "Alt/Az Grid",
            Self::EquatorialGrid => "Equatorial Grid",
        }
    }

    fn icon(self) -> &'static [u8] {
        match self {
            Self::Atmosphere => ICON_LAYER_ATMOSPHERE,
            Self::Ground => ICON_LAYER_GROUND,
            Self::Horizon => ICON_LAYER_HORIZON,
            Self::HipsSurvey => ICON_LAYER_HIPS,
            Self::Stars => ICON_LAYER_STARS,
            Self::SolarSystem => ICON_LAYER_SOLAR_SYSTEM,
            Self::AltAzGrid => ICON_GRID_ALTAZ,
            Self::EquatorialGrid => ICON_GRID_EQUATORIAL,
        }
    }

    fn active_icon_color(self, default: Hsla) -> Hsla {
        match self {
            Self::AltAzGrid => rgb(0x66b5f5).into(),
            Self::EquatorialGrid => rgb(0xeb9b56).into(),
            Self::SolarSystem => rgb(0xf7c957).into(),
            Self::Atmosphere
            | Self::Ground
            | Self::Horizon
            | Self::HipsSurvey
            | Self::Stars => default,
        }
    }

    fn is_enabled(self, model: &SettingsModel) -> bool {
        match self {
            Self::Atmosphere => model.atmosphere_enabled(),
            Self::Ground => model.ground_enabled(),
            Self::Horizon => model.horizon_enabled(),
            Self::HipsSurvey => model.hips_survey_enabled(),
            Self::Stars => model.stars_enabled(),
            Self::SolarSystem => model.solar_system_enabled(),
            Self::AltAzGrid => model.altaz_grid_enabled(),
            Self::EquatorialGrid => model.equatorial_grid_enabled(),
        }
    }

    fn stored_opacity(self, model: &SettingsModel) -> f32 {
        match self {
            Self::Atmosphere => model.atmosphere_strength(),
            Self::Ground => model.ground_opacity(),
            Self::Horizon => model.horizon_opacity(),
            Self::HipsSurvey => model.hips_survey_opacity(),
            Self::Stars => model.stars_opacity(),
            Self::SolarSystem => model.solar_system_opacity(),
            Self::AltAzGrid => model.altaz_grid_opacity(),
            Self::EquatorialGrid => model.equatorial_grid_opacity(),
        }
    }

    fn is_visible(self, model: &SettingsModel) -> bool {
        self.is_enabled(model) && self.stored_opacity(model) > 1e-3
    }

    fn set_enabled(self, model: &mut SettingsModel, enabled: bool, cx: &mut Context<SettingsModel>) {
        match self {
            Self::Atmosphere => model.set_atmosphere_enabled(enabled, cx),
            Self::Ground => model.set_ground_enabled(enabled, cx),
            Self::Horizon => model.set_horizon_enabled(enabled, cx),
            Self::HipsSurvey => model.set_hips_survey_enabled(enabled, cx),
            Self::Stars => model.set_stars_enabled(enabled, cx),
            Self::SolarSystem => model.set_solar_system_enabled(enabled, cx),
            Self::AltAzGrid => model.set_altaz_grid_enabled(enabled, cx),
            Self::EquatorialGrid => model.set_equatorial_grid_enabled(enabled, cx),
        }
    }

    fn toggle(self, model: &mut SettingsModel, cx: &mut Context<SettingsModel>) {
        if self.is_visible(model) {
            self.set_enabled(model, false, cx);
        } else {
            if self.stored_opacity(model) <= 1e-3 {
                self.commit_raw_opacity(model, 1.0, cx);
            }
            self.set_enabled(model, true, cx);
        }
    }

    fn preview_opacity(self, model: &mut SettingsModel, value: f32, cx: &mut Context<SettingsModel>) {
        if !self.is_enabled(model) && value > 1e-3 {
            self.set_enabled(model, true, cx);
        }
        match self {
            Self::Atmosphere => model.preview_atmosphere_strength(value, cx),
            Self::Ground => model.preview_ground_opacity(value, cx),
            Self::Horizon => model.preview_horizon_opacity(value, cx),
            Self::HipsSurvey => model.preview_hips_survey_opacity(value, cx),
            Self::Stars => model.preview_stars_opacity(value, cx),
            Self::SolarSystem => model.preview_solar_system_opacity(value, cx),
            Self::AltAzGrid => model.preview_altaz_grid_opacity(value, cx),
            Self::EquatorialGrid => model.preview_equatorial_grid_opacity(value, cx),
        }
    }

    fn commit_opacity(self, model: &mut SettingsModel, value: f32, cx: &mut Context<SettingsModel>) {
        if !self.is_enabled(model) && value > 1e-3 {
            self.set_enabled(model, true, cx);
        }
        self.commit_raw_opacity(model, value, cx);
    }

    fn commit_raw_opacity(
        self,
        model: &mut SettingsModel,
        value: f32,
        cx: &mut Context<SettingsModel>,
    ) {
        match self {
            Self::Atmosphere => model.commit_atmosphere_strength(value, cx),
            Self::Ground => model.commit_ground_opacity(value, cx),
            Self::Horizon => model.commit_horizon_opacity(value, cx),
            Self::HipsSurvey => model.commit_hips_survey_opacity(value, cx),
            Self::Stars => model.commit_stars_opacity(value, cx),
            Self::SolarSystem => model.commit_solar_system_opacity(value, cx),
            Self::AltAzGrid => model.commit_altaz_grid_opacity(value, cx),
            Self::EquatorialGrid => model.commit_equatorial_grid_opacity(value, cx),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum HoveredObject {
    Star(PickedStar),
    SolarSystem(PickedSolarSystemBody),
}

impl HoveredObject {
    fn id(&self) -> SelectedObjectId {
        match self {
            Self::Star(s) => SelectedObjectId::Star(s.catalog_index),
            Self::SolarSystem(b) => SelectedObjectId::SolarSystem(b.body),
        }
    }

    fn screen_ndc(&self) -> [f64; 2] {
        match self {
            Self::Star(s) => s.screen_ndc,
            Self::SolarSystem(b) => b.screen_ndc,
        }
    }

    fn rendered_radius_px(&self) -> f32 {
        match self {
            Self::Star(s) => s.rendered_radius_px,
            Self::SolarSystem(b) => b.rendered_radius_px,
        }
    }

    fn srgb(&self) -> [f32; 3] {
        match self {
            Self::Star(s) => s.srgb,
            Self::SolarSystem(b) => b.body.srgb(),
        }
    }

    fn title_and_badge(&self) -> (String, String) {
        match self {
            Self::Star(s) => (
                s.display_title_and_subtitle().0,
                format!("G {:.2}", s.mag),
            ),
            Self::SolarSystem(b) => match b.ephemeris.lunar_phase {
                Some(phase) => (
                    b.body.name().to_string(),
                    format!("{} ({:.0}%)", phase.phase_name(), phase.illuminated_fraction * 100.0),
                ),
                None => (
                    b.body.name().to_string(),
                    b.body.category_label().to_string(),
                ),
            },
        }
    }

    fn to_selected(&self) -> SelectedObject {
        match self {
            Self::Star(s) => SelectedObject::from_picked_star(s),
            Self::SolarSystem(b) => SelectedObject::from_picked_solar_system(b),
        }
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let viewport = PlanetariumPanel::build(window, cx, 1024, 1024);
            cx.observe(&viewport, |_this, _viewport, cx| {
                cx.notify();
            })
            .detach();
            workbench.add_panel(viewport, window, cx);
        }
    })
    .detach();
}

pub struct PlanetariumPanel {
    bridge: BevyBridge,
    cached_texture: Option<Arc<wgpu::TextureView>>,
    renderer_handle: Option<JoinHandle<()>>,
    focus_handle: FocusHandle,
    is_dragging: bool,
    did_drag: bool,
    drag_origin: Option<Point<Pixels>>,
    last_mouse_pos: Option<Point<Pixels>>,
    viewport_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    cursor_position: Option<Point<Pixels>>,
    latest_view_info: PlanetariumViewInfo,
    star_index: Option<Arc<StarSpatialIndex>>,
    hovered_object: Option<HoveredObject>,
    hover_preview_visible: bool,
    hover_preview_target_id: Option<SelectedObjectId>,
    hover_preview_task: Option<Task<()>>,
    last_synced_selection_nonce: Option<u64>,
    trajectory_cache_key: Option<TargetDayCacheKey>,
    cached_trajectory: Option<Arc<DailyTrajectory>>,
    trajectory_window_start_secs: f64,
    trajectory_start_local_hour: f64,
    trajectory_duration_hours: f64,
    trajectory_is_faded: bool,
    last_synced_trajectory: Option<(TargetDayCacheKey, bool)>,
    last_scale_factor: f64,
    active_opacity_layer: Option<QuickLayer>,
    opacity_slider: Entity<Slider>,
    last_synced_observer: Option<(GeodeticPosition, ObserverClockSpec)>,
    last_synced_hips_grid: Option<bool>,
    last_synced_atmosphere_strength: Option<f32>,
    last_synced_ground_opacity: Option<f32>,
    last_synced_hips_survey_enabled: Option<bool>,
    last_synced_hips_survey_opacity: Option<f32>,
    last_synced_stars_opacity: Option<f32>,
    last_synced_solar_system_opacity: Option<f32>,
    last_synced_altaz_grid_opacity: Option<f32>,
    last_synced_equatorial_grid_opacity: Option<f32>,
    last_synced_horizon_opacity: Option<f32>,
    last_synced_horizon_show_line: Option<bool>,
    last_synced_horizon_key: Option<(String, u64)>,
    last_requested_image_tag: Option<(String, u64)>,
    active_horizon_profile: Option<HorizonProfile>,
    horizon_image_load_gen: u64,
}

fn tracking_target_for_selected(selected: &SelectedObject) -> TrackingTarget {
    match selected {
        SelectedObject::Star(star) => TrackingTarget::equatorial_deg(star.ra_deg, star.dec_deg),
        SelectedObject::SolarSystem(body) => TrackingTarget::SolarSystem(*body),
    }
}

fn observer_spec(model: &ObserverModel) -> (GeodeticPosition, ObserverClockSpec) {
    let clock = match model.clock() {
        ObserverClock::Live => ObserverClockSpec::Live,
        ObserverClock::Manual { unix_seconds } => ObserverClockSpec::FixedUnixSeconds(unix_seconds),
    };
    (model.geodetic_position(), clock)
}

impl EventEmitter<PanelEvent> for PlanetariumPanel {}

impl PlanetariumPanel {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        initial_width: u32,
        initial_height: u32,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let _build_span = tracing::info_span!("planetarium_panel_build").entered();
            let (tx, mut rx) = futures::channel::mpsc::channel::<()>(1);
            let tx = std::sync::Mutex::new(tx);

            let notify_ui = {
                move || {
                    if let Ok(mut tx) = tx.lock() {
                        let _ = tx.try_send(());
                    }
                }
            };

            let bridge = BevyBridge::new(notify_ui);
            let observer_entity = cx.try_global::<GlobalObserver>().map(|g| g.0.clone());
            let initial_observer = observer_entity.as_ref().map(|entity| {
                let spec = observer_spec(entity.read(cx));
                bridge.set_observer(spec.0, spec.1);
                spec
            });

            if let Some(entity) = observer_entity {
                cx.observe(&entity, |this: &mut Self, model, cx| {
                    let next = observer_spec(model.read(cx));
                    if this.last_synced_observer != Some(next) {
                        this.last_synced_observer = Some(next);
                        this.bridge.set_observer(next.0, next.1);
                    }
                    this.sync_horizon_to_bridge(cx);
                })
                .detach();
            }

            let horizons_entity = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone());
            if let Some(entity) = horizons_entity {
                cx.observe(&entity, |this: &mut Self, _model, cx| {
                    this.sync_horizon_to_bridge(cx);
                    cx.notify();
                })
                .detach();
            }

            let settings_entity = cx.try_global::<GlobalSettings>().map(|g| g.0.clone());
            let (
                initial_hips_grid,
                initial_atmosphere_strength,
                initial_ground_opacity,
                initial_horizon_opacity,
                initial_horizon_show_line,
                initial_hips_survey_enabled,
                initial_hips_survey_opacity,
                initial_stars_opacity,
                initial_solar_system_opacity,
                initial_altaz_grid_opacity,
                initial_equatorial_grid_opacity,
            ) = if let Some(entity) = settings_entity.as_ref() {
                let model = entity.read(cx);
                let show_grid = model.show_hips_grid();
                let atmo = model.effective_atmosphere_strength();
                let ground = model.effective_ground_opacity();
                let horizon_opacity = model.effective_horizon_opacity();
                let horizon_show_line = model.horizon_show_line();
                let hips_enabled = model.hips_survey_enabled();
                let hips_opacity = model.hips_survey_opacity();
                let stars = model.effective_stars_opacity();
                let solar_system = model.effective_solar_system_opacity();
                let altaz_grid = model.effective_altaz_grid_opacity();
                let eq_grid = model.effective_equatorial_grid_opacity();
                bridge.set_hips_grid_visible(show_grid);
                bridge.set_atmosphere_strength(atmo);
                bridge.set_ground_opacity(ground);
                bridge.set_horizon_opacity(horizon_opacity);
                bridge.set_horizon_line_visible(horizon_show_line);
                bridge.set_hips_survey_visible(hips_enabled);
                bridge.set_hips_survey_opacity(hips_opacity);
                bridge.set_star_opacity(stars);
                bridge.set_solar_system_opacity(solar_system);
                bridge.set_altaz_grid_opacity(altaz_grid);
                bridge.set_equatorial_grid_opacity(eq_grid);
                (
                    Some(show_grid),
                    Some(atmo),
                    Some(ground),
                    Some(horizon_opacity),
                    Some(horizon_show_line),
                    Some(hips_enabled),
                    Some(hips_opacity),
                    Some(stars),
                    Some(solar_system),
                    Some(altaz_grid),
                    Some(eq_grid),
                )
            } else {
                (
                    None, None, None, None, None, None, None, None, None, None, None,
                )
            };

            let opacity_slider = cx.new(|_cx| Slider::new(1.0).default_value(1.0));
            cx.subscribe(
                &opacity_slider,
                |this: &mut Self, _slider, event: &SliderEvent, cx| {
                    let Some(layer) = this.active_opacity_layer else {
                        return;
                    };
                    let Some(settings_entity) =
                        cx.try_global::<GlobalSettings>().map(|g| g.0.clone())
                    else {
                        return;
                    };
                    match *event {
                        SliderEvent::Change(val) => {
                            settings_entity.update(cx, |model, cx| {
                                layer.preview_opacity(model, val, cx);
                            });
                        }
                        SliderEvent::Commit(val) => {
                            settings_entity.update(cx, |model, cx| {
                                layer.commit_opacity(model, val, cx);
                            });
                        }
                    }
                },
            )
            .detach();

            if let Some(entity) = settings_entity {
                cx.observe(&entity, |this: &mut Self, model, cx| {
                    let model_ref = model.read(cx);
                    let next_grid = model_ref.show_hips_grid();
                    if this.last_synced_hips_grid != Some(next_grid) {
                        this.last_synced_hips_grid = Some(next_grid);
                        this.bridge.set_hips_grid_visible(next_grid);
                    }
                    let next_atmo = model_ref.effective_atmosphere_strength();
                    if this
                        .last_synced_atmosphere_strength
                        .map_or(true, |prev| (prev - next_atmo).abs() > 1e-5)
                    {
                        this.last_synced_atmosphere_strength = Some(next_atmo);
                        this.bridge.set_atmosphere_strength(next_atmo);
                    }
                    let next_ground = model_ref.effective_ground_opacity();
                    if this
                        .last_synced_ground_opacity
                        .map_or(true, |prev| (prev - next_ground).abs() > 1e-5)
                    {
                        this.last_synced_ground_opacity = Some(next_ground);
                        this.bridge.set_ground_opacity(next_ground);
                    }
                    let next_horizon_opacity = model_ref.effective_horizon_opacity();
                    if this
                        .last_synced_horizon_opacity
                        .map_or(true, |prev| (prev - next_horizon_opacity).abs() > 1e-5)
                    {
                        this.last_synced_horizon_opacity = Some(next_horizon_opacity);
                        this.bridge.set_horizon_opacity(next_horizon_opacity);
                    }
                    let next_horizon_show_line = model_ref.horizon_show_line();
                    if this.last_synced_horizon_show_line != Some(next_horizon_show_line) {
                        this.last_synced_horizon_show_line = Some(next_horizon_show_line);
                        this.bridge.set_horizon_line_visible(next_horizon_show_line);
                    }
                    let next_hips_enabled = model_ref.hips_survey_enabled();
                    if this.last_synced_hips_survey_enabled != Some(next_hips_enabled) {
                        this.last_synced_hips_survey_enabled = Some(next_hips_enabled);
                        this.bridge.set_hips_survey_visible(next_hips_enabled);
                    }
                    let next_hips_opacity = model_ref.hips_survey_opacity();
                    if this
                        .last_synced_hips_survey_opacity
                        .map_or(true, |prev| (prev - next_hips_opacity).abs() > 1e-5)
                    {
                        this.last_synced_hips_survey_opacity = Some(next_hips_opacity);
                        this.bridge.set_hips_survey_opacity(next_hips_opacity);
                    }
                    let next_stars = model_ref.effective_stars_opacity();
                    if this
                        .last_synced_stars_opacity
                        .map_or(true, |prev| (prev - next_stars).abs() > 1e-5)
                    {
                        this.last_synced_stars_opacity = Some(next_stars);
                        this.bridge.set_star_opacity(next_stars);
                    }
                    let next_solar_system = model_ref.effective_solar_system_opacity();
                    if this
                        .last_synced_solar_system_opacity
                        .map_or(true, |prev| (prev - next_solar_system).abs() > 1e-5)
                    {
                        this.last_synced_solar_system_opacity = Some(next_solar_system);
                        this.bridge.set_solar_system_opacity(next_solar_system);
                    }
                    let next_altaz_grid = model_ref.effective_altaz_grid_opacity();
                    if this
                        .last_synced_altaz_grid_opacity
                        .map_or(true, |prev| (prev - next_altaz_grid).abs() > 1e-5)
                    {
                        this.last_synced_altaz_grid_opacity = Some(next_altaz_grid);
                        this.bridge.set_altaz_grid_opacity(next_altaz_grid);
                    }
                    let next_eq_grid = model_ref.effective_equatorial_grid_opacity();
                    if this
                        .last_synced_equatorial_grid_opacity
                        .map_or(true, |prev| (prev - next_eq_grid).abs() > 1e-5)
                    {
                        this.last_synced_equatorial_grid_opacity = Some(next_eq_grid);
                        this.bridge.set_equatorial_grid_opacity(next_eq_grid);
                    }
                    if let Some(layer) = this.active_opacity_layer {
                        let stored = layer.stored_opacity(model_ref);
                        this.opacity_slider.update(cx, |slider, cx| {
                            slider.set_value(stored, cx);
                        });
                    }
                    this.update_hovered_star(cx);
                    cx.notify();
                })
                .detach();
            }

            let device = (*window.wgpu_device().expect("wgpu device not available")).clone();
            let queue = (*window.wgpu_queue().expect("wgpu queue not available")).clone();
            let adapter = (*window.wgpu_adapter().expect("wgpu adapter not available")).clone();
            let instance = (*window.wgpu_instance().expect("wgpu instance not available")).clone();

            let renderer_handle = PlanetariumRenderer::spawn(
                device,
                queue,
                adapter,
                instance,
                initial_width,
                initial_height,
                bridge.clone(),
            )
            .expect("Failed to spawn PlanetariumRenderer thread");

            let initial_view_info = bridge.view_info();
            let initial_star_index = bridge.star_index();
            if let Some(index) = initial_star_index.as_ref() {
                sky_search::publish_star_index(index.clone(), cx);
            }
            let initial_scale_factor = f64::from(window.scale_factor());

            let initial_selection_nonce =
                if let Some(selection_entity) =
                    cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone())
                {
                    let (initial_target, initial_nonce) = {
                        let model = selection_entity.read(cx);
                        (
                            model.selected_object().map(tracking_target_for_selected),
                            model.selected_object().map(|_| model.activation_nonce()),
                        )
                    };
                    if let Some(target) = initial_target {
                        bridge.track_target(Some(target));
                    }
                    cx.observe(&selection_entity, |this: &mut Self, model, cx| {
                        let model_ref = model.read(cx);
                        match model_ref.selected_object() {
                            Some(selected) => {
                                let nonce = model_ref.activation_nonce();
                                if this.last_synced_selection_nonce != Some(nonce) {
                                    this.last_synced_selection_nonce = Some(nonce);
                                    this.bridge
                                        .track_target(Some(tracking_target_for_selected(selected)));
                                }
                            }
                            None => {
                                if this.last_synced_selection_nonce.take().is_some() {
                                    this.bridge.track_target(None);
                                }
                            }
                        }
                        cx.notify();
                    })
                    .detach();
                    initial_nonce
                } else {
                    None
                };

            cx.spawn(async move |this, cx| {
                while let Some(()) = rx.next().await {
                    if this
                        .update(cx, |this, cx| {
                            this.on_bevy_frame(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            let mut panel = Self {
                bridge,
                cached_texture: None,
                renderer_handle: Some(renderer_handle),
                focus_handle: cx.focus_handle(),
                is_dragging: false,
                did_drag: false,
                drag_origin: None,
                last_mouse_pos: None,
                viewport_bounds: Arc::new(Mutex::new(None)),
                cursor_position: None,
                latest_view_info: initial_view_info,
                star_index: initial_star_index,
                hovered_object: None,
                hover_preview_visible: false,
                hover_preview_target_id: None,
                hover_preview_task: None,
                last_synced_selection_nonce: initial_selection_nonce,
                trajectory_cache_key: None,
                cached_trajectory: None,
                trajectory_window_start_secs: 0.0,
                trajectory_start_local_hour: 0.0,
                trajectory_duration_hours: 24.0,
                trajectory_is_faded: false,
                last_synced_trajectory: None,
                last_scale_factor: initial_scale_factor,
                active_opacity_layer: None,
                opacity_slider,
                last_synced_observer: initial_observer,
                last_synced_hips_grid: initial_hips_grid,
                last_synced_atmosphere_strength: initial_atmosphere_strength,
                last_synced_ground_opacity: initial_ground_opacity,
                last_synced_hips_survey_enabled: initial_hips_survey_enabled,
                last_synced_hips_survey_opacity: initial_hips_survey_opacity,
                last_synced_stars_opacity: initial_stars_opacity,
                last_synced_solar_system_opacity: initial_solar_system_opacity,
                last_synced_altaz_grid_opacity: initial_altaz_grid_opacity,
                last_synced_equatorial_grid_opacity: initial_equatorial_grid_opacity,
                last_synced_horizon_opacity: initial_horizon_opacity,
                last_synced_horizon_show_line: initial_horizon_show_line,
                last_synced_horizon_key: None,
                last_requested_image_tag: None,
                active_horizon_profile: None,
                horizon_image_load_gen: 0,
            };
            panel.sync_horizon_to_bridge(cx);
            panel.sync_status_bar(cx);
            panel
        })
    }

    fn cursor_ndc(&self) -> Option<(f64, f64)> {
        let pos = self.cursor_position?;
        let bounds = self.viewport_bounds.lock().ok().and_then(|g| *g)?;
        let w = f64::from(f32::from(bounds.size.width));
        let h = f64::from(f32::from(bounds.size.height));
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        let u = f64::from(f32::from(pos.x - bounds.origin.x)) / w;
        let v = f64::from(f32::from(pos.y - bounds.origin.y)) / h;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        Some((2.0 * u - 1.0, 1.0 - 2.0 * v))
    }

    fn sync_status_bar(&self, cx: &mut Context<Self>) {
        let Some(status_entity) = cx
            .try_global::<GlobalPlanetariumStatus>()
            .map(|g| g.0.clone())
        else {
            return;
        };

        let info = self.latest_view_info;
        let mode = match info.view_mode {
            ViewMode::Horizontal => TelemetryCoordMode::Horizontal,
            ViewMode::Equatorial => TelemetryCoordMode::Equatorial,
        };
        let cursor = self.cursor_ndc().map(|(nx, ny)| {
            let c = info.coords_at_ndc(nx, ny);
            CursorSkyCoords {
                az_deg: c.az_deg,
                alt_deg: c.alt_deg,
                ra_deg: c.ra_deg,
                dec_deg: c.dec_deg,
            }
        });

        let telemetry = PlanetariumTelemetry {
            mode,
            fov_deg: info.fov_deg,
            cursor,
        };
        status_entity.update(cx, |model, cx| {
            model.update_telemetry(telemetry, cx);
        });
    }

    fn sync_horizon_to_bridge(&mut self, cx: &mut Context<Self>) {
        let active = cx
            .try_global::<GlobalObserver>()
            .and_then(|g| g.0.read(cx).active_horizon_id().map(str::to_string))
            .and_then(|horizon_id| {
                let horizons = cx.try_global::<GlobalHorizons>()?.0.read(cx);
                let bg = horizons.background(&horizon_id)?.clone();
                Some((horizons.live_generation(), bg))
            });

        let Some((live_gen, bg)) = active else {
            self.active_horizon_profile = None;
            self.last_requested_image_tag = None;
            self.horizon_image_load_gen = self.horizon_image_load_gen.wrapping_add(1);
            if self.last_synced_horizon_key.take().is_some() {
                self.bridge.set_horizon(None);
                self.update_hovered_star(cx);
            }
            return;
        };

        let next_key = (bg.id.clone(), live_gen);
        if self.last_synced_horizon_key.as_ref() == Some(&next_key) {
            return;
        }
        self.last_synced_horizon_key = Some(next_key);
        self.active_horizon_profile = Some(HorizonProfile::from_samples(
            bg.samples_deg.clone(),
            bg.azimuth_offset_deg,
        ));

        let image_tag = bg
            .image_asset
            .as_ref()
            .map(|asset| (format!("{}:{asset}", bg.id), bg.image_revision));

        self.bridge.set_horizon(Some(HorizonLayerData {
            id: bg.id.clone(),
            revision: bg.revision,
            samples_deg: bg.samples_deg.clone(),
            azimuth_offset_deg: bg.azimuth_offset_deg,
            image: None,
            image_tag: image_tag.clone(),
        }));

        if self.last_requested_image_tag != image_tag {
            self.last_requested_image_tag.clone_from(&image_tag);
            self.horizon_image_load_gen = self.horizon_image_load_gen.wrapping_add(1);
            let load_gen = self.horizon_image_load_gen;
            if let Some(expected_tag) = image_tag {
                let bytes = cx
                    .try_global::<GlobalHorizons>()
                    .and_then(|g| g.0.read(cx).load_image_bytes(&bg.id));
                if let Some(bytes) = bytes {
                    let horizon_id = bg.id.clone();
                    cx.spawn(async move |this, cx| {
                        let decoded = cx
                            .background_executor()
                            .spawn(async move { horizon::decode_rgba_for_planetarium(&bytes) })
                            .await;
                        let (width, height, rgba) = match decoded {
                            Ok(img) => img,
                            Err(err) => {
                                tracing::warn!("Failed to decode 360° horizon panorama: {err}");
                                return;
                            }
                        };
                        let _ = this.update(cx, |this, cx| {
                            if this.horizon_image_load_gen != load_gen
                                || this.last_requested_image_tag.as_ref() != Some(&expected_tag)
                            {
                                return;
                            }
                            let current_bg = cx.try_global::<GlobalHorizons>().and_then(|g| {
                                let model = g.0.read(cx);
                                model.background(&horizon_id).cloned()
                            });
                            let Some(current_bg) = current_bg else {
                                return;
                            };
                            let current_tag = current_bg
                                .image_asset
                                .as_ref()
                                .map(|asset| (format!("{}:{asset}", current_bg.id), current_bg.image_revision));
                            if current_tag.as_ref() != Some(&expected_tag) {
                                return;
                            }
                            this.bridge.set_horizon(Some(HorizonLayerData {
                                id: current_bg.id,
                                revision: current_bg.revision,
                                samples_deg: current_bg.samples_deg,
                                azimuth_offset_deg: current_bg.azimuth_offset_deg,
                                image: Some(Arc::new(HorizonImageData {
                                    width,
                                    height,
                                    rgba,
                                })),
                                image_tag: Some(expected_tag),
                            }));
                        });
                    })
                    .detach();
                }
            }
        }

        self.update_hovered_star(cx);
    }

    fn layer_visibility_flags(&self, cx: &App) -> (bool, bool, bool, bool) {
        cx.try_global::<GlobalSettings>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.effective_stars_opacity() > 1e-4,
                    m.effective_solar_system_opacity() > 1e-4,
                    m.effective_ground_opacity() >= 0.95,
                    m.effective_horizon_opacity() >= 0.95,
                )
            })
            .unwrap_or((true, true, true, true))
    }

    fn ensure_star_index(&mut self, cx: &mut App) -> Option<&Arc<StarSpatialIndex>> {
        if self.star_index.is_none() {
            if let Some(index) = self.bridge.star_index() {
                sky_search::publish_star_index(index.clone(), cx);
                self.star_index = Some(index);
            }
        }
        self.star_index.as_ref()
    }

    fn pick_object_at_cursor(&mut self, cx: &mut App) -> Option<HoveredObject> {
        let (ndc, bounds) = (
            self.cursor_ndc()?,
            self.viewport_bounds.lock().ok().and_then(|g| *g)?,
        );
        let (stars_visible, solar_system_visible, occlude_ground, occlude_custom_horizon) =
            self.layer_visibility_flags(cx);
        let horizon_occluder = if occlude_custom_horizon {
            self.active_horizon_profile.as_ref()
        } else {
            None
        };
        let w = f64::from(f32::from(bounds.size.width));
        let h = f64::from(f32::from(bounds.size.height));

        if solar_system_visible {
            if let Some(body) = pick_solar_system_body(
                &self.latest_view_info,
                ndc,
                (w, h),
                self.last_scale_factor,
                occlude_ground,
                horizon_occluder,
            ) {
                return Some(HoveredObject::SolarSystem(body));
            }
        }

        if !stars_visible {
            return None;
        }
        let view_info = self.latest_view_info;
        let scale_factor = self.last_scale_factor;
        let index = self.ensure_star_index(cx)?.clone();
        let star_horizon_occluder = if occlude_custom_horizon {
            self.active_horizon_profile.as_ref()
        } else {
            None
        };
        let atmo_strength = cx
            .try_global::<GlobalSettings>()
            .map_or(1.0, |g| g.0.read(cx).effective_atmosphere_strength());
        let sun_alt_deg = celestial::solar::solar_altitude_deg(
            view_info.observer.location,
            view_info.observer.epoch,
        ) as f32;
        let star_params = StarRenderParams::for_sky(
            sun_alt_deg,
            view_info.projection.fov_y.radians() as f32,
            atmo_strength,
        );
        index
            .pick(
                &view_info,
                ndc,
                (w, h),
                scale_factor,
                &star_params,
                occlude_ground,
                star_horizon_occluder,
            )
            .map(HoveredObject::Star)
    }

    fn compute_hovered_star(&mut self, cx: &mut Context<Self>) -> bool {
        let next = if self.is_dragging {
            None
        } else {
            self.pick_object_at_cursor(cx)
        };
        let next_id = next.as_ref().map(HoveredObject::id);
        let was_visible = self.hover_preview_visible;

        if next_id != self.hover_preview_target_id {
            self.hover_preview_task = None;
            self.hover_preview_visible = false;
            self.hover_preview_target_id = next_id;
            if let Some(target_id) = next_id {
                self.hover_preview_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_PREVIEW_DELAY).await;
                    let _ = this.update(cx, |this, cx| {
                        if !this.is_dragging
                            && this.hovered_object.as_ref().map(HoveredObject::id)
                                == Some(target_id)
                        {
                            this.hover_preview_visible = true;
                            cx.notify();
                        }
                    });
                }));
            }
        }

        let moved_on_screen = match (self.hovered_object.as_ref(), next.as_ref()) {
            (Some(prev), Some(curr)) if prev.id() == curr.id() => {
                let bounds_size = self
                    .viewport_bounds
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .map(|b| (f64::from(f32::from(b.size.width)), f64::from(f32::from(b.size.height))));
                bounds_size.is_some_and(|(w, h)| {
                    let prev_ndc = prev.screen_ndc();
                    let curr_ndc = curr.screen_ndc();
                    let dx = (curr_ndc[0] - prev_ndc[0]) * 0.5 * w;
                    let dy = (curr_ndc[1] - prev_ndc[1]) * 0.5 * h;
                    (dx * dx + dy * dy) > 0.25
                })
            }
            _ => false,
        };

        self.hovered_object = next;
        (was_visible != self.hover_preview_visible)
            || (self.hover_preview_visible && moved_on_screen)
    }

    fn update_hovered_star(&mut self, cx: &mut Context<Self>) {
        if self.compute_hovered_star(cx) {
            cx.notify();
        }
    }

    fn clear_hovered_star(&mut self, cx: &mut Context<Self>) {
        let was_visible = self.hover_preview_visible;
        self.hover_preview_task = None;
        self.hover_preview_target_id = None;
        self.hover_preview_visible = false;
        self.hovered_object = None;
        if was_visible {
            cx.notify();
        }
    }

    fn ensure_trajectory_fresh(
        &mut self,
        selected: Option<&SelectedObject>,
        cx: &App,
    ) -> Option<Arc<DailyTrajectory>> {
        let Some(selected) = selected else {
            self.trajectory_cache_key = None;
            self.cached_trajectory = None;
            return None;
        };

        let (loc, midnight, epoch_secs) = cx
            .try_global::<GlobalObserver>()
            .map(|g| {
                let m = g.0.read(cx);
                (
                    m.geodetic_position(),
                    m.local_midnight_unix_seconds(),
                    m.observation_context().epoch_unix_seconds(),
                )
            })
            .unwrap_or_else(|| {
                let obs = self.latest_view_info.observer;
                let secs = obs.epoch_unix_seconds();
                let midnight = (secs / 86_400.0).floor() * 86_400.0;
                (obs.location, midnight, secs)
            });

        let (window_start_secs, duration_hours, is_faded, cache_time_bucket) = match selected {
            SelectedObject::Star(_) => (midnight, 24.0, false, midnight.round() as i64),
            SelectedObject::SolarSystem(_) => {
                let start = epoch_secs - SOLAR_SYSTEM_TRAJ_HALF_WINDOW_HOURS * 3600.0;
                // Quantize to 1-minute steps so the [-6h..+6h] arc tracks smoothly without per-frame rebuilds.
                let bucket = (start / 60.0).round() as i64;
                (
                    bucket as f64 * 60.0,
                    SOLAR_SYSTEM_TRAJ_DURATION_HOURS,
                    true,
                    bucket,
                )
            }
        };

        let next_key = TargetDayCacheKey {
            target_id: selected.id(),
            lat_bits: loc.latitude_deg().to_bits(),
            lon_bits: loc.longitude_deg().to_bits(),
            elev_bits: loc.elevation_m().to_bits(),
            local_midnight_secs: cache_time_bucket,
            horizon_key: self.last_synced_horizon_key.clone(),
        };

        if self.trajectory_cache_key.as_ref() != Some(&next_key) {
            let traj = match selected {
                SelectedObject::Star(star) => compute_equatorial_daily_trajectory(
                    loc,
                    window_start_secs,
                    Angle::from_degrees(star.ra_deg),
                    Angle::from_degrees(star.dec_deg),
                    self.active_horizon_profile.as_ref(),
                    145,
                ),
                SelectedObject::SolarSystem(body) => compute_solar_system_window_trajectory(
                    loc,
                    window_start_secs,
                    duration_hours,
                    *body,
                    self.active_horizon_profile.as_ref(),
                    121,
                ),
            };
            self.cached_trajectory = Some(Arc::new(traj));
            self.trajectory_cache_key = Some(next_key);
            self.trajectory_window_start_secs = window_start_secs;
            self.trajectory_start_local_hour =
                ((window_start_secs - midnight) / 3600.0).rem_euclid(24.0);
            self.trajectory_duration_hours = duration_hours;
            self.trajectory_is_faded = is_faded;
        }

        self.cached_trajectory.clone()
    }

    fn jump_to_trajectory_offset_hour(&self, offset_hour: f64, cx: &mut Context<Self>) {
        let target_unix_secs = self.trajectory_window_start_secs + offset_hour * 3600.0;
        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            observer.update(cx, |model, cx| {
                model.set_manual_unix_seconds(target_unix_secs, cx);
            });
        }
    }

    fn on_bevy_frame(&mut self, cx: &mut Context<Self>) {
        let _ = self.ensure_star_index(cx);
        self.latest_view_info = self.bridge.view_info();
        self.sync_status_bar(cx);
        let _ = self.compute_hovered_star(cx);
        cx.notify();
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.is_dragging = true;
        self.did_drag = false;
        self.drag_origin = Some(event.position);
        self.cursor_position = Some(event.position);
        self.last_mouse_pos = Some(event.position);
        self.clear_hovered_star(cx);
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_position = Some(event.position);
        self.last_scale_factor = f64::from(window.scale_factor());

        if !self.is_dragging {
            self.sync_status_bar(cx);
            self.update_hovered_star(cx);
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.handle_mouse_up(false, cx);
            return;
        }

        if !self.did_drag {
            let moved_px = self.drag_origin.map_or(0.0, |origin| {
                let dx = f32::from(event.position.x - origin.x);
                let dy = f32::from(event.position.y - origin.y);
                (dx * dx + dy * dy).sqrt()
            });
            if moved_px <= CLICK_DRAG_THRESHOLD_PX {
                return;
            }
            self.did_drag = true;
        }

        let last_pos = self.last_mouse_pos.unwrap_or(event.position);
        let scale = f64::from(window.scale_factor());
        let dx = f32::from(event.position.x - last_pos.x) as f64 * scale;
        let dy = f32::from(event.position.y - last_pos.y) as f64 * scale;

        // dx > 0 when dragging right
        // dy > 0 when dragging down
        self.bridge.pan(dx, dy);
        self.last_mouse_pos = Some(event.position);
    }

    fn handle_mouse_up(&mut self, from_outside: bool, cx: &mut Context<Self>) {
        if self.is_dragging {
            let is_click = !from_outside && !self.did_drag;
            self.is_dragging = false;
            self.did_drag = false;
            self.drag_origin = None;
            self.last_mouse_pos = None;
            if is_click {
                if let Some(picked) = self.pick_object_at_cursor(cx) {
                    self.clear_hovered_star(cx);
                    if let Some(selection) =
                        cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone())
                    {
                        selection.update(cx, |model, cx| {
                            model.select_object(picked.to_selected(), cx);
                        });
                    }
                    cx.notify();
                    return;
                }
            }
            let _ = self.compute_hovered_star(cx);
            cx.notify();
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.cursor_position = Some(event.position);
        let delta_lines = match event.delta {
            ScrollDelta::Lines(pt) => pt.y as f64,
            ScrollDelta::Pixels(pt) => (f32::from(pt.y) / 20.0) as f64,
        };

        if delta_lines.abs() > 1e-4 {
            if let Some(ndc) = self.cursor_ndc() {
                self.bridge.zoom_at(delta_lines, ndc);
            } else {
                self.bridge.zoom(delta_lines);
            }
            self.clear_hovered_star(cx);
        }
    }

    fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.latest_view_info.view_mode != mode {
            self.bridge.set_view_mode(mode);
            self.latest_view_info.view_mode = mode;
            self.sync_status_bar(cx);
            cx.notify();
        }
    }

    fn toggle_quick_layer(&mut self, layer: QuickLayer, cx: &mut Context<Self>) {
        let Some(settings_entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) else {
            return;
        };
        settings_entity.update(cx, |model, cx| {
            layer.toggle(model, cx);
        });
    }

    fn open_opacity_popover(&mut self, layer: QuickLayer, cx: &mut Context<Self>) {
        if self.active_opacity_layer == Some(layer) {
            self.active_opacity_layer = None;
            cx.notify();
            return;
        }
        let stored_opacity = cx
            .try_global::<GlobalSettings>()
            .map_or(1.0, |g| layer.stored_opacity(g.0.read(cx)));
        self.active_opacity_layer = Some(layer);
        self.opacity_slider.update(cx, |slider, cx| {
            slider.set_value(stored_opacity, cx);
        });
        cx.notify();
    }

    fn render_layer_pill(
        &self,
        pill_id: &'static str,
        layers: &[QuickLayer],
        style: LayerPillStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let states: Vec<(QuickLayer, bool, f32)> = {
            let model = cx.try_global::<GlobalSettings>().map(|g| g.0.read(cx));
            layers
                .iter()
                .copied()
                .map(|layer| {
                    (
                        layer,
                        model.is_none_or(|m| layer.is_visible(m)),
                        model.map_or(1.0, |m| layer.stored_opacity(m)),
                    )
                })
                .collect()
        };
        let active_popover = self.active_opacity_layer;

        h_flex()
            .id(pill_id)
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded_full()
            .bg(style.pill_bg)
            .border_1()
            .border_color(style.pill_border)
            .shadow_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if *hovered {
                    this.cursor_position = None;
                    this.sync_status_bar(cx);
                    this.clear_hovered_star(cx);
                }
            }))
            .children(states.into_iter().map(|(layer, is_visible, stored)| {
                let is_popover_open = active_popover == Some(layer);
                let status_label = if is_visible {
                    format!("{:.0}%", stored * 100.0)
                } else {
                    "Off".to_string()
                };
                let tip = format!(
                    "{} · {} (Right-click for opacity)",
                    layer.title(),
                    status_label
                );

                div()
                    .id(layer.id())
                    .w(px(26.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(is_visible, |el| el.bg(style.active_bg))
                    .when(!is_visible, |el| el.hover(|s| s.bg(style.hover_bg)))
                    .when(is_popover_open, |el| {
                        el.border_1().border_color(style.popover_ring)
                    })
                    .tooltip(Tooltip::text(tip))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_quick_layer(layer, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.open_opacity_popover(layer, cx);
                        }),
                    )
                    .child(
                        svg()
                            .data(layer.icon())
                            .size(px(14.0))
                            .flex_none()
                            .text_color(if is_visible {
                                layer.active_icon_color(style.active_icon)
                            } else {
                                style.disabled_icon
                            }),
                    )
            }))
    }
}

impl Render for PlanetariumPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.bridge.snapshot();
        self.cached_texture = snapshot.texture_view;
        self.latest_view_info = snapshot.view_info;
        self.last_scale_factor = f64::from(window.scale_factor());
        let bridge = self.bridge.clone();
        let last_size = snapshot.last_size;
        let texture_size = snapshot.texture_size;
        let view_mode = self.latest_view_info.view_mode;
        let viewport_bounds = self.viewport_bounds.clone();
        let bounds_snapshot = viewport_bounds.lock().ok().and_then(|g| *g);
        let scale_factor = self.last_scale_factor.max(0.25);
        // Project overlays against the rendered texture's logical size (which matches
        // `snapshot.view_info` and `gpui::surface`), falling back to `bounds_snapshot`
        // only before the first texture arrives. This avoids 1-frame jumps on resize
        // (e.g. opening/closing the details panel).
        let proj_size = texture_size
            .map(|(tw, th)| (f64::from(tw) / scale_factor, f64::from(th) / scale_factor))
            .or_else(|| {
                bounds_snapshot.map(|b| {
                    (
                        f64::from(f32::from(b.size.width)),
                        f64::from(f32::from(b.size.height)),
                    )
                })
            });

        let colors = cx.theme().colors();
        let pill_bg = colors.elevated_surface_background.opacity(0.65);
        let pill_border = colors.border_variant.opacity(0.75);
        let active_bg = colors.element_selected.opacity(0.9);
        let hover_bg = colors.ghost_element_hover.opacity(0.6);
        let active_icon = colors.text;
        let muted_icon = colors.text_muted.opacity(0.75);
        let disabled_layer_icon = colors.text_muted.opacity(0.45);
        let popover_ring_border = colors.text_accent.opacity(0.7);
        let popover_bg = colors.elevated_surface_background.opacity(0.92);
        let popover_border = colors.border;
        let text_color = colors.text;
        let muted_text = colors.text_muted;

        let selected_object = cx
            .try_global::<GlobalStarSelection>()
            .and_then(|g| g.0.read(cx).selected_object().cloned());
        let selected_target_id = selected_object.as_ref().map(SelectedObject::id);

        let (stars_visible, solar_system_visible, occlude_ground, occlude_custom_horizon) =
            self.layer_visibility_flags(cx);
        let selected_layer_visible = match selected_object.as_ref() {
            Some(SelectedObject::Star(_)) => stars_visible,
            Some(SelectedObject::SolarSystem(_)) => solar_system_visible,
            None => false,
        };

        let trajectory_snapshot = if selected_layer_visible {
            let traj = self.ensure_trajectory_fresh(selected_object.as_ref(), cx);
            if let (Some(key), Some(traj_arc), Some(selected)) = (
                self.trajectory_cache_key.as_ref(),
                traj.as_ref(),
                selected_object.as_ref(),
            ) {
                let sync_key = (key.clone(), occlude_ground);
                if self.last_synced_trajectory.as_ref() != Some(&sync_key) {
                    self.last_synced_trajectory = Some(sync_key);
                    let duration_hours = self.trajectory_duration_hours;
                    let is_faded = self.trajectory_is_faded;
                    self.bridge.set_trajectory(Some(TrajectoryRenderData {
                        points: traj_arc
                            .samples
                            .iter()
                            .map(|s| {
                                let fade = trajectory_edge_fade(s.hour, duration_hours, is_faded);
                                (
                                    [s.topo_enu.x as f32, s.topo_enu.y as f32, s.topo_enu.z as f32],
                                    s.above_horizon,
                                    fade,
                                )
                            })
                            .collect(),
                        srgb: selected.srgb(),
                        occlude_ground,
                    }));
                }
            }
            traj
        } else {
            if self.last_synced_trajectory.take().is_some() {
                self.bridge.set_trajectory(None);
            }
            None
        };

        let selected_overlay = selected_object
            .as_ref()
            .filter(|_| selected_layer_visible)
            .and_then(|selected| {
                let (bw, bh) = proj_size?;
                if bw <= 1.0 || bh <= 1.0 {
                    return None;
                }
                let obs_ctx = self.latest_view_info.observer;
                let fov_y_rad = self.latest_view_info.projection.fov_y.radians() as f32;
                let (dir, rendered_px) = match selected {
                    SelectedObject::Star(star) => (
                        star.icrs_dir(),
                        StarRenderParams::default().rendered_radius_px(star.mag, fov_y_rad),
                    ),
                    SelectedObject::SolarSystem(body) => {
                        let eph = body.ephemeris(obs_ctx.location, obs_ctx.epoch);
                        let px = rendered_solar_system_disc_radius_px(
                            *body,
                            eph.angular_radius_deg,
                            fov_y_rad,
                            (bh * scale_factor) as f32,
                        );
                        (eph.icrs_dir, px)
                    }
                };
                if occlude_ground
                    || (occlude_custom_horizon && self.active_horizon_profile.is_some())
                {
                    let (az_deg, alt_deg) = self.latest_view_info.icrs_to_topocentric_deg(dir);
                    if occlude_ground && alt_deg < 0.0 {
                        return None;
                    }
                    if occlude_custom_horizon {
                        if let Some(horizon) = self.active_horizon_profile.as_ref() {
                            if !horizon.is_visible(az_deg, alt_deg) {
                                return None;
                            }
                        }
                    }
                }
                let (nx, ny) = self.latest_view_info.project_icrs_to_ndc(dir)?;
                if nx.abs() > 1.04 || ny.abs() > 1.04 {
                    return None;
                }
                let sx = (nx * 0.5 + 0.5) * bw;
                let sy = (0.5 - ny * 0.5) * bh;
                let inner_d =
                    ((f64::from(rendered_px) / scale_factor) * 2.0 + 10.0).clamp(16.0, 96.0);
                let outer_d = inner_d + 6.0;
                Some((selected.srgb(), sx, sy, inner_d, outer_d))
            });

        let trajectory_overlay = selected_object
            .as_ref()
            .zip(trajectory_snapshot)
            .and_then(|(selected, trajectory)| {
                let (bw, bh) = proj_size?;
                if bw <= 2.0 || bh <= 2.0 {
                    return None;
                }
                let srgb = selected.srgb();
                let view_info = self.latest_view_info;
                let is_circumpolar = trajectory.visibility.circumpolar;

                let mut event_badges = Vec::with_capacity(trajectory.events.len());
                for event in &trajectory.events {
                    let show_event = match event.kind {
                        TrajectoryPointKind::Rise
                        | TrajectoryPointKind::Transit
                        | TrajectoryPointKind::Set => {
                            event.above_horizon || !occlude_ground
                        }
                        TrajectoryPointKind::LowerTransit => {
                            (is_circumpolar && event.above_horizon) || !occlude_ground
                        }
                    };
                    if !show_event {
                        continue;
                    }
                    let Some((nx, ny)) = view_info.project_topo_to_ndc(event.topo_enu) else {
                        continue;
                    };
                    if nx.abs() > 0.98 || ny.abs() > 0.98 {
                        continue;
                    }
                    let sx = (nx * 0.5 + 0.5) * bw;
                    let sy = (0.5 - ny * 0.5) * bh;
                    event_badges.push((*event, sx, sy));
                }

                Some((
                    srgb,
                    trajectory,
                    view_info,
                    (bw, bh),
                    occlude_ground,
                    event_badges,
                    self.trajectory_start_local_hour,
                    self.trajectory_duration_hours,
                    self.trajectory_is_faded,
                ))
            });

        let hovered_overlay = self
            .hovered_object
            .as_ref()
            .filter(|_| self.hover_preview_visible)
            .and_then(|hovered| {
                let (bw, bh) = proj_size?;
                if bw <= 1.0 || bh <= 1.0 {
                    return None;
                }
                let screen_ndc = hovered.screen_ndc();
                let sx = (screen_ndc[0] * 0.5 + 0.5) * bw;
                let sy = (0.5 - screen_ndc[1] * 0.5) * bh;
                let reticle_d = ((f64::from(hovered.rendered_radius_px()) / scale_factor) * 2.0
                    + 10.0)
                    .clamp(16.0, 96.0);
                let reticle_x = sx - reticle_d * 0.5;
                let reticle_y = sy - reticle_d * 0.5;

                let (title, badge_label) = hovered.title_and_badge();
                let est_w = 220.0_f64;
                let pill_h = 28.0_f64;
                let margin = 12.0_f64;
                let gap = reticle_d * 0.5 + 8.0;

                let anchor_right = sx + gap + est_w + margin > bw;
                let anchor_x = if anchor_right {
                    (bw - (sx - gap)).clamp(margin, (bw - margin).max(margin))
                } else {
                    (sx + gap).clamp(margin, (bw - margin).max(margin))
                };

                let mut pill_y = sy - pill_h * 0.5;
                if sx + gap > bw - 310.0 && pill_y < 52.0 {
                    pill_y = 52.0;
                }
                let pill_y = pill_y.clamp(margin, (bh - pill_h - margin).max(margin));

                Some((
                    hovered.id(),
                    hovered.srgb(),
                    badge_label,
                    title,
                    reticle_x,
                    reticle_y,
                    reticle_d,
                    anchor_right,
                    anchor_x,
                    pill_y,
                ))
            });

        let pill_style = LayerPillStyle {
            pill_bg,
            pill_border,
            active_bg,
            hover_bg,
            active_icon,
            disabled_icon: disabled_layer_icon,
            popover_ring: popover_ring_border,
        };

        let active_popover_state = self.active_opacity_layer.map(|layer| {
            let model = cx.try_global::<GlobalSettings>().map(|g| g.0.read(cx));
            (
                layer,
                model.is_none_or(|m| layer.is_enabled(m)),
                model.map_or(100.0, |m| layer.stored_opacity(m) * 100.0),
            )
        });

        let sky_pill =
            self.render_layer_pill("layer_quick_toggles", &QuickLayer::SKY_LAYERS, pill_style, cx);
        let grid_pill = self.render_layer_pill(
            "grid_quick_toggles",
            &QuickLayer::GRID_LAYERS,
            pill_style,
            cx,
        );

        let coord_options = [
            (
                ViewMode::Horizontal,
                "coord_mode_horizontal",
                ICON_COORDS_HORIZONTAL,
                "Horizontal (Alt/Az)",
            ),
            (
                ViewMode::Equatorial,
                "coord_mode_equatorial",
                ICON_COORDS_EQUATORIAL,
                "Equatorial (RA/Dec)",
            ),
        ];

        div()
            .id("planetarium_viewport")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .relative()
            .cursor(CursorStyle::Arrow)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" && this.active_opacity_layer.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !hovered {
                    this.cursor_position = None;
                    this.sync_status_bar(cx);
                    this.clear_hovered_star(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.handle_mouse_down(event, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.handle_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(false, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(true, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.handle_scroll_wheel(event, cx);
            }))
            .child(
                gpui::canvas(
                    move |bounds, window, _cx| {
                        if let Ok(mut guard) = viewport_bounds.lock() {
                            *guard = Some(bounds);
                        }
                        let scale = window.scale_factor();
                        let width = (f32::from(bounds.size.width) * scale).round() as u32;
                        let height = (f32::from(bounds.size.height) * scale).round() as u32;
                        if width > 0 && height > 0 && last_size != Some((width, height)) {
                            bridge.resize(width, height);
                        }
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .size_full()
                .absolute()
                .top_0()
                .left_0(),
            )
            .children(
                self.cached_texture
                    .as_ref()
                    .map(|view| {
                        // Draw at the texture's native size (top-left anchored) so a
                        // resize snaps instead of stretching the stale frame.
                        let surface = gpui::surface(view.clone());
                        match texture_size {
                            Some((tw, th)) => {
                                let sc = scale_factor as f32;
                                surface
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .w(px(tw as f32 / sc))
                                    .h(px(th as f32 / sc))
                            }
                            None => surface.size_full(),
                        }
                    }),
            )
            .when_some(
                trajectory_overlay,
                |el,
                 (
                     srgb,
                     trajectory,
                     view_info,
                     proj_dims,
                     occlude_ground,
                     event_badges,
                     start_local_hour,
                     duration_hours,
                     is_faded,
                 )| {
                    let obj_rgb = Rgba {
                        r: srgb[0],
                        g: srgb[1],
                        b: srgb[2],
                        a: 1.0,
                    };
                    let obj_color: Hsla = obj_rgb.into();
                    let badges_for_canvas = event_badges.clone();
                    let (bw, bh) = proj_dims;

                    el.child(
                        gpui::canvas(
                            |_bounds, _window, _cx| {},
                            move |bounds, (), window, _cx| {
                                let proj_bounds = Bounds::new(
                                    bounds.origin,
                                    gpui::size(px(bw as f32), px(bh as f32)),
                                );
                                paint_planetarium_trajectory(
                                    proj_bounds,
                                    &trajectory,
                                    &view_info,
                                    &badges_for_canvas,
                                    obj_rgb,
                                    occlude_ground,
                                    start_local_hour,
                                    duration_hours,
                                    is_faded,
                                    window,
                                );
                            },
                        )
                        .size_full()
                        .absolute()
                        .top_0()
                        .left_0(),
                    )
                    .children(event_badges.into_iter().enumerate().map(
                        |(idx, (event, sx, sy))| {
                            let display_hour = (start_local_hour + event.hour).rem_euclid(24.0);
                            let (icon_glyph, kind_label, detail_text) = match event.kind {
                                TrajectoryPointKind::Transit => (
                                    "▲",
                                    "Transit",
                                    format!(
                                        "{} ({:+.0}°)",
                                        format_local_hour_hm(display_hour),
                                        event.alt_deg
                                    ),
                                ),
                                TrajectoryPointKind::Rise => (
                                    "↗",
                                    "Rise",
                                    format_local_hour_hm(display_hour),
                                ),
                                TrajectoryPointKind::Set => (
                                    "↘",
                                    "Set",
                                    format_local_hour_hm(display_hour),
                                ),
                                TrajectoryPointKind::LowerTransit => (
                                    "▼",
                                    "Low",
                                    format!(
                                        "{} ({:+.0}°)",
                                        format_local_hour_hm(display_hour),
                                        event.alt_deg
                                    ),
                                ),
                            };

                            let pill_y = match event.kind {
                                TrajectoryPointKind::Transit => (sy - 28.0).max(10.0),
                                TrajectoryPointKind::LowerTransit => (sy + 10.0).min(bh - 30.0),
                                TrajectoryPointKind::Rise | TrajectoryPointKind::Set => {
                                    (sy - 24.0).clamp(10.0, (bh - 30.0).max(10.0))
                                }
                            };
                            let pill_x = (sx + 10.0).clamp(10.0, (bw - 148.0).max(10.0));
                            let jump_offset_hour = event.hour;
                            let tip_text = format!(
                                "{} at {} (Az {:.0}°, Alt {:+.1}°) — Click to jump",
                                event.kind.label(),
                                format_local_hour_hm(display_hour),
                                event.az_deg,
                                event.alt_deg
                            );

                            h_flex()
                                .id(("trajectory_event_badge", idx))
                                .absolute()
                                .left(px(pill_x as f32))
                                .top(px(pill_y as f32))
                                .items_center()
                                .gap_1()
                                .px_1p5()
                                .py_0p5()
                                .rounded_md()
                                .bg(popover_bg)
                                .border_1()
                                .border_color(obj_color.opacity(0.55))
                                .shadow_md()
                                .cursor_pointer()
                                .hover(|s| s.border_color(obj_color.opacity(0.95)))
                                .tooltip(Tooltip::text(tip_text))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.jump_to_trajectory_offset_hour(jump_offset_hour, cx);
                                }))
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(obj_color)
                                        .child(icon_glyph),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_color)
                                        .child(kind_label),
                                )
                                .child(
                                    div()
                                        .font_buffer(cx)
                                        .text_size(px(10.0))
                                        .text_color(obj_color)
                                        .child(detail_text),
                                )
                        },
                    ))
                },
            )
            .when_some(
                selected_overlay,
                |el, (srgb, sx, sy, inner_d, outer_d)| {
                    let star_color: Hsla = Rgba {
                        r: srgb[0],
                        g: srgb[1],
                        b: srgb[2],
                        a: 1.0,
                    }
                    .into();
                    el.child(
                        div()
                            .absolute()
                            .left(px((sx - outer_d * 0.5) as f32))
                            .top(px((sy - outer_d * 0.5) as f32))
                            .w(px(outer_d as f32))
                            .h(px(outer_d as f32))
                            .rounded_full()
                            .border_1()
                            .border_color(star_color.opacity(0.92)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px((sx - inner_d * 0.5) as f32))
                            .top(px((sy - inner_d * 0.5) as f32))
                            .w(px(inner_d as f32))
                            .h(px(inner_d as f32))
                            .rounded_full()
                            .border_1()
                            .border_color(star_color.opacity(0.45)),
                    )
                },
            )
            .when_some(
                hovered_overlay,
                |el,
                 (
                     hovered_id,
                     srgb,
                     badge_label,
                     title,
                     reticle_x,
                     reticle_y,
                     reticle_d,
                     anchor_right,
                     anchor_x,
                     pill_y,
                 )| {
                    let star_color: Hsla = Rgba {
                        r: srgb[0],
                        g: srgb[1],
                        b: srgb[2],
                        a: 1.0,
                    }
                    .into();
                    let show_hover_reticle = selected_target_id != Some(hovered_id);

                    let preview = h_flex()
                        .absolute()
                        .top(px(pill_y as f32))
                        .when(anchor_right, |el| el.right(px(anchor_x as f32)))
                        .when(!anchor_right, |el| el.left(px(anchor_x as f32)))
                        .max_w(px(240.0))
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(popover_bg)
                        .border_1()
                        .border_color(popover_border)
                        .shadow_lg()
                        .child(
                            div()
                                .size(px(7.0))
                                .rounded_full()
                                .flex_none()
                                .bg(star_color),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_color)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(muted_text)
                                .child("·"),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_buffer(cx)
                                .text_size(px(11.0))
                                .text_color(star_color)
                                .child(badge_label),
                        );

                    el.when(show_hover_reticle, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(px(reticle_x as f32))
                                .top(px(reticle_y as f32))
                                .w(px(reticle_d as f32))
                                .h(px(reticle_d as f32))
                                .rounded_full()
                                .border_1()
                                .border_color(star_color.opacity(0.85)),
                        )
                    })
                    .child(preview)
                },
            )
            .child(
                v_flex()
                    .id("planetarium_overlay_controls")
                    .absolute()
                    .top_3()
                    .right_3()
                    .items_end()
                    .gap_1p5()
                    .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                        if this.active_opacity_layer.is_some()
                            && !this.opacity_slider.read(cx).is_dragging()
                        {
                            this.active_opacity_layer = None;
                            cx.notify();
                        }
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(sky_pill)
                            .child(grid_pill)
                            .child(
                                h_flex()
                                    .id("coord_mode_switch")
                                    .items_center()
                                    .gap_0p5()
                                    .p_0p5()
                                    .rounded_full()
                                    .bg(pill_bg)
                                    .border_1()
                                    .border_color(pill_border)
                                    .shadow_sm()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                                        if *hovered {
                                            this.cursor_position = None;
                                            this.sync_status_bar(cx);
                                            this.clear_hovered_star(cx);
                                        }
                                    }))
                                    .children(coord_options.into_iter().map(
                                        |(mode, id, icon, tip)| {
                                            let is_active = view_mode == mode;
                                            div()
                                                .id(id)
                                                .w(px(26.0))
                                                .h(px(22.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_full()
                                                .cursor_pointer()
                                                .when(is_active, |el| el.bg(active_bg))
                                                .when(!is_active, |el| {
                                                    el.hover(|s| s.bg(hover_bg))
                                                })
                                                .tooltip(Tooltip::text(tip))
                                                .on_click(cx.listener(
                                                    move |this, _event, _window, cx| {
                                                        this.set_view_mode(mode, cx);
                                                    },
                                                ))
                                                .child(
                                                    svg()
                                                        .data(icon)
                                                        .size(px(14.0))
                                                        .flex_none()
                                                        .text_color(if is_active {
                                                            active_icon
                                                        } else {
                                                            muted_icon
                                                        }),
                                                )
                                        },
                                    )),
                            ),
                    )
                    .when_some(active_popover_state, |el, (layer, is_enabled, stored_pct)| {
                        let value_label = if is_enabled {
                            format!("{stored_pct:.0}%")
                        } else {
                            "Off".to_string()
                        };

                        el.child(
                            v_flex()
                                .id("quick_layer_opacity_popover")
                                .w(px(212.0))
                                .px_3()
                                .py_2()
                                .gap_1p5()
                                .rounded_lg()
                                .bg(popover_bg)
                                .border_1()
                                .border_color(popover_border)
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_down(MouseButton::Right, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                                    if *hovered {
                                        this.cursor_position = None;
                                        this.sync_status_bar(cx);
                                        this.clear_hovered_star(cx);
                                    }
                                }))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_1p5()
                                                .child(
                                                    svg()
                                                        .data(layer.icon())
                                                        .size(px(13.0))
                                                        .flex_none()
                                                        .text_color(if is_enabled {
                                                            layer.active_icon_color(text_color)
                                                        } else {
                                                            muted_text
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(text_color)
                                                        .child(layer.title()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_buffer(cx)
                                                .text_size(px(11.5))
                                                .text_color(if is_enabled {
                                                    text_color
                                                } else {
                                                    muted_text
                                                })
                                                .child(value_label),
                                        ),
                                )
                                .child(self.opacity_slider.clone()),
                        )
                    }),
            )
    }
}

impl Focusable for PlanetariumPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_planetarium_trajectory(
    bounds: Bounds<Pixels>,
    trajectory: &DailyTrajectory,
    view_info: &PlanetariumViewInfo,
    event_badges: &[(celestial::TrajectoryEventPoint, f64, f64)],
    obj_rgb: Rgba,
    occlude_ground: bool,
    start_local_hour: f64,
    duration_hours: f64,
    is_faded: bool,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 || trajectory.samples.len() < 2 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let bw = f64::from(width);
    let bh = f64::from(height);
    let crisp_color = lerp_rgba(obj_rgb, rgb(0xffffff), 0.22).alpha(0.88);

    // 1. Hourly time tick marks along visible trajectory segments (faded near window ends when faded)
    for sample in &trajectory.samples {
        if !sample.above_horizon && occlude_ground {
            continue;
        }
        let fade = trajectory_edge_fade(sample.hour, duration_hours, is_faded);
        if fade <= 0.08 {
            continue;
        }
        let clock_h = start_local_hour + sample.hour;
        let nearest_hour = clock_h.round();
        if (clock_h - nearest_hour).abs() > 0.045 {
            continue;
        }
        if !is_faded && sample.hour >= duration_hours - 1e-3 {
            continue;
        }
        let Some((nx, ny)) = view_info.project_topo_to_ndc(sample.topo_enu) else {
            continue;
        };
        if nx.abs() > 1.05 || ny.abs() > 1.05 {
            continue;
        }
        let pt = point(
            px(origin_x + ((nx * 0.5 + 0.5) * bw) as f32),
            px(origin_y + ((0.5 - ny * 0.5) * bh) as f32),
        );
        let h_int = (nearest_hour.rem_euclid(24.0)).round() as u32 % 24;
        let r = if h_int.is_multiple_of(6) { 2.7 } else { 1.8 };
        let base_fill = if sample.above_horizon {
            crisp_color
        } else {
            obj_rgb.alpha(0.35)
        };
        let fill_color = Rgba {
            a: base_fill.a * fade,
            ..base_fill
        };
        let tick_halo = rgb(0x050811).alpha(0.75 * fade);
        paint_disc(window, pt, r, fill_color, 0.9, tick_halo);
    }

    // 2. Notable event point markers (Transit, Rise, Set, Lower Culmination)
    let dark_ring = rgb(0x060a14);
    let bright_ring = rgb(0xf4f8ff);
    for &(event, sx, sy) in event_badges {
        let pt = point(px(origin_x + sx as f32), px(origin_y + sy as f32));
        match event.kind {
            TrajectoryPointKind::Transit => {
                paint_disc(window, pt, 5.0, bright_ring.alpha(0.95), 1.4, dark_ring.alpha(0.88));
                paint_disc(window, pt, 3.2, obj_rgb.alpha(1.0), 0.0, dark_ring.alpha(0.0));
            }
            TrajectoryPointKind::Rise | TrajectoryPointKind::Set => {
                paint_disc(window, pt, 4.1, crisp_color, 1.3, dark_ring.alpha(0.85));
                paint_disc(window, pt, 2.2, dark_ring.alpha(0.9), 0.0, dark_ring.alpha(0.0));
            }
            TrajectoryPointKind::LowerTransit => {
                paint_disc(window, pt, 3.7, obj_rgb.alpha(0.85), 1.3, dark_ring.alpha(0.82));
                paint_disc(window, pt, 1.9, bright_ring.alpha(0.9), 0.0, dark_ring.alpha(0.0));
            }
        }
    }
}

impl Panel for PlanetariumPanel {
    fn persistent_name() -> &'static str {
        "planetarium_panel"
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Planetarium".into()
    }

    fn position(&self, _cx: &App) -> workspace::dock::DockPosition {
        DockPosition::Center
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool {
        matches!(kind, WorkspaceKind::Explore)
    }
}

impl Drop for PlanetariumPanel {
    fn drop(&mut self) {
        self.cached_texture = None;
        self.bridge.shutdown();
        if let Some(handle) = self.renderer_handle.take() {
            if let Err(e) = handle.join() {
                tracing::error!("Failed to join PlanetariumRenderer thread: {e:?}");
            }
        }
    }
}
