use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use celestial::{HorizonProfile, MAX_HORIZON_ALT_DEG, MIN_HORIZON_ALT_DEG};
use components::{Slider, SliderEvent, StepPrecision, TextField, TextFieldEvent};
use gpui::{
    App, Bounds, ContentMask, Context, Corners, CursorStyle, Entity, FocusHandle, Focusable, Hsla,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, PathPromptOptions, Pixels, Point, Render, RenderImage, Rgba, ScrollDelta,
    ScrollWheelEvent, TitlebarOptions, Window, WindowBounds, WindowOptions, div, fill, hsla,
    point, prelude::*, px, rgb, size, svg,
};
use persistence::HorizonBackground;
use ui::{
    ActiveTheme, Button, ButtonCommon, ButtonStyle, Clickable, Color, Disableable, Divider,
    FixedWidth, Icon, IconButton, IconName, IconSize, Label, LabelCommon, LabelSize,
    StyledTypography, Tooltip, h_flex, v_flex,
};

use crate::extraction::{
    EDITOR_PREVIEW_MAX_HEIGHT, EDITOR_PREVIEW_MAX_WIDTH, HorizonExtractionParams,
    WorkingPanorama, decode_and_prepare_panorama, decode_for_gpui_preview,
    decode_thumbnail_render_image, extract_horizon_from_working,
};
use crate::state::GlobalHorizons;

const SIDEBAR_WIDTH: Pixels = px(256.0);
const RULER_HEIGHT_PX: f32 = 22.0;
const MAX_UNDO_STEPS: usize = 64;
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 8.0;

const ICON_UNDO: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M6.125 9.25L3 6.125L6.125 3" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M3 6.125H9.5625C11.461 6.125 13 7.664 13 9.5625C13 11.461 11.461 13 9.5625 13H7.375" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##;

const ICON_REDO: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
  <path d="M9.875 9.25L13 6.125L9.875 3" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M13 6.125H6.4375C4.539 6.125 3 7.664 3 9.5625C3 11.461 4.539 13 6.4375 13H8.625" stroke="black" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##;

/// Maps normalized display x in `[0, 1]` (centered on North `0°` at `0.5`) to world azimuth `[0, 360)`.
fn disp_u_to_world_az_deg(u_disp: f32) -> f64 {
    ((f64::from(u_disp) + 0.5).rem_euclid(1.0)) * 360.0
}

