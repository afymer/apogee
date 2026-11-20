use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use celestial::coords::geodetic::GeodeticPosition;
use components::{Slider, SliderEvent};
use futures::StreamExt;
use gpui::{
    App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, Window, div, prelude::*, px, rgb, svg,
};
use observer::{GlobalObserver, ObserverClock, ObserverModel};
use planetarium::{
    BevyBridge, ObserverClockSpec, PlanetariumRenderer, PlanetariumViewInfo, ViewMode,
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
    HipsSurvey,
    Stars,
    AltAzGrid,
    EquatorialGrid,
}

impl QuickLayer {
    const SKY_LAYERS: [Self; 4] = [Self::Atmosphere, Self::Ground, Self::HipsSurvey, Self::Stars];
    const GRID_LAYERS: [Self; 2] = [Self::AltAzGrid, Self::EquatorialGrid];

    fn id(self) -> &'static str {
        match self {
            Self::Atmosphere => "quick_layer_atmosphere",
            Self::Ground => "quick_layer_ground",
            Self::HipsSurvey => "quick_layer_hips",
            Self::Stars => "quick_layer_stars",
            Self::AltAzGrid => "quick_layer_altaz_grid",
            Self::EquatorialGrid => "quick_layer_eq_grid",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Atmosphere => "Sky Atmosphere",
            Self::Ground => "Ground Floor",
            Self::HipsSurvey => "HiPS Survey",
            Self::Stars => "Stars",
            Self::AltAzGrid => "Alt/Az Grid",
            Self::EquatorialGrid => "Equatorial Grid",
        }
    }

    fn icon(self) -> &'static [u8] {
        match self {
            Self::Atmosphere => ICON_LAYER_ATMOSPHERE,
            Self::Ground => ICON_LAYER_GROUND,
            Self::HipsSurvey => ICON_LAYER_HIPS,
            Self::Stars => ICON_LAYER_STARS,
            Self::AltAzGrid => ICON_GRID_ALTAZ,
            Self::EquatorialGrid => ICON_GRID_EQUATORIAL,
        }
    }

    fn active_icon_color(self, default: Hsla) -> Hsla {
        match self {
            Self::AltAzGrid => rgb(0x66b5f5).into(),
            Self::EquatorialGrid => rgb(0xeb9b56).into(),
            Self::Atmosphere | Self::Ground | Self::HipsSurvey | Self::Stars => default,
        }
    }

    fn is_enabled(self, model: &SettingsModel) -> bool {
        match self {
            Self::Atmosphere => model.atmosphere_enabled(),
            Self::Ground => model.ground_enabled(),
            Self::HipsSurvey => model.hips_survey_enabled(),
            Self::Stars => model.stars_enabled(),
            Self::AltAzGrid => model.altaz_grid_enabled(),
            Self::EquatorialGrid => model.equatorial_grid_enabled(),
        }
    }

    fn stored_opacity(self, model: &SettingsModel) -> f32 {
        match self {
            Self::Atmosphere => model.atmosphere_strength(),
            Self::Ground => model.ground_opacity(),
            Self::HipsSurvey => model.hips_survey_opacity(),
            Self::Stars => model.stars_opacity(),
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
            Self::HipsSurvey => model.set_hips_survey_enabled(enabled, cx),
            Self::Stars => model.set_stars_enabled(enabled, cx),
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
            Self::HipsSurvey => model.preview_hips_survey_opacity(value, cx),
            Self::Stars => model.preview_stars_opacity(value, cx),
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
            Self::HipsSurvey => model.commit_hips_survey_opacity(value, cx),
            Self::Stars => model.commit_stars_opacity(value, cx),
            Self::AltAzGrid => model.commit_altaz_grid_opacity(value, cx),
            Self::EquatorialGrid => model.commit_equatorial_grid_opacity(value, cx),
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
    last_mouse_pos: Option<Point<Pixels>>,
    viewport_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    cursor_position: Option<Point<Pixels>>,
    latest_view_info: PlanetariumViewInfo,
    active_opacity_layer: Option<QuickLayer>,
    opacity_slider: Entity<Slider>,
    last_synced_observer: Option<(GeodeticPosition, ObserverClockSpec)>,
    last_synced_hips_grid: Option<bool>,
    last_synced_atmosphere_strength: Option<f32>,
    last_synced_ground_opacity: Option<f32>,
    last_synced_hips_survey_enabled: Option<bool>,
    last_synced_hips_survey_opacity: Option<f32>,
    last_synced_stars_opacity: Option<f32>,
    last_synced_altaz_grid_opacity: Option<f32>,
    last_synced_equatorial_grid_opacity: Option<f32>,
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
                })
                .detach();
            }

            let settings_entity = cx.try_global::<GlobalSettings>().map(|g| g.0.clone());
            let (
                initial_hips_grid,
                initial_atmosphere_strength,
                initial_ground_opacity,
                initial_hips_survey_enabled,
                initial_hips_survey_opacity,
                initial_stars_opacity,
                initial_altaz_grid_opacity,
                initial_equatorial_grid_opacity,
            ) = if let Some(entity) = settings_entity.as_ref() {
                let model = entity.read(cx);
                let show_grid = model.show_hips_grid();
                let atmo = model.effective_atmosphere_strength();
                let ground = model.effective_ground_opacity();
                let hips_enabled = model.hips_survey_enabled();
                let hips_opacity = model.hips_survey_opacity();
                let stars = model.effective_stars_opacity();
                let altaz_grid = model.effective_altaz_grid_opacity();
                let eq_grid = model.effective_equatorial_grid_opacity();
                bridge.set_hips_grid_visible(show_grid);
                bridge.set_atmosphere_strength(atmo);
                bridge.set_ground_opacity(ground);
                bridge.set_hips_survey_visible(hips_enabled);
                bridge.set_hips_survey_opacity(hips_opacity);
                bridge.set_star_opacity(stars);
                bridge.set_altaz_grid_opacity(altaz_grid);
                bridge.set_equatorial_grid_opacity(eq_grid);
                (
                    Some(show_grid),
                    Some(atmo),
                    Some(ground),
                    Some(hips_enabled),
                    Some(hips_opacity),
                    Some(stars),
                    Some(altaz_grid),
                    Some(eq_grid),
                )
            } else {
                (None, None, None, None, None, None, None, None)
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

            let panel = Self {
                bridge,
                cached_texture: None,
                renderer_handle: Some(renderer_handle),
                focus_handle: cx.focus_handle(),
                is_dragging: false,
                last_mouse_pos: None,
                viewport_bounds: Arc::new(Mutex::new(None)),
                cursor_position: None,
                latest_view_info: initial_view_info,
                active_opacity_layer: None,
                opacity_slider,
                last_synced_observer: initial_observer,
                last_synced_hips_grid: initial_hips_grid,
                last_synced_atmosphere_strength: initial_atmosphere_strength,
                last_synced_ground_opacity: initial_ground_opacity,
                last_synced_hips_survey_enabled: initial_hips_survey_enabled,
                last_synced_hips_survey_opacity: initial_hips_survey_opacity,
                last_synced_stars_opacity: initial_stars_opacity,
                last_synced_altaz_grid_opacity: initial_altaz_grid_opacity,
                last_synced_equatorial_grid_opacity: initial_equatorial_grid_opacity,
            };
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

    fn on_bevy_frame(&mut self, cx: &mut Context<Self>) {
        self.latest_view_info = self.bridge.view_info();
        self.sync_status_bar(cx);
        cx.notify();
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.is_dragging = true;
        self.cursor_position = Some(event.position);
        self.last_mouse_pos = Some(event.position);
        cx.notify();
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_position = Some(event.position);

        if !self.is_dragging {
            self.sync_status_bar(cx);
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.handle_mouse_up(cx);
            return;
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

    fn handle_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.is_dragging {
            self.is_dragging = false;
            self.last_mouse_pos = None;
            cx.notify();
        }
    }

    fn handle_scroll_wheel(&mut self, event: &ScrollWheelEvent, _cx: &mut Context<Self>) {
        self.cursor_position = Some(event.position);
        let delta_lines = match event.delta {
            ScrollDelta::Lines(pt) => pt.y as f64,
            ScrollDelta::Pixels(pt) => (f32::from(pt.y) / 20.0) as f64,
        };

        if delta_lines.abs() > 1e-4 {
            self.bridge.zoom(delta_lines);
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.bridge.snapshot();
        self.cached_texture = snapshot.texture_view;
        self.latest_view_info = snapshot.view_info;
        let bridge = self.bridge.clone();
        let last_size = snapshot.last_size;
        let view_mode = self.latest_view_info.view_mode;
        let viewport_bounds = self.viewport_bounds.clone();

        let cursor = if self.is_dragging {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };

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
            .size_full()
            .bg(rgb(0x000000))
            .relative()
            .cursor(cursor)
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !hovered {
                    this.cursor_position = None;
                    this.sync_status_bar(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.handle_mouse_down(event, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.handle_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.handle_mouse_up(cx);
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
                    .map(|view| gpui::surface(view.clone()).size_full()),
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