/// Maps world azimuth in degrees to normalized display x in `[0, 1)` (North `0°` at `0.5`).
fn world_az_deg_to_disp_u(az_deg: f64) -> f32 {
    ((az_deg + 180.0).rem_euclid(360.0) / 360.0) as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawToolMode {
    Draw,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskPreviewMode {
    TintGround,
    DimSky,
    LineOnly,
}

impl MaskPreviewMode {
    fn label(self) -> &'static str {
        match self {
            Self::TintGround => "Mask: Ground",
            Self::DimSky => "Mask: Sky",
            Self::LineOnly => "Mask: Line Only",
        }
    }

    fn cycle(self) -> Self {
        match self {
            Self::TintGround => Self::DimSky,
            Self::DimSky => Self::LineOnly,
            Self::LineOnly => Self::TintGround,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    id: String,
    asset: String,
    image_revision: u64,
}

impl ImageCacheKey {
    fn from_bg(bg: &HorizonBackground) -> Option<Self> {
        Some(Self {
            id: bg.id.clone(),
            asset: bg.image_asset.clone()?,
            image_revision: bg.image_revision,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct CanvasGeometry {
    ruler_bounds: Bounds<Pixels>,
    vp_bounds: Bounds<Pixels>,
    pano_w: f32,
    pano_h: f32,
    pano_x0: f32,
    pano_y: f32,
    m_min: i32,
    m_max: i32,
}

impl CanvasGeometry {
    fn compute(
        total_bounds: Bounds<Pixels>,
        zoom: f32,
        req_center_u: f32,
        req_center_v: f32,
    ) -> Option<Self> {
        let total_w = f32::from(total_bounds.size.width);
        let total_h = f32::from(total_bounds.size.height);
        if total_w <= 10.0 || total_h <= RULER_HEIGHT_PX + 10.0 {
            return None;
        }
        let origin_x = f32::from(total_bounds.origin.x);
        let origin_y = f32::from(total_bounds.origin.y);

        let ruler_bounds = Bounds::new(
            point(px(origin_x), px(origin_y)),
            size(px(total_w), px(RULER_HEIGHT_PX)),
        );
        let vp_h = total_h - RULER_HEIGHT_PX;
        let vp_bounds = Bounds::new(
            point(px(origin_x), px(origin_y + RULER_HEIGHT_PX)),
            size(px(total_w), px(vp_h)),
        );

        let base_w = total_w.min(vp_h * 2.0).max(1.0);
        let base_h = base_w * 0.5;
        let z = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let pano_w = base_w * z;
        let pano_h = base_h * z;

        let view_center_u = if pano_w <= total_w + 0.5 {
            0.5
        } else {
            req_center_u.rem_euclid(1.0)
        };
        let view_center_v = if pano_h <= vp_h + 0.5 {
            0.5
        } else {
            let max_off = 0.5 * (pano_h - vp_h) / pano_h;
            req_center_v.clamp(0.5 - max_off, 0.5 + max_off)
        };

        let vp_cx = origin_x + total_w * 0.5;
        let vp_cy = origin_y + RULER_HEIGHT_PX + vp_h * 0.5;
        let pano_x0 = vp_cx - view_center_u * pano_w;
        let pano_y = vp_cy - view_center_v * pano_h;

        let (m_min, m_max) = if pano_w <= total_w + 0.5 {
            (0, 0)
        } else {
            let min_m = ((origin_x - pano_x0) / pano_w).floor() as i32;
            let max_m = ((origin_x + total_w - pano_x0) / pano_w).ceil() as i32 - 1;
            (min_m.clamp(-3, 3), max_m.max(min_m).clamp(-3, 3))
        };

        Some(Self {
            ruler_bounds,
            vp_bounds,
            pano_w,
            pano_h,
            pano_x0,
            pano_y,
            m_min,
            m_max,
        })
    }

    fn world_x(&self, u_world: f32, tile_m: i32) -> f32 {
        self.pano_x0 + (tile_m as f32 + u_world) * self.pano_w
    }

    fn alt_to_y(&self, alt_deg: f32) -> f32 {
        let v = ((90.0 - alt_deg) / 180.0).clamp(0.0, 1.0);
        self.pano_y + v * self.pano_h
    }

    fn draw_x_bounds(&self) -> (f32, f32) {
        let vp_left = f32::from(self.vp_bounds.origin.x);
        let vp_right = vp_left + f32::from(self.vp_bounds.size.width);
        let span_left = self.pano_x0 + (self.m_min as f32) * self.pano_w;
        let span_right = self.pano_x0 + ((self.m_max + 1) as f32) * self.pano_w;
        (vp_left.max(span_left), vp_right.min(span_right))
    }

    fn screen_to_world(&self, pos: Point<Pixels>, clamp_x_to_pano: bool) -> (f64, f32) {
        let px_x = f32::from(pos.x);
        let px_y = f32::from(pos.y);

        let (x_lo, x_hi) = self.draw_x_bounds();
        let effective_x = if clamp_x_to_pano && x_hi > x_lo {
            px_x.clamp(x_lo, x_hi)
        } else {
            px_x
        };

        let u_disp = ((effective_x - self.pano_x0) / self.pano_w).rem_euclid(1.0);
        let v_world = ((px_y - self.pano_y) / self.pano_h).clamp(0.0, 1.0);

        let az_deg = disp_u_to_world_az_deg(u_disp);
        let alt_deg = (90.0 - v_world * 180.0).clamp(MIN_HORIZON_ALT_DEG, MAX_HORIZON_ALT_DEG);
        (az_deg, alt_deg)
    }

    fn screen_to_unwrapped_uv(&self, pos: Point<Pixels>) -> (f32, f32) {
        let px_x = f32::from(pos.x);
        let px_y = f32::from(pos.y);
        let u_unwrapped = (px_x - self.pano_x0) / self.pano_w;
        let v_world = ((px_y - self.pano_y) / self.pano_h).clamp(0.0, 1.0);
        (u_unwrapped, v_world)
    }
}

/// Opens the dedicated Horizon Backgrounds manager window, or activates it if already open.
pub fn open_horizon_manager_window(initial_selected_id: Option<String>, cx: &mut App) {
    let existing = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<HorizonManagerWindow>());

    if let Some(existing) = existing {
        let _ = existing.update(cx, |view, window, cx| {
            if let Some(id) = initial_selected_id {
                view.select_background(&id, window, cx);
            }
            window.activate_window();
        });
        return;
    }

    cx.defer(move |cx| {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Apogee — Horizon Backgrounds".into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_movable: true,
            kind: gpui::WindowKind::Normal,
            window_background: cx.theme().window_background_appearance(),
            window_min_size: Some(gpui::Size {
                width: px(760.0),
                height: px(480.0),
            }),
            window_bounds: Some(WindowBounds::centered(
                gpui::size(px(1120.0), px(680.0)),
                cx,
            )),
            ..Default::default()
        };

        if let Err(err) = cx.open_window(options, |window, cx| {
            cx.new(|cx| HorizonManagerWindow::new(initial_selected_id.as_deref(), window, cx))
        }) {
            tracing::error!("Failed to open horizon manager window: {err:?}");
        }
    });
}

/// Dedicated window for importing 360° equirectangular panoramas, extracting horizons,
/// editing horizon curves, and calibrating North azimuth alignment.
pub struct HorizonManagerWindow {
    focus_handle: FocusHandle,
    selected_id: Option<String>,
    editing_profile: Option<HorizonProfile>,
    undo_stack: Vec<HorizonProfile>,
    redo_stack: Vec<HorizonProfile>,

    name_input: Entity<TextField>,
    offset_slider: Entity<Slider>,
    offset_input: Entity<TextField>,
    bias_slider: Entity<Slider>,
    smooth_slider: Entity<Slider>,
    extraction_params: HorizonExtractionParams,

    tool_mode: DrawToolMode,
    mask_preview: MaskPreviewMode,

    zoom: f32,
    view_center_u: f32,
    view_center_v: f32,

    canvas_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    is_drawing: bool,
    last_draw_pt: Option<(f64, f32)>,
    line_anchor: Option<(f64, f32)>,
    is_panning: bool,
    pan_did_drag: bool,
    pan_last_pos: Option<Point<Pixels>>,
    hover_pos: Option<Point<Pixels>>,
    offset_drag_active: bool,
    extract_drag_active: bool,

    loaded_image_key: Option<ImageCacheKey>,
    preview_image: Option<Arc<RenderImage>>,
    working_panorama: Option<WorkingPanorama>,
    preview_load_generation: u64,

    thumbnails: HashMap<ImageCacheKey, Arc<RenderImage>>,
    loading_thumbnails: HashSet<ImageCacheKey>,

    is_importing: bool,
    status_message: Option<String>,
    warning_message: Option<String>,
    error_message: Option<String>,
}

impl Focusable for HorizonManagerWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl HorizonManagerWindow {
    pub fn new(
        initial_selected_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let (initial_id, initial_name, initial_profile) =
            if let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.read(cx)) {
                let target = initial_selected_id
                    .and_then(|id| horizons.background(id))
                    .or_else(|| horizons.backgrounds().first());
                if let Some(bg) = target {
                    (
                        Some(bg.id.clone()),
                        bg.name.clone(),
                        Some(HorizonProfile::from_samples(
                            bg.samples_deg.clone(),
                            bg.azimuth_offset_deg,
                        )),
                    )
                } else {
                    (None, String::new(), None)
                }
            } else {
                (None, String::new(), None)
            };

        let initial_offset_deg = initial_profile
            .as_ref()
            .map_or(0.0, |p| p.azimuth_offset_deg);
        let default_params = HorizonExtractionParams::default();

        let name_input = cx.new(|cx| {
            TextField::new(initial_name, window, cx).placeholder("Background name")
        });
        let offset_slider = cx.new(|_cx| {
            Slider::new((initial_offset_deg / 360.0) as f32)
                .default_value(0.0)
                .accent_color(rgb(0x42b3c2).into())
        });
        let offset_input = cx.new(|cx| {
            TextField::new(format!("{initial_offset_deg:.1}"), window, cx)
                .placeholder("0.0")
                .suffix("°")
                .monospace(true)
        });
        let bias_slider = cx.new(|_cx| {
            Slider::new(f32::midpoint(default_params.sky_bias, 1.0))
                .center_mark(0.5)
                .fill_from_center(true)
                .default_value(0.5)
                .accent_color(rgb(0x42b3c2).into())
        });
        let smooth_slider = cx.new(|_cx| {
            Slider::new(default_params.smoothness)
                .default_value(default_params.smoothness)
                .accent_color(rgb(0x42b3c2).into())
        });

        Self::wire_name_input(&name_input, window, cx);
        Self::wire_offset_slider(&offset_slider, cx);
        Self::wire_offset_input(&offset_input, window, cx);
        Self::wire_extraction_sliders(&bias_slider, &smooth_slider, cx);

        if let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
            cx.observe_in(&horizons, window, |this, model, window, cx| {
                this.on_horizons_changed(&model, window, cx);
            })
            .detach();
        }

        cx.on_window_closed(|cx, _window_id| {
            if let Some(existing_window) = cx
                .windows()
                .into_iter()
                .find_map(|window| window.downcast::<HorizonManagerWindow>())
                && cx.windows().len() == 1
            {
                let _ = existing_window.update(cx, |_, window, _| {
                    window.remove_window();
                });
            }
        })
        .detach();

        let mut this = Self {
            focus_handle,
            selected_id: initial_id.clone(),
            editing_profile: initial_profile,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            name_input,
            offset_slider,
            offset_input,
            bias_slider,
            smooth_slider,
            extraction_params: default_params,
            tool_mode: DrawToolMode::Draw,
            mask_preview: MaskPreviewMode::TintGround,
            zoom: 1.0,
            view_center_u: 0.5,
            view_center_v: 0.5,
            canvas_bounds: Arc::new(Mutex::new(None)),
            is_drawing: false,
            last_draw_pt: None,
            line_anchor: None,
            is_panning: false,
            pan_did_drag: false,
            pan_last_pos: None,
            hover_pos: None,
            offset_drag_active: false,
            extract_drag_active: false,
            loaded_image_key: None,
            preview_image: None,
            working_panorama: None,
            preview_load_generation: 0,
            thumbnails: HashMap::new(),
            loading_thumbnails: HashSet::new(),
            is_importing: false,
            status_message: None,
            warning_message: None,
            error_message: None,
        };

        if let Some(id) = initial_id {
            this.trigger_preview_load(&id, window, cx);
        }
        this.schedule_missing_thumbnails(cx);
        this
    }

    fn wire_name_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(input, window, |this, entity, event: &TextFieldEvent, window, cx| {
            match event {
                TextFieldEvent::Submit(raw) | TextFieldEvent::Blur(raw) => {
                    let Some(id) = this.selected_id.clone() else {
                        return;
                    };
                    let trimmed = raw.trim().to_string();
                    if trimmed.is_empty() {
                        if let Some(current_name) = cx
                            .try_global::<GlobalHorizons>()
                            .and_then(|g| g.0.read(cx).background(&id).map(|b| b.name.clone()))
                        {
                            entity.update(cx, |f, cx| f.set_text(current_name, cx));
                        }
                        return;
                    }
                    if let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
                        let unchanged = horizons
                            .read(cx)
                            .background(&id)
                            .is_some_and(|b| b.name == trimmed);
                        if !unchanged {
                            horizons.update(cx, |model, cx| {
                                model.rename(&id, &trimmed, cx);
                            });
                        }
                    }
                    if matches!(event, TextFieldEvent::Submit(_)) {
                        window.focus(&this.focus_handle, cx);
                    }
                }
                TextFieldEvent::Cancel => {
                    if let Some(id) = this.selected_id.as_deref()
                        && let Some(current_name) = cx
                            .try_global::<GlobalHorizons>()
                            .and_then(|g| g.0.read(cx).background(id).map(|b| b.name.clone()))
                    {
                        entity.update(cx, |f, cx| f.set_text(current_name, cx));
                    }
                    window.focus(&this.focus_handle, cx);
                }
                TextFieldEvent::Change(_) | TextFieldEvent::Step { .. } => {}
            }
        })
        .detach();
    }

    fn wire_offset_slider(slider: &Entity<Slider>, cx: &mut Context<Self>) {
        cx.subscribe(slider, |this, _slider, event: &SliderEvent, cx| {
            let Some(mut profile) = this.editing_profile.clone() else {
                return;
            };
            match *event {
                SliderEvent::Change(t) => {
                    if !this.offset_drag_active {
                        this.offset_drag_active = true;
                        this.push_undo_snapshot();
                    }
                    profile.azimuth_offset_deg = (f64::from(t.clamp(0.0, 1.0)) * 360.0) % 360.0;
                    this.apply_profile_change(profile, false, cx);
                }
                SliderEvent::Commit(t) => {
                    if !this.offset_drag_active {
                        this.push_undo_snapshot();
                    }
                    this.offset_drag_active = false;
                    profile.azimuth_offset_deg = (f64::from(t.clamp(0.0, 1.0)) * 360.0) % 360.0;
                    this.apply_profile_change(profile, true, cx);
                }
            }
        })
        .detach();
    }

    fn wire_offset_input(input: &Entity<TextField>, window: &Window, cx: &mut Context<Self>) {
        cx.subscribe_in(input, window, |this, entity, event: &TextFieldEvent, window, cx| {
            let Some(mut profile) = this.editing_profile.clone() else {
                return;
            };
            match event {
                TextFieldEvent::Submit(raw) => {
                    if let Some(deg) = parse_offset_deg(raw) {
                        this.push_undo_snapshot();
                        profile.azimuth_offset_deg = deg;
                        this.apply_profile_change(profile, true, cx);
                        entity.update(cx, |f, cx| f.set_invalid(false, cx));
                        window.focus(&this.focus_handle, cx);
                    } else {
                        entity.update(cx, |f, cx| f.set_invalid(true, cx));
                    }
                }
                TextFieldEvent::Blur(raw) => {
                    if let Some(deg) = parse_offset_deg(raw) {
                        if (deg - profile.azimuth_offset_deg).abs() > 1e-4 {
                            this.push_undo_snapshot();
                            profile.azimuth_offset_deg = deg;
                            this.apply_profile_change(profile, true, cx);
                        } else {
                            entity.update(cx, |f, cx| {
                                f.set_text(format!("{deg:.1}"), cx);
                            });
                        }
                    } else {
                        entity.update(cx, |f, cx| {
                            f.set_text(format!("{:.1}", profile.azimuth_offset_deg), cx);
                        });
                    }
                    entity.update(cx, |f, cx| f.set_invalid(false, cx));
                }
                TextFieldEvent::Step {
                    direction,
                    precision,
                } => {
                    let base = parse_offset_deg(entity.read(cx).text())
                        .unwrap_or(profile.azimuth_offset_deg);
                    let step = match precision {
                        StepPrecision::Fine => 0.5,
                        StepPrecision::Normal => 1.0,
                        StepPrecision::Coarse => 10.0,
                    };
                    let next = (base + f64::from(*direction) * step).rem_euclid(360.0);
                    this.push_undo_snapshot();
                    profile.azimuth_offset_deg = next;
                    this.apply_profile_change(profile, true, cx);
                }
                TextFieldEvent::Cancel => {
                    entity.update(cx, |f, cx| {
                        f.set_text(format!("{:.1}", profile.azimuth_offset_deg), cx);
                        f.set_invalid(false, cx);
                    });
                    window.focus(&this.focus_handle, cx);
                }
                TextFieldEvent::Change(_) => {}
            }
        })
        .detach();
    }

    fn wire_extraction_sliders(
        bias_slider: &Entity<Slider>,
        smooth_slider: &Entity<Slider>,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(bias_slider, |this, _slider, event: &SliderEvent, cx| {
            match *event {
                SliderEvent::Change(t) => {
                    this.extraction_params.sky_bias = (t.clamp(0.0, 1.0) * 2.0) - 1.0;
                    this.run_live_extraction(false, cx);
                }
                SliderEvent::Commit(t) => {
                    this.extraction_params.sky_bias = (t.clamp(0.0, 1.0) * 2.0) - 1.0;
                    this.run_live_extraction(true, cx);
                }
            }
        })
        .detach();

        cx.subscribe(smooth_slider, |this, _slider, event: &SliderEvent, cx| {
            match *event {
                SliderEvent::Change(t) => {
                    this.extraction_params.smoothness = t.clamp(0.0, 1.0);
                    this.run_live_extraction(false, cx);
                }
                SliderEvent::Commit(t) => {
                    this.extraction_params.smoothness = t.clamp(0.0, 1.0);
                    this.run_live_extraction(true, cx);
                }
            }
        })
        .detach();
    }

    fn on_horizons_changed(
        &mut self,
        model: &Entity<crate::state::HorizonModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.schedule_missing_thumbnails(cx);
        let model_ref = model.read(cx);

        let Some(selected_id) = self.selected_id.clone() else {
            if let Some(first) = model_ref.backgrounds().first() {
                let id = first.id.clone();
                self.select_background(&id, window, cx);
            }
            return;
        };

        let Some(bg) = model_ref.background(&selected_id).cloned() else {
            // Selected background was deleted; select first remaining or clear.
            if let Some(first) = model_ref.backgrounds().first() {
                let id = first.id.clone();
                self.select_background(&id, window, cx);
            } else {
                self.clear_selection(cx);
            }
            return;
        };

        let next_key = ImageCacheKey::from_bg(&bg);
        if next_key != self.loaded_image_key {
            self.trigger_preview_load(&selected_id, window, cx);
        }

        if !self.name_input.read(cx).is_focused(window)
            && self.name_input.read(cx).text() != bg.name
        {
            self.name_input
                .update(cx, |field, cx| field.set_text(bg.name.clone(), cx));
        }

        // Only overwrite local editing state if we're not actively dragging and model differs.
        if !self.is_drawing && !self.offset_drag_active && !self.extract_drag_active {
            let model_profile =
                HorizonProfile::from_samples(bg.samples_deg.clone(), bg.azimuth_offset_deg);
            if self.editing_profile.as_ref() != Some(&model_profile) {
                self.editing_profile = Some(model_profile.clone());
                self.sync_offset_controls(model_profile.azimuth_offset_deg, window, cx);
            }
        }
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected_id = None;
        self.editing_profile = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.line_anchor = None;
        self.loaded_image_key = None;
        self.preview_image = None;
        self.working_panorama = None;
        self.name_input.update(cx, |f, cx| f.set_text("", cx));
        cx.notify();
    }

    pub fn select_background(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_id.as_deref() == Some(id) {
            return;
        }
        let Some(bg) = cx
            .try_global::<GlobalHorizons>()
            .and_then(|g| g.0.read(cx).background(id).cloned())
        else {
            return;
        };

        self.selected_id = Some(id.to_string());
        let profile = HorizonProfile::from_samples(bg.samples_deg.clone(), bg.azimuth_offset_deg);
        self.editing_profile = Some(profile.clone());
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.line_anchor = None;
        self.is_drawing = false;
        self.last_draw_pt = None;
        self.zoom = 1.0;
        self.view_center_u = 0.5;
        self.view_center_v = 0.5;
        self.error_message = None;
        self.warning_message = None;
        self.status_message = None;

        self.name_input
            .update(cx, |field, cx| field.set_text(bg.name.clone(), cx));
        self.sync_offset_controls(profile.azimuth_offset_deg, window, cx);
        self.trigger_preview_load(id, window, cx);
        cx.notify();
    }

    fn sync_offset_controls(&self, offset_deg: f64, window: &Window, cx: &mut Context<Self>) {
        let norm = (offset_deg.rem_euclid(360.0) / 360.0) as f32;
        self.offset_slider.update(cx, |s, cx| s.set_value(norm, cx));
        self.offset_input.update(cx, |f, cx| {
            if !f.is_focused(window) {
                f.set_text(format!("{offset_deg:.1}"), cx);
                f.set_invalid(false, cx);
            }
        });
    }

    fn sync_offset_controls_unconditional(&self, offset_deg: f64, cx: &mut Context<Self>) {
        let norm = (offset_deg.rem_euclid(360.0) / 360.0) as f32;
        self.offset_slider.update(cx, |s, cx| s.set_value(norm, cx));
        self.offset_input.update(cx, |f, cx| {
            f.set_text(format!("{offset_deg:.1}"), cx);
            f.set_invalid(false, cx);
        });
    }

    fn trigger_preview_load(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) else {
            return;
        };
        let (key, bytes_opt) = {
            let m = horizons.read(cx);
            let bg = m.background(id);
            let key = bg.and_then(ImageCacheKey::from_bg);
            let bytes = if key.is_some() {
                m.load_image_bytes(id)
            } else {
                None
            };
            (key, bytes)
        };

        self.loaded_image_key.clone_from(&key);
        self.preview_image = None;
        self.working_panorama = None;

        let (Some(expected_key), Some(bytes)) = (key, bytes_opt) else {
            cx.notify();
            return;
        };

        self.preview_load_generation = self.preview_load_generation.wrapping_add(1);
        let generation = self.preview_load_generation;

        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move {
                    decode_for_gpui_preview(
                        &bytes,
                        EDITOR_PREVIEW_MAX_WIDTH,
                        EDITOR_PREVIEW_MAX_HEIGHT,
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.preview_load_generation != generation
                    || this.loaded_image_key.as_ref() != Some(&expected_key)
                {
                    return;
                }
                match decoded {
                    Ok((render_img, working)) => {
                        this.preview_image = Some(render_img);
                        this.working_panorama = Some(working);
                        this.error_message = None;
                    }
                    Err(err) => {
                        this.error_message = Some(format!("Failed to load 360° preview: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn schedule_missing_thumbnails(&mut self, cx: &mut Context<Self>) {
        let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) else {
            return;
        };
        let mut needed: Vec<(ImageCacheKey, Vec<u8>)> = Vec::new();
        {
            let m = horizons.read(cx);
            let active_keys: HashSet<ImageCacheKey> = m
                .backgrounds()
                .iter()
                .filter_map(ImageCacheKey::from_bg)
                .collect();
            self.thumbnails.retain(|k, _| active_keys.contains(k));

            for bg in m.backgrounds() {
                if let Some(key) = ImageCacheKey::from_bg(bg)
                    && !self.thumbnails.contains_key(&key)
                    && !self.loading_thumbnails.contains(&key)
                    && let Some(bytes) = m.load_image_bytes(&bg.id)
                {
                    self.loading_thumbnails.insert(key.clone());
                    needed.push((key, bytes));
                }
            }
        }

        for (key, bytes) in needed {
            cx.spawn(async move |this, cx| {
                let res = cx
                    .background_executor()
                    .spawn(async move { decode_thumbnail_render_image(&bytes) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.loading_thumbnails.remove(&key);
                    if let Ok(thumb) = res {
                        this.thumbnails.insert(key, thumb);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn push_undo_snapshot(&mut self) {
        let Some(current) = self.editing_profile.clone() else {
            return;
        };
        if self.undo_stack.last() == Some(&current) {
            return;
        }
        self.undo_stack.push(current);
        if self.undo_stack.len() > MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        if let Some(curr) = self.editing_profile.clone() {
            self.redo_stack.push(curr);
        }
        self.line_anchor = None;
        self.apply_profile_change(prev, true, cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        if let Some(curr) = self.editing_profile.clone() {
            self.undo_stack.push(curr);
        }
        self.line_anchor = None;
        self.apply_profile_change(next, true, cx);
    }

    fn apply_profile_change(
        &mut self,
        mut profile: HorizonProfile,
        commit: bool,
        cx: &mut Context<Self>,
    ) {
        profile.sanitize();
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        self.editing_profile = Some(profile.clone());
        self.sync_offset_controls_unconditional(profile.azimuth_offset_deg, cx);

        if let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
            horizons.update(cx, |model, cx| {
                if commit {
                    model.commit_horizon(&id, profile.samples_deg, profile.azimuth_offset_deg, cx);
                } else {
                    model.preview_horizon(&id, profile.samples_deg, profile.azimuth_offset_deg, cx);
                }
            });
        }
        cx.notify();
    }

    fn run_live_extraction(&mut self, commit: bool, cx: &mut Context<Self>) {
        if self.working_panorama.is_none() || self.editing_profile.is_none() {
            return;
        }
        if commit {
            if !self.extract_drag_active {
                self.push_undo_snapshot();
            }
            self.extract_drag_active = false;
        } else if !self.extract_drag_active {
            self.extract_drag_active = true;
            self.push_undo_snapshot();
        }
        let (Some(working), Some(mut profile)) =
            (self.working_panorama.as_ref(), self.editing_profile.clone())
        else {
            return;
        };
        profile.samples_deg = extract_horizon_from_working(working, self.extraction_params);
        self.apply_profile_change(profile, commit, cx);
    }

    fn import_via_dialog(
        &mut self,
        replace_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_importing {
            return;
        }
        let prompt = if replace_id.is_some() {
            "Replace 360° Equirectangular Panorama"
        } else {
            "Import 360° Equirectangular Panorama"
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt.into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };

            let _ = this.update(cx, |this, cx| {
                this.is_importing = true;
                this.error_message = None;
                this.warning_message = None;
                this.status_message = Some("Decoding 360° panorama and extracting horizon…".into());
                cx.notify();
            });

            let params = this
                .read_with(cx, |this, _| this.extraction_params)
                .unwrap_or_default();

            let path_clone = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path_clone)
                        .map_err(|e| format!("Failed to read file {}: {e}", path_clone.display()))?;
                    let prepared = decode_and_prepare_panorama(&bytes)?;
                    let samples = extract_horizon_from_working(&prepared.working, params);
                    Ok::<_, String>((prepared, samples))
                })
                .await;

            let _ = this.update_in(cx, |this, window, cx| {
                this.is_importing = false;
                this.status_message = None;

                let (prepared, samples) = match result {
                    Ok(pair) => pair,
                    Err(err) => {
                        this.error_message = Some(err);
                        cx.notify();
                        return;
                    }
                };

                let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) else {
                    this.error_message = Some("Horizon catalog service unavailable.".into());
                    cx.notify();
                    return;
                };

                let source_filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(ToString::to_string);
                let default_name = derive_name_from_path(&path);
                let aspect_warning = prepared.aspect_ratio_warning.clone();

                if let Some(target_id) = replace_id {
                    let res = horizons.update(cx, |model, cx| {
                        model.replace_prepared_image(
                            &target_id,
                            source_filename,
                            &prepared,
                            Some(samples.clone()),
                            cx,
                        )
                    });
                    match res {
                        Ok(()) => {
                            this.warning_message = aspect_warning;
                            this.undo_stack.clear();
                            this.redo_stack.clear();
                            if let Some(mut profile) = this.editing_profile.clone() {
                                profile.samples_deg = samples;
                                this.editing_profile = Some(profile);
                            }
                            this.trigger_preview_load(&target_id, window, cx);
                        }
                        Err(err) => {
                            this.error_message = Some(format!("Failed to replace image: {err}"));
                        }
                    }
                } else {
                    let res = horizons.update(cx, |model, cx| {
                        model.import_prepared(&default_name, source_filename, &prepared, samples, cx)
                    });
                    match res {
                        Ok(new_id) => {
                            this.select_background(&new_id, window, cx);
                            this.warning_message = aspect_warning;
                        }
                        Err(err) => {
                            this.error_message = Some(format!("Failed to import panorama: {err}"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_blank_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) else {
            return;
        };
        let new_id = horizons.update(cx, |model, cx| model.create_blank("Custom Horizon", cx));
        self.select_background(&new_id, window, cx);
    }

    fn delete_selected_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) else {
            return;
        };
        horizons.update(cx, |model, cx| {
            model.delete(&id, cx);
        });
        let next_id = horizons
            .read(cx)
            .backgrounds()
            .first()
            .map(|b| b.id.clone());
        if let Some(next_id) = next_id {
            self.select_background(&next_id, window, cx);
        } else {
            self.clear_selection(cx);
        }
    }

    fn current_geometry(&self) -> Option<CanvasGeometry> {
        let total_bounds = self.canvas_bounds.lock().ok().and_then(|g| *g)?;
        CanvasGeometry::compute(
            total_bounds,
            self.zoom,
            self.view_center_u,
            self.view_center_v,
        )
    }

    fn handle_canvas_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(geo) = self.current_geometry() else {
            return;
        };
        let Some(mut profile) = self.editing_profile.clone() else {
            return;
        };
        if !geo.vp_bounds.contains(&event.position) {
            return;
        }

        if event.button == MouseButton::Right || event.button == MouseButton::Middle {
            self.is_panning = true;
            self.pan_did_drag = false;
            self.pan_last_pos = Some(event.position);
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }

        let (az_deg, alt_deg) = geo.screen_to_world(event.position, true);
        let use_line = self.tool_mode == DrawToolMode::Line || event.modifiers.shift;

        if use_line {
            if let Some((az0, alt0)) = self.line_anchor {
                self.push_undo_snapshot();
                profile.set_segment_world_az(az0, alt0, az_deg, alt_deg);
                self.line_anchor = Some((az_deg, alt_deg));
                self.apply_profile_change(profile, true, cx);
            } else {
                self.line_anchor = Some((az_deg, alt_deg));
                cx.notify();
            }
            return;
        }

        self.push_undo_snapshot();
        self.is_drawing = true;
        self.last_draw_pt = Some((az_deg, alt_deg));
        profile.set_segment_world_az(az_deg, alt_deg, az_deg, alt_deg);
        self.apply_profile_change(profile, false, cx);
    }

    fn handle_canvas_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(geo) = self.current_geometry() else {
            return;
        };
        self.hover_pos = if geo.vp_bounds.contains(&event.position) {
            Some(event.position)
        } else {
            None
        };

        if self.is_panning {
            if event.pressed_button == Some(MouseButton::Right)
                || event.pressed_button == Some(MouseButton::Middle)
            {
                if let Some(last) = self.pan_last_pos {
                    let dx = f32::from(event.position.x - last.x);
                    let dy = f32::from(event.position.y - last.y);
                    if dx.abs() > 1.0 || dy.abs() > 1.0 {
                        self.pan_did_drag = true;
                    }
                    if geo.pano_w > f32::from(geo.vp_bounds.size.width) + 0.5 {
                        self.view_center_u =
                            (self.view_center_u - dx / geo.pano_w).rem_euclid(1.0);
                    }
                    let vp_h = f32::from(geo.vp_bounds.size.height);
                    if geo.pano_h > vp_h + 0.5 {
                        let max_off = 0.5 * (geo.pano_h - vp_h) / geo.pano_h;
                        self.view_center_v =
                            (self.view_center_v - dy / geo.pano_h).clamp(0.5 - max_off, 0.5 + max_off);
                    }
                }
                self.pan_last_pos = Some(event.position);
                cx.notify();
            } else {
                self.is_panning = false;
                self.pan_last_pos = None;
            }
            return;
        }

        if self.is_drawing {
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_drawing(cx);
                return;
            }
            if let Some(mut profile) = self.editing_profile.clone() {
                let (az_deg, alt_deg) = geo.screen_to_world(event.position, true);
                let (az0, alt0) = self.last_draw_pt.unwrap_or((az_deg, alt_deg));
                profile.set_segment_world_az(az0, alt0, az_deg, alt_deg);
                self.last_draw_pt = Some((az_deg, alt_deg));
                self.apply_profile_change(profile, false, cx);
                return;
            }
        }

        cx.notify();
    }

    fn handle_canvas_mouse_up(&mut self, button: MouseButton, cx: &mut Context<Self>) {
        if (button == MouseButton::Right || button == MouseButton::Middle) && self.is_panning {
            let was_click = !self.pan_did_drag && button == MouseButton::Right;
            self.is_panning = false;
            self.pan_did_drag = false;
            self.pan_last_pos = None;
            if was_click && self.line_anchor.take().is_some() {
                cx.notify();
            }
            return;
        }
        if button == MouseButton::Left && self.is_drawing {
            self.finish_drawing(cx);
        }
    }

    fn finish_drawing(&mut self, cx: &mut Context<Self>) {
        self.is_drawing = false;
        self.last_draw_pt = None;
        if let Some(profile) = self.editing_profile.clone() {
            self.apply_profile_change(profile, true, cx);
        }
    }

    fn handle_canvas_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(geo) = self.current_geometry() else {
            return;
        };
        if !geo.vp_bounds.contains(&event.position) {
            return;
        }
        let delta_lines = match event.delta {
            ScrollDelta::Lines(pt) => pt.y,
            ScrollDelta::Pixels(pt) => f32::from(pt.y) / 20.0,
        };
        if delta_lines.abs() < 1e-4 {
            return;
        }

        let old_zoom = self.zoom;
        let factor = (1.16_f32).powf(delta_lines.clamp(-5.0, 5.0));
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - old_zoom).abs() < 1e-4 {
            return;
        }

        if new_zoom <= 1.01 {
            self.zoom = 1.0;
            self.view_center_u = 0.5;
            self.view_center_v = 0.5;
            cx.notify();
            return;
        }

        let (u_cursor, v_cursor) = geo.screen_to_unwrapped_uv(event.position);
        let vp_cx = f32::from(geo.vp_bounds.origin.x) + f32::from(geo.vp_bounds.size.width) * 0.5;
        let vp_cy = f32::from(geo.vp_bounds.origin.y) + f32::from(geo.vp_bounds.size.height) * 0.5;
        let dx_from_center = f32::from(event.position.x) - vp_cx;
        let dy_from_center = f32::from(event.position.y) - vp_cy;

        let new_total = Bounds::new(
            point(
                geo.ruler_bounds.origin.x,
                geo.ruler_bounds.origin.y,
            ),
            size(
                geo.vp_bounds.size.width,
                px(f32::from(geo.vp_bounds.size.height) + RULER_HEIGHT_PX),
            ),
        );
        if let Some(next_base) = CanvasGeometry::compute(new_total, new_zoom, 0.5, 0.5) {
            self.zoom = new_zoom;
            self.view_center_u = (u_cursor - dx_from_center / next_base.pano_w).rem_euclid(1.0);
            self.view_center_v = (v_cursor - dy_from_center / next_base.pano_h).clamp(0.0, 1.0);
            cx.notify();
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let backgrounds: Vec<HorizonBackground> = cx
            .try_global::<GlobalHorizons>()
            .map(|g| g.0.read(cx).backgrounds().to_vec())
            .unwrap_or_default();

        v_flex()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors.text_muted)
                                    .child("BACKGROUNDS"),
                            )
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(10.5))
                                    .text_color(colors.text_muted)
                                    .child(format!("{}", backgrounds.len())),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                div().flex_1().child(
                                    Button::new("import_360_sidebar_btn", "Import 360°…")
                                        .start_icon(
                                            Icon::new(IconName::Image).size(IconSize::Small),
                                        )
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::Small)
                                        .full_width()
                                        .disabled(self.is_importing)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.import_via_dialog(None, window, cx);
                                        })),
                                ),
                            )
                            .child(
                                IconButton::new("create_blank_horizon_btn", IconName::Plus)
                                    .icon_size(IconSize::Small)
                                    .style(ButtonStyle::Subtle)
                                    .tooltip(Tooltip::text("New blank flat horizon (no image)"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.create_blank_background(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("horizon_bg_list")
                    .flex_1()
                    .p_2()
                    .gap_1p5()
                    .overflow_y_scroll()
                    .when(backgrounds.is_empty(), |el| {
                        el.child(
                            v_flex()
                                .py_8()
                                .px_3()
                                .items_center()
                                .gap_1()
                                .child(
                                    Label::new("No backgrounds yet")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new("Import a 360° equirectangular photo to extract its horizon.")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .children(backgrounds.into_iter().map(|bg| {
                        let id = bg.id.clone();
                        let is_selected = self.selected_id.as_deref() == Some(id.as_str());
                        let profile = HorizonProfile::from_samples(
                            bg.samples_deg.clone(),
                            bg.azimuth_offset_deg,
                        );
                        let (min_alt, max_alt) = profile.min_max_altitude_deg();
                        let thumb = ImageCacheKey::from_bg(&bg)
                            .and_then(|k| self.thumbnails.get(&k).cloned());

                        h_flex()
                            .id(format!("horizon_item_{id}"))
                            .w_full()
                            .p_1p5()
                            .gap_2()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if is_selected {
                                cyan.opacity(0.16)
                            } else {
                                colors.surface_background.opacity(0.55)
                            })
                            .border_1()
                            .border_color(if is_selected {
                                cyan.opacity(0.75)
                            } else {
                                colors.border_variant
                            })
                            .hover(|s| {
                                s.bg(if is_selected {
                                    cyan.opacity(0.20)
                                } else {
                                    colors.element_hover
                                })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_background(&id, window, cx);
                            }))
                            .child(
                                div()
                                    .w(px(64.0))
                                    .h(px(32.0))
                                    .flex_none()
                                    .rounded_sm()
                                    .overflow_hidden()
                                    .bg(rgb(0x090d16))
                                    .border_1()
                                    .border_color(colors.border_variant)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(if let Some(img) = thumb {
                                        gpui::img(img).size_full().into_any_element()
                                    } else {
                                        Icon::new(IconName::Image)
                                            .size(IconSize::Small)
                                            .color(Color::Muted)
                                            .into_any_element()
                                    }),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.0))
                                            .font_weight(if is_selected {
                                                gpui::FontWeight::SEMIBOLD
                                            } else {
                                                gpui::FontWeight::MEDIUM
                                            })
                                            .text_color(colors.text)
                                            .child(bg.name),
                                    )
                                    .child(
                                        div()
                                            .font_buffer(cx)
                                            .text_size(px(10.0))
                                            .text_color(colors.text_muted)
                                            .truncate()
                                            .child(format!(
                                                "{min_alt:+.0}°..{max_alt:+.0}° · N{:+.0}°",
                                                bg.azimuth_offset_deg
                                            )),
                                    ),
                            )
                    })),
            )
    }

    fn render_empty_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .gap_3()
            .bg(colors.editor_background)
            .p_8()
            .child(
                Icon::new(IconName::Image)
                    .size(IconSize::XLarge)
                    .color(Color::Muted),
            )
            .child(Label::new("Custom 360° Horizon & Background").size(LabelSize::Large))
            .child(
                Label::new("Import an equirectangular 360° panorama (e.g. Insta360) to automatically extract your local horizon, align compass North, and use it in planetarium rendering and visibility checks.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("empty_import_btn", "Import 360° Panorama…")
                            .start_icon(Icon::new(IconName::Image).size(IconSize::Small))
                            .style(ButtonStyle::Filled)
                            .disabled(self.is_importing)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_via_dialog(None, window, cx);
                            })),
                    )
                    .child(
                        Button::new("empty_blank_btn", "Create Blank Horizon")
                            .style(ButtonStyle::Outlined)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_blank_background(window, cx);
                            })),
                    ),
            )
    }

    fn render_main_editor(
        &self,
        bg: &HorizonBackground,
        profile: &HorizonProfile,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let cyan: Hsla = rgb(0x42b3c2).into();
        let (min_alt, max_alt) = profile.min_max_altitude_deg();
        let dim_badge = match (bg.image_dimensions, bg.source_filename.as_deref()) {
            (Some((w, h)), Some(file)) => format!("{file} · {w}×{h}"),
            (Some((w, h)), None) => format!("{w}×{h}"),
            (None, _) => "No 360° image (silhouette only)".to_string(),
        };

        let selected_id_for_replace = bg.id.clone();
        let has_image = self.working_panorama.is_some();

        let total_bounds_snapshot = self.canvas_bounds.lock().ok().and_then(|g| *g);
        let ruler_labels: Vec<(f32, &'static str)> = total_bounds_snapshot
            .and_then(|tb| {
                CanvasGeometry::compute(tb, self.zoom, self.view_center_u, self.view_center_v)
            })
            .map(|geo| {
                const TICKS: &[(f32, &str)] = &[
                    (0.0, "S 180°"),
                    (0.125, "SW"),
                    (0.25, "W 270°"),
                    (0.375, "NW"),
                    (0.5, "N 0°"),
                    (0.625, "NE"),
                    (0.75, "E 90°"),
                    (0.875, "SE"),
                    (1.0, "S"),
                ];
                let ruler_x = f32::from(geo.ruler_bounds.origin.x);
                let ruler_w = f32::from(geo.ruler_bounds.size.width);
                let mut out = Vec::new();
                for m in geo.m_min..=geo.m_max {
                    for &(u, label) in TICKS {
                        if u == 1.0 && m < geo.m_max {
                            continue;
                        }
                        let x = geo.world_x(u, m) - ruler_x;
                        if x >= -24.0 && x <= ruler_w + 24.0 {
                            out.push((x, label));
                        }
                    }
                }
                out
            })
            .unwrap_or_default();

        let hover_readout = self.hover_pos.and_then(|pos| {
            let tb = total_bounds_snapshot?;
            let geo = CanvasGeometry::compute(tb, self.zoom, self.view_center_u, self.view_center_v)?;
            if !geo.vp_bounds.contains(&pos) {
                return None;
            }
            let (az_deg, alt_deg) = geo.screen_to_world(pos, false);
            Some(format!(
                "Az {az_deg:5.1}° ({}) · Alt {alt_deg:+.1}°",
                azimuth_cardinal_label(az_deg)
            ))
        });

        let preview_image = self.preview_image.clone();
        let profile_for_paint = profile.clone();
        let mask_mode = self.mask_preview;
        let line_anchor = self.line_anchor;
        let hover_pos = self.hover_pos;
        let zoom = self.zoom;
        let center_u = self.view_center_u;
        let center_v = self.view_center_v;
        let bounds_store = self.canvas_bounds.clone();
        let entity = cx.entity().clone();

        let editor_canvas = gpui::canvas(
            move |bounds, _window, _cx| {
                if let Ok(mut guard) = bounds_store.lock() {
                    *guard = Some(bounds);
                }
            },
            move |bounds, (), window, _cx| {
                let Some(geo) = CanvasGeometry::compute(bounds, zoom, center_u, center_v) else {
                    return;
                };

                // 1. Compass ruler background and tick marks.
                window.paint_quad(fill(geo.ruler_bounds, rgb(0x10141d)));
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: geo.ruler_bounds,
                    }),
                    |window| {
                        let ry = f32::from(geo.ruler_bounds.origin.y);
                        let rh = f32::from(geo.ruler_bounds.size.height);
                        for m in geo.m_min..=geo.m_max {
                            for deg_step in 0..24 {
                                let u = (deg_step as f32) / 24.0;
                                let x = geo.world_x(u, m);
                                let is_cardinal = deg_step % 6 == 0;
                                let is_half = deg_step % 3 == 0;
                                let tick_h = if is_cardinal {
                                    8.0
                                } else if is_half {
                                    5.0
                                } else {
                                    3.0
                                };
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(px(x), px(ry + rh - tick_h)),
                                        size(px(1.0), px(tick_h)),
                                    ),
                                    if is_cardinal {
                                        hsla(0.0, 0.0, 1.0, 0.50)
                                    } else {
                                        hsla(0.0, 0.0, 1.0, 0.25)
                                    },
                                ));
                            }
                        }
                    },
                );

                // 2. Panorama viewport background & horizontally wrapped image.
                window.paint_quad(fill(geo.vp_bounds, rgb(0x060910)));

                let u_shift = world_az_deg_to_disp_u(profile_for_paint.azimuth_offset_deg);
                let shift_px = u_shift * geo.pano_w;

                for m in geo.m_min..=geo.m_max {
                    let span_x = geo.pano_x0 + (m as f32) * geo.pano_w;
                    let span_rect = Bounds::new(
                        point(px(span_x), px(geo.pano_y)),
                        size(px(geo.pano_w), px(geo.pano_h)),
                    );
                    let clip_rect = geo.vp_bounds.intersect(&span_rect);
                    if clip_rect.size.width <= px(0.5) || clip_rect.size.height <= px(0.5) {
                        continue;
                    }

                    if let Some(img) = &preview_image {
                        let primary_rect = Bounds::new(
                            point(px(span_x + shift_px), px(geo.pano_y)),
                            size(px(geo.pano_w), px(geo.pano_h)),
                        );
                        let _ = window.paint_image(
                            clip_rect,
                            primary_rect,
                            Corners::default(),
                            img.clone(),
                            0,
                            false,
                        );
                        if shift_px > 0.5 {
                            let wrap_rect = Bounds::new(
                                point(px(span_x + shift_px - geo.pano_w), px(geo.pano_y)),
                                size(px(geo.pano_w), px(geo.pano_h)),
                            );
                            let _ = window.paint_image(
                                clip_rect,
                                wrap_rect,
                                Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        }
                    } else {
                        window.paint_quad(fill(clip_rect, rgb(0x0c1322)));
                    }
                }

                // 3. Reference grids, mask fill, horizon stroke, line preview, hover crosshair.
                let (x_left, x_right) = geo.draw_x_bounds();
                let draw_clip = geo.vp_bounds.intersect(&Bounds::new(
                    point(px(x_left), px(geo.pano_y)),
                    size(px((x_right - x_left).max(0.0)), px(geo.pano_h)),
                ));

                if draw_clip.size.width > px(1.0) && draw_clip.size.height > px(1.0) {
                    window.with_content_mask(Some(ContentMask { bounds: draw_clip }), |window| {
                        let clip_top = f32::from(draw_clip.origin.y);
                        let clip_bottom = clip_top + f32::from(draw_clip.size.height);

                        // Mask fill polygon.
                        if mask_mode != MaskPreviewMode::LineOnly {
                            const POLY_STEPS: usize = 240;
                            let mut poly = PathBuilder::fill();
                            let anchor_y = match mask_mode {
                                MaskPreviewMode::TintGround => clip_bottom,
                                MaskPreviewMode::DimSky => clip_top,
                                MaskPreviewMode::LineOnly => clip_bottom,
                            };
                            poly.move_to(point(px(x_left), px(anchor_y)));
                            for i in 0..=POLY_STEPS {
                                let t = i as f32 / POLY_STEPS as f32;
                                let x = x_left + t * (x_right - x_left);
                                let u_disp = ((x - geo.pano_x0) / geo.pano_w).rem_euclid(1.0);
                                let az = disp_u_to_world_az_deg(u_disp);
                                let h_alt = profile_for_paint.altitude_at_azimuth_deg(az) as f32;
                                let y = geo.alt_to_y(h_alt);
                                poly.line_to(point(px(x), px(y)));
                            }
                            poly.line_to(point(px(x_right), px(anchor_y)));
                            poly.close();
                            if let Ok(path) = poly.build() {
                                let color: Rgba = match mask_mode {
                                    MaskPreviewMode::TintGround => rgb(0x06101a).alpha(0.45),
                                    MaskPreviewMode::DimSky => rgb(0x03050a).alpha(0.50),
                                    MaskPreviewMode::LineOnly => rgb(0x000000).alpha(0.0),
                                };
                                window.paint_path(path, color);
                            }
                        }

                        // Horizontal altitude lines (+60°, +30°, 0°, -30°, -60°).
                        for deg in [60.0_f32, 30.0, 0.0, -30.0, -60.0] {
                            let y = geo.alt_to_y(deg);
                            let alpha = if deg == 0.0 { 0.42 } else { 0.16 };
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(x_left), px(y)),
                                    size(px(x_right - x_left), px(1.0)),
                                ),
                                hsla(0.0, 0.0, 1.0, alpha),
                            ));
                        }

                        // Vertical cardinal azimuth guides.
                        for m in geo.m_min..=geo.m_max {
                            for k in 0..8 {
                                let u = (k as f32) / 8.0;
                                let x = geo.world_x(u, m);
                                let is_cardinal = k % 2 == 0;
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(px(x), px(clip_top)),
                                        size(px(1.0), px(clip_bottom - clip_top)),
                                    ),
                                    hsla(0.0, 0.0, 1.0, if is_cardinal { 0.20 } else { 0.08 }),
                                ));
                            }
                        }

                        // Dual-stroke horizon curve.
                        const CURVE_STEPS: usize = 360;
                        let mut shadow = PathBuilder::stroke(px(3.6));
                        let mut stroke = PathBuilder::stroke(px(2.0));
                        for i in 0..=CURVE_STEPS {
                            let t = i as f32 / CURVE_STEPS as f32;
                            let x = x_left + t * (x_right - x_left);
                            let u_disp = ((x - geo.pano_x0) / geo.pano_w).rem_euclid(1.0);
                            let az = disp_u_to_world_az_deg(u_disp);
                            let h_alt = profile_for_paint.altitude_at_azimuth_deg(az) as f32;
                            let pt = point(px(x), px(geo.alt_to_y(h_alt)));
                            if i == 0 {
                                shadow.move_to(pt);
                                stroke.move_to(pt);
                            } else {
                                shadow.line_to(pt);
                                stroke.line_to(pt);
                            }
                        }
                        if let Ok(path) = shadow.build() {
                            window.paint_path(path, rgb(0x050810).alpha(0.86));
                        }
                        if let Ok(path) = stroke.build() {
                            window.paint_path(path, rgb(0x45ddf2).alpha(0.96));
                        }

                        // Line-tool anchor and rubber-band preview.
                        if let Some((az0, alt0)) = line_anchor {
                            let u0 = world_az_deg_to_disp_u(az0);
                            let y0 = geo.alt_to_y(alt0);
                            for tile_m in geo.m_min..=geo.m_max {
                                let x0 = geo.world_x(u0, tile_m);
                                paint_dot(window, point(px(x0), px(y0)), 4.5, rgb(0xf6c15a).alpha(0.95));
                            }
                            if let Some(hpos) = hover_pos
                                && geo.vp_bounds.contains(&hpos)
                            {
                                let (az1, alt1) = geo.screen_to_world(hpos, true);
                                let mut delta = (az1 - az0).rem_euclid(360.0);
                                if delta > 180.0 {
                                    delta -= 360.0;
                                }
                                let steps = 32;
                                for tile_m in (geo.m_min - 1)..=(geo.m_max + 1) {
                                    let mut preview = PathBuilder::stroke(px(1.6));
                                    for step in 0..=steps {
                                        let frac = step as f64 / steps as f64;
                                        let u_disp = (f64::from(u0) + (delta / 360.0) * frac) as f32;
                                        let alt = alt0 + (alt1 - alt0) * (frac as f32);
                                        let px_x = geo.world_x(u_disp, tile_m);
                                        let px_y = geo.alt_to_y(alt);
                                        if step == 0 {
                                            preview.move_to(point(px(px_x), px(px_y)));
                                        } else {
                                            preview.line_to(point(px(px_x), px(px_y)));
                                        }
                                    }
                                    if let Ok(path) = preview.build() {
                                        window.paint_path(path, rgb(0xf6c15a).alpha(0.85));
                                    }
                                }
                            }
                        }

                        // Hover crosshair.
                        if let Some(hpos) = hover_pos
                            && geo.vp_bounds.contains(&hpos)
                        {
                            let hx = f32::from(hpos.x);
                            let hy = f32::from(hpos.y);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(hx), px(clip_top)),
                                    size(px(1.0), px(clip_bottom - clip_top)),
                                ),
                                hsla(0.0, 0.0, 1.0, 0.22),
                            ));
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(x_left), px(hy)),
                                    size(px(x_right - x_left), px(1.0)),
                                ),
                                hsla(0.0, 0.0, 1.0, 0.14),
                            ));
                        }
                    });
                }

                // Capture window mouse move & mouse up so dragging never sticks outside canvas.
                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        e.update(cx, |this, cx| {
                            if this.is_drawing || this.is_panning {
                                this.handle_canvas_mouse_move(event, cx);
                            }
                        });
                    }
                });

                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        e.update(cx, |this, cx| {
                            this.handle_canvas_mouse_up(event.button, cx);
                        });
                    }
                });
            },
        )
        .size_full();

        let status_banner: Option<(String, Hsla, Hsla, Hsla)> = self
            .error_message
            .as_ref()
            .map(|msg| {
                (
                    msg.clone(),
                    colors.text,
                    rgb(0xd9534f).alpha(0.22).into(),
                    rgb(0xd9534f).alpha(0.6).into(),
                )
            })
            .or_else(|| {
                self.warning_message.as_ref().map(|msg| {
                    (
                        msg.clone(),
                        colors.text,
                        rgb(0xe5a93b).alpha(0.16).into(),
                        rgb(0xe5a93b).alpha(0.5).into(),
                    )
                })
            })
            .or_else(|| {
                self.status_message.as_ref().map(|msg| {
                    (
                        msg.clone(),
                        colors.text,
                        cyan.opacity(0.16),
                        cyan.opacity(0.5),
                    )
                })
            });

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(colors.editor_background)
            // 1. Metadata & Actions Header
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .bg(colors.panel_background.opacity(0.5))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2p5()
                            .items_center()
                            .child(div().w(px(220.0)).child(self.name_input.clone()))
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(11.0))
                                    .text_color(colors.text_muted)
                                    .truncate()
                                    .child(format!("{dim_badge} · {min_alt:+.1}° to {max_alt:+.1}°")),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Button::new("replace_360_btn", "Replace 360°…")
                                    .start_icon(Icon::new(IconName::Image).size(IconSize::Small))
                                    .style(ButtonStyle::Outlined)
                                    .label_size(LabelSize::Small)
                                    .disabled(self.is_importing)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.import_via_dialog(
                                            Some(selected_id_for_replace.clone()),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                IconButton::new("delete_horizon_btn", IconName::Trash)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Error)
                                    .tooltip(Tooltip::text("Delete background"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.delete_selected_background(window, cx);
                                    })),
                            ),
                    ),
            )
            // Optional Status/Warning/Error Banner
            .when_some(status_banner, |el, (msg, text_c, bg_c, border_c)| {
                el.child(
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_1p5()
                        .justify_between()
                        .items_center()
                        .bg(bg_c)
                        .border_b_1()
                        .border_color(border_c)
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(text_c)
                                .child(msg),
                        )
                        .child(
                            IconButton::new("dismiss_banner_btn", IconName::Close)
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error_message = None;
                                    this.warning_message = None;
                                    this.status_message = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            // 2. Editor Tool Strip
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("tool_draw_btn", "Draw")
                                    .start_icon(
                                        Icon::new(IconName::Pencil).size(IconSize::XSmall),
                                    )
                                    .style(if self.tool_mode == DrawToolMode::Draw {
                                        ButtonStyle::Filled
                                    } else {
                                        ButtonStyle::Subtle
                                    })
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tool_mode = DrawToolMode::Draw;
                                        this.line_anchor = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("tool_line_btn", "Line")
                                    .start_icon(
                                        Icon::new(IconName::ToolPencil).size(IconSize::XSmall),
                                    )
                                    .style(if self.tool_mode == DrawToolMode::Line {
                                        ButtonStyle::Filled
                                    } else {
                                        ButtonStyle::Subtle
                                    })
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tool_mode = DrawToolMode::Line;
                                        cx.notify();
                                    })),
                            )
                            .child(Divider::vertical().color(ui::DividerColor::BorderFaded))
                            .child(
                                Button::new("horizon_smooth_btn", "Smooth")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(mut prof) = this.editing_profile.clone() {
                                            this.push_undo_snapshot();
                                            prof.smooth_circular(2);
                                            this.apply_profile_change(prof, true, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("horizon_shift_down_btn", "-1°")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(mut prof) = this.editing_profile.clone() {
                                            this.push_undo_snapshot();
                                            prof.shift_altitude(-1.0);
                                            this.apply_profile_change(prof, true, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("horizon_shift_up_btn", "+1°")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(mut prof) = this.editing_profile.clone() {
                                            this.push_undo_snapshot();
                                            prof.shift_altitude(1.0);
                                            this.apply_profile_change(prof, true, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("horizon_flatten_btn", "Flatten 0°")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(mut prof) = this.editing_profile.clone() {
                                            this.push_undo_snapshot();
                                            prof.flatten(0.0);
                                            this.apply_profile_change(prof, true, cx);
                                        }
                                    })),
                            )
                            .child(Divider::vertical().color(ui::DividerColor::BorderFaded))
                            .child(render_history_icon_button(
                                "horizon_undo_btn",
                                ICON_UNDO,
                                self.undo_stack.is_empty(),
                                "Undo (Ctrl+Z)",
                                colors.element_hover,
                                colors.text,
                                cx.listener(|this, _, _, cx| {
                                    this.undo(cx);
                                }),
                            ))
                            .child(render_history_icon_button(
                                "horizon_redo_btn",
                                ICON_REDO,
                                self.redo_stack.is_empty(),
                                "Redo (Ctrl+Shift+Z)",
                                colors.element_hover,
                                colors.text,
                                cx.listener(|this, _, _, cx| {
                                    this.redo(cx);
                                }),
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Button::new("mask_mode_btn", self.mask_preview.label())
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mask_preview = this.mask_preview.cycle();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .font_buffer(cx)
                                    .text_size(px(11.0))
                                    .text_color(colors.text_muted)
                                    .child(format!("{:.0}%", self.zoom * 100.0)),
                            )
                            .child(
                                Button::new("zoom_fit_btn", "Fit")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::Small)
                                    .disabled((self.zoom - 1.0).abs() < 1e-3)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = 1.0;
                                        this.view_center_u = 0.5;
                                        this.view_center_v = 0.5;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // 3. Interactive 360° Canvas
            .child(
                div()
                    .id("horizon_editor_canvas_container")
                    .flex_1()
                    .min_h(px(200.0))
                    .relative()
                    .overflow_hidden()
                    .cursor(if self.is_panning {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::Crosshair
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if !this.name_input.read(cx).is_focused(window)
                                && !this.offset_input.read(cx).is_focused(window)
                            {
                                window.focus(&this.focus_handle, cx);
                            }
                            this.handle_canvas_mouse_down(event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.handle_canvas_mouse_down(event, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.handle_canvas_mouse_down(event, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.handle_canvas_mouse_move(event, cx);
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        if !*hovered && this.hover_pos.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                        this.handle_canvas_scroll(event, cx);
                    }))
                    .child(editor_canvas)
                    // Compass labels in top 22px ruler strip
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(RULER_HEIGHT_PX))
                            .overflow_hidden()
                            .children(ruler_labels.into_iter().map(|(x, text)| {
                                div()
                                    .absolute()
                                    .left(px(x + 4.0))
                                    .top(px(2.0))
                                    .font_buffer(cx)
                                    .text_size(px(9.5))
                                    .text_color(colors.text_muted)
                                    .child(text)
                            })),
                    )
                    // Live hover coordinate pill
                    .when_some(hover_readout, |el, text| {
                        el.child(
                            div()
                                .absolute()
                                .bottom_2()
                                .left_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(colors.elevated_surface_background.opacity(0.9))
                                .border_1()
                                .border_color(colors.border)
                                .font_buffer(cx)
                                .text_size(px(10.5))
                                .text_color(colors.text)
                                .child(text),
                        )
                    }),
            )
            // 4. Bottom North Alignment & Auto-Extraction Strip
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .bg(colors.panel_background.opacity(0.65))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new("North Offset")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(div().w(px(150.0)).child(self.offset_slider.clone()))
                            .child(div().w(px(74.0)).child(self.offset_input.clone())),
                    )
                    .when(has_image, |el| {
                        el.child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    Button::new("auto_extract_btn", "Auto-Extract")
                                        .start_icon(
                                            Icon::new(IconName::Sparkle).size(IconSize::XSmall),
                                        )
                                        .style(ButtonStyle::Outlined)
                                        .label_size(LabelSize::Small)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.run_live_extraction(true, cx);
                                        })),
                                )
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(
                                            Label::new("Sky Bias")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                        .child(div().w(px(100.0)).child(self.bias_slider.clone())),
                                )
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(
                                            Label::new("Smoothness")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                        .child(div().w(px(100.0)).child(self.smooth_slider.clone())),
                                ),
                        )
                    }),
            )
            .when(false, |el| {
                // Reference unused `window` parameter cleanly when needed
                let _ = window;
                el
            })
    }
}

impl Render for HorizonManagerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme::theme_settings(cx).ui_font(cx).clone();
        let colors = cx.theme().colors();

        let selected_data = self.selected_id.as_deref().and_then(|id| {
            let horizons = cx.try_global::<GlobalHorizons>()?;
            let bg = horizons.0.read(cx).background(id)?.clone();
            let profile = self.editing_profile.clone().unwrap_or_else(|| {
                HorizonProfile::from_samples(bg.samples_deg.clone(), bg.azimuth_offset_deg)
            });
            Some((bg, profile))
        });

        div()
            .id("horizon-manager-window")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_row()
            .size_full()
            .font(ui_font)
            .bg(colors.background)
            .text_color(colors.text)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let ctrl = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
                if ctrl && key.eq_ignore_ascii_case("z") {
                    if event.keystroke.modifiers.shift {
                        this.redo(cx);
                    } else {
                        this.undo(cx);
                    }
                    cx.stop_propagation();
                } else if ctrl && key.eq_ignore_ascii_case("y") {
                    this.redo(cx);
                    cx.stop_propagation();
                } else if key == "escape" && this.line_anchor.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(self.render_sidebar(cx))
            .child(match selected_data {
                Some((bg, profile)) => self
                    .render_main_editor(&bg, &profile, window, cx)
                    .into_any_element(),
                None => self.render_empty_main(cx).into_any_element(),
            })
    }
}

fn paint_dot(window: &mut Window, center: Point<Pixels>, radius: f32, color: Rgba) {
    const SEGMENTS: usize = 20;
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

fn parse_offset_deg(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().trim_end_matches('°').trim();
    let val: f64 = cleaned.parse().ok()?;
    if !val.is_finite() {
        return None;
    }
    let norm = val.rem_euclid(360.0);
    Some(if norm >= 360.0 { 0.0 } else { norm })
}

fn azimuth_cardinal_label(az_deg: f64) -> &'static str {
    const DIRS: &[&str] = &["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = ((az_deg.rem_euclid(360.0) / 45.0).round() as usize) % DIRS.len();
    DIRS[idx]
}

fn derive_name_from_path(path: &std::path::Path) -> String {
    let raw = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Panorama");
    let cleaned: String = raw
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Panorama".to_string()
    } else {
        trimmed.to_string()
    }
}

fn render_history_icon_button(
    id: &'static str,
    icon_bytes: &'static [u8],
    disabled: bool,
    tooltip: &'static str,
    hover_bg: Hsla,
    icon_color: Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let icon_el = svg()
        .data(icon_bytes)
        .size(px(14.0))
        .flex_none()
        .text_color(if disabled {
            icon_color.opacity(0.38)
        } else {
            icon_color
        });
    div()
        .id(id)
        .w(px(22.0))
        .h(px(22.0))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .tooltip(Tooltip::text(tooltip))
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(move |h| h.bg(hover_bg))
                .on_click(on_click)
        })
        .child(icon_el)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn north_centered_display_mapping_round_trips() {
        assert!((disp_u_to_world_az_deg(0.0) - 180.0).abs() < 1e-6);
        assert!((disp_u_to_world_az_deg(0.25) - 270.0).abs() < 1e-6);
        assert!(disp_u_to_world_az_deg(0.5).abs() < 1e-6);
        assert!((disp_u_to_world_az_deg(0.75) - 90.0).abs() < 1e-6);

        assert!((world_az_deg_to_disp_u(0.0) - 0.5).abs() < 1e-6);
        assert!((world_az_deg_to_disp_u(90.0) - 0.75).abs() < 1e-6);
        assert!(world_az_deg_to_disp_u(180.0).abs() < 1e-6);
        assert!((world_az_deg_to_disp_u(270.0) - 0.25).abs() < 1e-6);
    }
}
