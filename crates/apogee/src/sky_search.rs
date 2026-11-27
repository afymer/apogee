use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use celestial::{
    DailyTargetVisibility, DsoGroup, DsoRecord, HorizonProfile, ObservationContext,
    SolarSystemBody, StarMetadata, compute_solar_system_visibility_with_horizon,
    compute_target_visibility_with_horizon,
    coords::{Angle, cartesian_to_canonical_spherical, topocentric_enu_to_spherical},
    sample_day_equatorial_track, sample_day_solar_system_track,
};
use components::{DoubleSlider, DoubleSliderEvent, Slider, SliderEvent, TextField, TextFieldEvent};
use glam::{DMat3, DVec3};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Corners, Entity, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, KeystrokeEvent, MouseButton, MouseDownEvent, Pixels, Render,
    RenderImage, Rgba, ScrollHandle, Subscription, Task, WeakEntity, Window, div, fill, hsla,
    point, prelude::*, px, rgb,
};
use hifitime::Epoch;
use horizon::GlobalHorizons;
use observer::{GlobalObserver, ObserverModel, paint_twilight_gradient, sample_day_profile};
use planetarium::{
    DsoSpatialIndex, StarSpatialIndex, bp_rp_to_srgb, estimate_teff_kelvin,
    spectral_class_from_bp_rp,
};
use settings::OpenSkySearch;
use ui::{
    ActiveTheme, ButtonCommon, Clickable, Icon, IconButton, IconName, IconSize, StyledTypography,
    Tooltip, h_flex, v_flex,
};
use workspace::{WorkspaceKind, workbench::Workbench};

use crate::star_details::{
    GlobalStarSelection, GlobalVisibilityGraphPrefs, SelectedDso, SelectedObject, SelectedObjectId,
    SelectedStar, VisibilityGraphCenter, alt_to_y_fraction, format_local_hour_hm, paint_disc,
    rasterize_visibility_curve_only,
};

const MAX_RESULTS: usize = 40;
const SLIDER_DEBOUNCE: Duration = Duration::from_millis(35);
const MINI_GRAPH_WIDTH_PX: f32 = 148.0;
const MINI_GRAPH_HEIGHT_PX: f32 = 42.0;
const MINI_GRAPH_PAD_Y: f32 = 3.5;
const MINI_GRAPH_SAMPLES: usize = 65;

const MAG_SLIDER_MIN: f32 = -2.0;
const MAG_SLIDER_MAX: f32 = 16.0;
const ALT_SLIDER_MIN: f32 = -90.0;
const ALT_SLIDER_MAX: f32 = 90.0;
const PEAK_SLIDER_MIN: f32 = 0.0;
const PEAK_SLIDER_MAX: f32 = 90.0;

const SPECTRAL_CLASSES: [&str; 7] = ["O", "B", "A", "F", "G", "K", "M"];

const GREEK_LETTERS: [(&str, &str, &str); 24] = [
    ("α", "alpha", "alf"),
    ("β", "beta", "bet"),
    ("γ", "gamma", "gam"),
    ("δ", "delta", "del"),
    ("ε", "epsilon", "eps"),
    ("ζ", "zeta", "zet"),
    ("η", "eta", "eta"),
    ("θ", "theta", "the"),
    ("ι", "iota", "iot"),
    ("κ", "kappa", "kap"),
    ("λ", "lambda", "lam"),
    ("μ", "mu", "mu"),
    ("ν", "nu", "nu"),
    ("ξ", "xi", "xi"),
    ("ο", "omicron", "omi"),
    ("π", "pi", "pi"),
    ("ρ", "rho", "rho"),
    ("σ", "sigma", "sig"),
    ("τ", "tau", "tau"),
    ("υ", "upsilon", "ups"),
    ("φ", "phi", "phi"),
    ("χ", "chi", "chi"),
    ("ψ", "psi", "psi"),
    ("ω", "omega", "ome"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CategoryFilter {
    #[default]
    All,
    SolarSystem,
    Messier,
    Ngc,
    Stars,
}

impl CategoryFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::SolarSystem,
        Self::Messier,
        Self::Ngc,
        Self::Stars,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::SolarSystem => "Solar System",
            Self::Messier => "Messier",
            Self::Ngc => "NGC",
            Self::Stars => "Stars",
        }
    }

    const fn includes_dsos(self) -> bool {
        matches!(self, Self::All | Self::Messier | Self::Ngc)
    }

    const fn includes_stars(self) -> bool {
        matches!(self, Self::All | Self::Stars)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisibilityFilter {
    #[default]
    Any,
    UpNow,
    Tonight,
}

impl VisibilityFilter {
    const ALL: [Self; 3] = [Self::Any, Self::UpNow, Self::Tonight];

    fn label(self) -> &'static str {
        match self {
            Self::Any => "Any",
            Self::UpNow => "Up now",
            Self::Tonight => "Tonight",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    #[default]
    Relevance,
    Magnitude,
    Altitude,
}

impl SortMode {
    const ALL: [Self; 3] = [Self::Relevance, Self::Magnitude, Self::Altitude];

    fn label(self) -> &'static str {
        match self {
            Self::Relevance => "Relevance",
            Self::Magnitude => "Mag",
            Self::Altitude => "Alt",
        }
    }

    fn default_direction(self) -> SortDirection {
        match self {
            Self::Relevance | Self::Magnitude => SortDirection::Ascending,
            Self::Altitude => SortDirection::Descending,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySearchFilters {
    pub category: CategoryFilter,
    pub visibility: VisibilityFilter,
    pub min_mag: Option<f32>,
    pub max_mag: Option<f32>,
    pub min_alt_deg: Option<f64>,
    pub min_peak_alt_deg: Option<f64>,
    pub spectral_class: Option<&'static str>,
    pub dso_group: Option<DsoGroup>,
    pub sort: SortMode,
    pub sort_direction: SortDirection,
}

impl Default for SkySearchFilters {
    fn default() -> Self {
        Self {
            category: CategoryFilter::All,
            visibility: VisibilityFilter::Any,
            min_mag: None,
            max_mag: None,
            min_alt_deg: None,
            min_peak_alt_deg: None,
            spectral_class: None,
            dso_group: None,
            sort: SortMode::Relevance,
            sort_direction: SortMode::Relevance.default_direction(),
        }
    }
}

impl SkySearchFilters {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn advanced_active_count(&self) -> usize {
        usize::from(self.min_mag.is_some() || self.max_mag.is_some())
            + usize::from(self.min_alt_deg.is_some())
            + usize::from(self.min_peak_alt_deg.is_some())
            + usize::from(self.spectral_class.is_some())
            + usize::from(self.dso_group.is_some())
    }
}

fn mag_from_slider(t: f32) -> f32 {
    let raw = MAG_SLIDER_MIN + t.clamp(0.0, 1.0) * (MAG_SLIDER_MAX - MAG_SLIDER_MIN);
    (raw * 2.0).round() / 2.0
}

fn alt_from_slider(t: f32) -> f64 {
    let raw = ALT_SLIDER_MIN + t.clamp(0.0, 1.0) * (ALT_SLIDER_MAX - ALT_SLIDER_MIN);
    f64::from((raw / 5.0).round() * 5.0)
}

fn peak_from_slider(t: f32) -> f64 {
    let raw = PEAK_SLIDER_MIN + t.clamp(0.0, 1.0) * (PEAK_SLIDER_MAX - PEAK_SLIDER_MIN);
    f64::from((raw / 5.0).round() * 5.0)
}

#[derive(Clone, Debug)]
struct StarSearchNames {
    proper_lower: Option<String>,
    bayer_lower: Option<String>,
    bayer_ascii_lower: Option<String>,
    hip_compact: Option<String>,
    hd_compact: Option<String>,
    hr_compact: Option<String>,
    tyc_compact: Option<String>,
    tyc_lower: Option<String>,
}

impl StarSearchNames {
    fn from_metadata(meta: &StarMetadata) -> Self {
        let proper_lower = meta
            .proper_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        let bayer_lower = meta
            .bayer_flamsteed
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        let bayer_ascii_lower = bayer_lower.as_deref().map(expand_greek_letters);
        let hip_compact = meta.hip.map(|id| format!("hip{id}"));
        let hd_compact = meta.hd.map(|id| format!("hd{id}"));
        let hr_compact = meta.hr.map(|id| format!("hr{id}"));
        let tyc_lower = meta
            .tyc
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("tyc {}", s.to_lowercase()));
        let tyc_compact = meta
            .tyc
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("tyc{}", s.to_lowercase().replace(' ', "")));

        Self {
            proper_lower,
            bayer_lower,
            bayer_ascii_lower,
            hip_compact,
            hd_compact,
            hr_compact,
            tyc_compact,
            tyc_lower,
        }
    }
}

fn expand_greek_letters(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        if let Some((_, full, abbr)) = GREEK_LETTERS.iter().find(|(sym, _, _)| *sym == s) {
            out.push_str(full);
            out.push(' ');
            out.push_str(abbr);
        } else {
            out.push(ch);
        }
    }
    out
}

fn canonicalize_query_token(raw: &str) -> (String, String) {
    let lower = raw.to_lowercase();
    let compact = lower.replace([' ', '-'], "");
    (lower, compact)
}

#[derive(Clone, Debug)]
struct SearchableStar {
    catalog_index: u32,
    ra_deg: f64,
    dec_deg: f64,
    icrs_dir: DVec3,
    mag: f32,
    bp_rp: f32,
    srgb: [f32; 3],
    spectral_class: &'static str,
    estimated_teff_k: u32,
    metadata: Option<StarMetadata>,
    names: Option<Box<StarSearchNames>>,
}

impl SearchableStar {
    fn to_selected_star(&self) -> SelectedStar {
        SelectedStar {
            catalog_index: self.catalog_index,
            ra_deg: self.ra_deg,
            dec_deg: self.dec_deg,
            mag: self.mag,
            bp_rp: self.bp_rp,
            srgb: self.srgb,
            spectral_class: self.spectral_class,
            estimated_teff_k: self.estimated_teff_k,
            metadata: self.metadata.clone(),
        }
    }
}

fn normalize_catalog_query_compact(compact: &str) -> String {
    for prefix in ["messier", "m", "ngc", "ic"] {
        if let Some(rest) = compact.strip_prefix(prefix)
            && rest.starts_with(|c: char| c.is_ascii_digit())
        {
            let trimmed = rest.trim_start_matches('0');
            let out_prefix = if prefix == "messier" { "m" } else { prefix };
            return if trimmed.is_empty() {
                format!("{out_prefix}0")
            } else {
                format!("{out_prefix}{trimmed}")
            };
        }
    }
    compact.to_string()
}

#[derive(Clone, Debug)]
struct DsoSearchNames {
    messier_compact: Option<String>,
    messier_word_lower: Option<String>,
    ngc_compact: Option<String>,
    ic_compact: Option<String>,
    alt_ngc_compacts: Vec<String>,
    common_names_lower: Vec<String>,
    kind_lower: String,
    group_lower: String,
    hubble_lower: Option<String>,
    constellation_lower: Option<String>,
}

impl DsoSearchNames {
    fn from_record(record: &DsoRecord) -> Self {
        let normalize_with_prefix = |prefix: &str, id: &str| -> String {
            let lower = id.trim().to_lowercase().replace([' ', '-'], "");
            if lower.starts_with("ngc") || lower.starts_with("ic") {
                normalize_catalog_query_compact(&lower)
            } else {
                normalize_catalog_query_compact(&format!("{prefix}{lower}"))
            }
        };
        let messier_compact = record.messier.map(|m| format!("m{m}"));
        let messier_word_lower = record.messier.map(|m| format!("messier {m}"));
        let ngc_compact = record
            .ngc
            .as_deref()
            .map(|s| normalize_with_prefix("ngc", s));
        let ic_compact = record.ic.as_deref().map(|s| normalize_with_prefix("ic", s));
        let alt_ngc_compacts = record
            .alt_ngc
            .iter()
            .map(|s| normalize_with_prefix("ngc", s))
            .collect();
        let common_names_lower = record
            .common_names
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        let kind_lower = record.kind.label().to_lowercase();
        let group_lower = record.kind.group().label().to_lowercase();
        let hubble_lower = record.hubble_type.as_deref().map(str::to_lowercase);
        let constellation_lower = record.constellation_str().map(str::to_lowercase);

        Self {
            messier_compact,
            messier_word_lower,
            ngc_compact,
            ic_compact,
            alt_ngc_compacts,
            common_names_lower,
            kind_lower,
            group_lower,
            hubble_lower,
            constellation_lower,
        }
    }
}

#[derive(Clone, Debug)]
struct SearchableDso {
    catalog_index: u32,
    icrs_dir: DVec3,
    known_mag: Option<f32>,
    display_mag: f32,
    record: DsoRecord,
    names: DsoSearchNames,
}

impl SearchableDso {
    fn from_record(catalog_index: u32, record: &DsoRecord) -> Option<Self> {
        let icrs_dir = record.icrs_dir();
        if icrs_dir == DVec3::ZERO {
            return None;
        }
        Some(Self {
            catalog_index,
            icrs_dir,
            known_mag: record.sort_mag(),
            display_mag: record.effective_display_mag(),
            names: DsoSearchNames::from_record(record),
            record: record.clone(),
        })
    }

    fn to_selected_dso(&self) -> SelectedDso {
        SelectedDso {
            catalog_index: self.catalog_index,
            record: self.record.clone(),
        }
    }
}

fn build_searchable_stars(index: &StarSpatialIndex) -> Arc<Vec<SearchableStar>> {
    let mut stars = Vec::new();
    for (catalog_index, record, metadata) in index.iter_stars() {
        let icrs_dir = DVec3::new(
            f64::from(record.pos[0]),
            f64::from(record.pos[1]),
            f64::from(record.pos[2]),
        )
        .normalize_or_zero();
        if icrs_dir == DVec3::ZERO {
            continue;
        }
        let (ra, dec) = cartesian_to_canonical_spherical(icrs_dir);
        let bp_rp = record.color_index();
        let meta_owned = metadata.cloned();
        let names = meta_owned
            .as_ref()
            .map(|m| Box::new(StarSearchNames::from_metadata(m)));
        stars.push(SearchableStar {
            catalog_index,
            ra_deg: ra.degrees(),
            dec_deg: dec.degrees(),
            icrs_dir,
            mag: record.mag(),
            bp_rp,
            srgb: bp_rp_to_srgb(bp_rp),
            spectral_class: spectral_class_from_bp_rp(bp_rp),
            estimated_teff_k: estimate_teff_kelvin(bp_rp),
            metadata: meta_owned,
            names,
        });
    }

    stars.sort_unstable_by(|a, b| {
        a.mag
            .partial_cmp(&b.mag)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.catalog_index.cmp(&b.catalog_index))
    });

    Arc::new(stars)
}

fn build_searchable_dsos(index: &DsoSpatialIndex) -> Arc<Vec<SearchableDso>> {
    let mut dsos = Vec::new();
    for (catalog_index, record) in index.iter_dsos() {
        if let Some(dso) = SearchableDso::from_record(catalog_index, record) {
            dsos.push(dso);
        }
    }

    dsos.sort_unstable_by(|a, b| {
        a.known_mag
            .is_none()
            .cmp(&b.known_mag.is_none())
            .then_with(|| {
                a.display_mag
                    .partial_cmp(&b.display_mag)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.catalog_index.cmp(&b.catalog_index))
    });

    Arc::new(dsos)
}

/// Pre-indexed sky catalog sorted brightest-first for fast interactive search and filtering.
#[derive(Clone, Debug, Default)]
pub struct SkySearchIndex {
    stars: Arc<Vec<SearchableStar>>,
    dsos: Arc<Vec<SearchableDso>>,
}

impl SkySearchIndex {
    #[cfg(test)]
    pub fn from_spatial_index(index: &StarSpatialIndex) -> Self {
        Self {
            stars: build_searchable_stars(index),
            dsos: Arc::default(),
        }
    }
}

#[derive(Clone, Debug)]
struct SearchResultItem {
    object: SelectedObject,
    title: String,
    subtitle: Option<String>,
    category_badge: &'static str,
    mag_label: String,
    current_alt_deg: f64,
    is_visible_now: bool,
    visibility: DailyTargetVisibility,
    alt_profile: Arc<Vec<f64>>,
    horizon_profile: Arc<Vec<f64>>,
    srgb: [f32; 3],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MiniGraphCacheKey {
    target_id: SelectedObjectId,
    day_gen: u64,
    phys_w: u32,
    phys_h: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct SearchDayKey {
    lat_bits: u64,
    lon_bits: u64,
    elev_bits: u64,
    local_midnight_secs: i64,
    horizon_key: Option<(String, u64)>,
}

struct QuerySpec {
    raw_lower: String,
    raw_compact: String,
    tokens: Vec<(String, String)>,
}

impl QuerySpec {
    fn parse(query: &str) -> Option<Self> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (raw_lower, raw_compact) = canonicalize_query_token(trimmed);
        let tokens = trimmed
            .split_whitespace()
            .map(canonicalize_query_token)
            .collect();
        Some(Self {
            raw_lower,
            raw_compact,
            tokens,
        })
    }
}

fn match_solar_system(body: SolarSystemBody, query: &QuerySpec) -> Option<u32> {
    let name = body.name().to_lowercase();
    let kind = body.kind_label().to_lowercase();
    let cat = body.category_label().to_lowercase();

    if query.raw_lower == name {
        return Some(0);
    }
    if name.starts_with(&query.raw_lower) {
        return Some(1);
    }
    let all_match = query.tokens.iter().all(|(tok, _)| {
        name.contains(tok.as_str()) || kind.contains(tok.as_str()) || cat.contains(tok.as_str())
    });
    if all_match { Some(5) } else { None }
}

fn match_star(star: &SearchableStar, query: &QuerySpec) -> Option<u32> {
    let names = star.names.as_deref()?;

    if let Some(proper) = names.proper_lower.as_deref() {
        if proper == query.raw_lower {
            return Some(0);
        }
        if proper.starts_with(&query.raw_lower) {
            return Some(1);
        }
    }

    if let Some(bayer) = names.bayer_lower.as_deref() {
        if bayer == query.raw_lower {
            return Some(1);
        }
        if bayer.starts_with(&query.raw_lower) {
            return Some(2);
        }
    }

    for catalog_id in [
        names.hip_compact.as_deref(),
        names.hd_compact.as_deref(),
        names.hr_compact.as_deref(),
        names.tyc_compact.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if catalog_id == query.raw_compact {
            return Some(1);
        }
    }

    let token_matches = |tok: &str, compact: &str| -> bool {
        if names
            .proper_lower
            .as_deref()
            .is_some_and(|p| p.contains(tok))
        {
            return true;
        }
        if names
            .bayer_lower
            .as_deref()
            .is_some_and(|b| b.contains(tok))
        {
            return true;
        }
        if names
            .bayer_ascii_lower
            .as_deref()
            .is_some_and(|b| b.contains(tok))
        {
            return true;
        }
        if names.tyc_lower.as_deref().is_some_and(|t| t.contains(tok)) {
            return true;
        }
        if !compact.is_empty() {
            for id in [
                names.hip_compact.as_deref(),
                names.hd_compact.as_deref(),
                names.hr_compact.as_deref(),
                names.tyc_compact.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if id.contains(compact) {
                    return true;
                }
            }
        }
        false
    };

    // Also allow matching combined two-word catalog IDs like "hip 32349" against "hip32349".
    let full_compact_match = !query.raw_compact.is_empty()
        && [
            names.hip_compact.as_deref(),
            names.hd_compact.as_deref(),
            names.hr_compact.as_deref(),
            names.tyc_compact.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|id| id.starts_with(&query.raw_compact) || id.contains(&query.raw_compact));

    if full_compact_match {
        return Some(3);
    }

    if !query
        .tokens
        .iter()
        .all(|(tok, compact)| token_matches(tok, compact))
    {
        return None;
    }

    if names
        .proper_lower
        .as_deref()
        .is_some_and(|p| p.contains(&query.raw_lower))
    {
        Some(2)
    } else if names.bayer_lower.is_some() || names.bayer_ascii_lower.is_some() {
        Some(4)
    } else {
        Some(6)
    }
}

fn match_dso(dso: &SearchableDso, query: &QuerySpec) -> Option<u32> {
    let names = &dso.names;
    let raw_cat = normalize_catalog_query_compact(&query.raw_compact);
    let primary_ids = [
        names.messier_compact.as_deref(),
        names.ngc_compact.as_deref(),
        names.ic_compact.as_deref(),
    ];

    if !raw_cat.is_empty() && primary_ids.into_iter().flatten().any(|id| id == raw_cat) {
        return Some(0);
    }
    if names
        .common_names_lower
        .iter()
        .any(|n| n == &query.raw_lower)
    {
        return Some(0);
    }

    if !raw_cat.is_empty() && names.alt_ngc_compacts.iter().any(|id| id == &raw_cat) {
        return Some(1);
    }
    if names
        .common_names_lower
        .iter()
        .any(|n| n.starts_with(&query.raw_lower))
    {
        return Some(1);
    }
    if !raw_cat.is_empty()
        && primary_ids
            .into_iter()
            .flatten()
            .any(|id| id.starts_with(&raw_cat))
    {
        return Some(1);
    }

    if !raw_cat.is_empty()
        && (names
            .alt_ngc_compacts
            .iter()
            .any(|id| id.starts_with(&raw_cat) || id.contains(&raw_cat))
            || primary_ids
                .into_iter()
                .flatten()
                .any(|id| id.contains(&raw_cat)))
    {
        return Some(3);
    }

    let token_matches = |tok: &str, compact: &str| -> bool {
        let tok_cat = normalize_catalog_query_compact(compact);
        if !tok_cat.is_empty()
            && (primary_ids
                .into_iter()
                .flatten()
                .any(|id| id.contains(&tok_cat))
                || names
                    .alt_ngc_compacts
                    .iter()
                    .any(|id| id.contains(&tok_cat)))
        {
            return true;
        }
        if names
            .messier_word_lower
            .as_deref()
            .is_some_and(|m| m.contains(tok))
        {
            return true;
        }
        if names.common_names_lower.iter().any(|n| n.contains(tok)) {
            return true;
        }
        if names.kind_lower.contains(tok) || names.group_lower.contains(tok) {
            return true;
        }
        if names
            .hubble_lower
            .as_deref()
            .is_some_and(|h| h.contains(tok))
        {
            return true;
        }
        names
            .constellation_lower
            .as_deref()
            .is_some_and(|c| c.contains(tok))
    };

    if !query
        .tokens
        .iter()
        .all(|(tok, compact)| token_matches(tok, compact))
    {
        return None;
    }

    if names
        .common_names_lower
        .iter()
        .any(|n| n.contains(&query.raw_lower))
        || names
            .messier_word_lower
            .as_deref()
            .is_some_and(|m| m.contains(&query.raw_lower))
    {
        Some(2)
    } else if names.kind_lower.contains(&query.raw_lower)
        || names.group_lower.contains(&query.raw_lower)
    {
        Some(4)
    } else {
        Some(5)
    }
}

fn dir_az_alt_deg(eq_to_topo: DMat3, icrs_dir: DVec3) -> (f64, f64) {
    let enu = eq_to_topo.mul_vec3(icrs_dir);
    let (az, alt) = topocentric_enu_to_spherical(enu);
    (az.degrees(), alt.degrees())
}

fn is_above_horizon(az_deg: f64, alt_deg: f64, horizon: Option<&HorizonProfile>) -> bool {
    match horizon {
        Some(prof) => prof.is_visible(az_deg, alt_deg),
        None => alt_deg >= 0.0,
    }
}

#[inline]
fn analytical_max_altitude_deg(lat_deg: f64, dec_deg: f64) -> f64 {
    90.0 - (lat_deg - dec_deg).abs()
}

/// Precomputes equatorial-to-topocentric rotation matrices for dark hours (solar alt < -6°),
/// falling back to evenly spaced samples across the 24h day during polar daylight.
fn night_sample_matrices(
    obs_ctx: ObservationContext,
    local_midnight_unix_secs: f64,
    solar_profile: &[f64],
) -> Vec<(Epoch, DMat3)> {
    const SAMPLES: usize = 24;
    let mut night_hours: Vec<f64> = (0..SAMPLES)
        .map(|i| (i as f64 + 0.5) * (24.0 / SAMPLES as f64))
        .filter(|&h| sample_day_profile(solar_profile, h) < -6.0)
        .collect();

    if night_hours.is_empty() {
        night_hours = (0..12).map(|i| (i as f64 + 0.5) * 2.0).collect();
    }

    night_hours
        .into_iter()
        .map(|h| {
            let epoch = Epoch::from_unix_seconds(local_midnight_unix_secs + h * 3600.0);
            let mat =
                ObservationContext::new(obs_ctx.location, epoch).equatorial_to_topocentric_matrix();
            (epoch, mat)
        })
        .collect()
}

#[inline]
fn up_row(mat: DMat3) -> DVec3 {
    DVec3::new(mat.x_axis.z, mat.y_axis.z, mat.z_axis.z)
}

struct FixedVisibilityContext<'a> {
    visibility: VisibilityFilter,
    eq_to_topo: DMat3,
    eq_up_row: DVec3,
    sin_min_alt: Option<f64>,
    need_exact_alt: bool,
    night_mats: &'a [(Epoch, DMat3)],
    night_up_rows: &'a [DVec3],
    horizon: Option<&'a HorizonProfile>,
}

impl FixedVisibilityContext<'_> {
    fn evaluate(&self, icrs_dir: DVec3) -> Option<(f64, bool)> {
        // Fast dot-product altitude check before any trig!
        let up_z = self.eq_up_row.dot(icrs_dir);
        if let Some(min_sin) = self.sin_min_alt
            && up_z < min_sin
        {
            return None;
        }

        let (alt_deg, visible_now) = if self.need_exact_alt {
            let (az_deg, alt_deg) = dir_az_alt_deg(self.eq_to_topo, icrs_dir);
            (alt_deg, is_above_horizon(az_deg, alt_deg, self.horizon))
        } else {
            // Defer asin/to_degrees until after top-K selection.
            (up_z, up_z >= 0.0)
        };

        match self.visibility {
            VisibilityFilter::Any => {}
            VisibilityFilter::UpNow => {
                if !visible_now {
                    return None;
                }
            }
            VisibilityFilter::Tonight => {
                let up_tonight = if let Some(prof) = self.horizon {
                    self.night_mats.iter().any(|(_, mat)| {
                        let (az, alt) = dir_az_alt_deg(*mat, icrs_dir);
                        prof.is_visible(az, alt)
                    })
                } else {
                    self.night_up_rows
                        .iter()
                        .any(|row| row.dot(icrs_dir) >= 0.0)
                };
                if !up_tonight {
                    return None;
                }
            }
        }

        Some((alt_deg, visible_now))
    }
}

#[derive(Clone, Copy)]
enum CandidateRef {
    SolarSystem(SolarSystemBody),
    Star(usize),
    Dso(usize),
}

struct RankedCandidate {
    target: CandidateRef,
    rank_tier: u32,
    mag: f32,
    has_known_mag: bool,
    current_alt_deg: f64,
    is_visible_now: bool,
    has_proper_name: bool,
    has_bayer: bool,
}

fn execute_search(
    catalog: Option<&SkySearchIndex>,
    query_str: &str,
    filters: SkySearchFilters,
    obs_ctx: ObservationContext,
    local_midnight_unix_secs: f64,
    solar_profile: &[f64],
    horizon: Option<&HorizonProfile>,
) -> (Vec<SearchResultItem>, usize) {
    let query = QuerySpec::parse(query_str);
    let eq_to_topo = obs_ctx.equatorial_to_topocentric_matrix();
    let eq_up_row = up_row(eq_to_topo);
    let lat_deg = obs_ctx.location.latitude_deg();
    let sin_min_alt = filters.min_alt_deg.map(|d| d.to_radians().sin());
    let need_exact_alt_during_scan = horizon.is_some() || filters.sort == SortMode::Altitude;
    let night_mats = if filters.visibility == VisibilityFilter::Tonight {
        night_sample_matrices(obs_ctx, local_midnight_unix_secs, solar_profile)
    } else {
        Vec::new()
    };
    let night_up_rows: Vec<DVec3> = night_mats.iter().map(|(_, m)| up_row(*m)).collect();
    let fixed_ctx = FixedVisibilityContext {
        visibility: filters.visibility,
        eq_to_topo,
        eq_up_row,
        sin_min_alt,
        need_exact_alt: need_exact_alt_during_scan,
        night_mats: &night_mats,
        night_up_rows: &night_up_rows,
        horizon,
    };

    let mut candidates: Vec<RankedCandidate> = Vec::new();

    // 1. Solar System bodies
    if matches!(
        filters.category,
        CategoryFilter::All | CategoryFilter::SolarSystem
    ) && filters.spectral_class.is_none()
        && filters.dso_group.is_none()
    {
        for body in SolarSystemBody::ALL {
            let eph = body.ephemeris(obs_ctx.location, obs_ctx.epoch);
            if let Some(min_mag) = filters.min_mag
                && eph.apparent_magnitude < min_mag
            {
                continue;
            }
            if let Some(max_mag) = filters.max_mag
                && eph.apparent_magnitude > max_mag
            {
                continue;
            }
            if let Some(min_alt) = filters.min_alt_deg
                && eph.alt_deg < min_alt
            {
                continue;
            }
            if let Some(min_peak) = filters.min_peak_alt_deg {
                let vis = compute_solar_system_visibility_with_horizon(
                    obs_ctx.location,
                    local_midnight_unix_secs,
                    body,
                    horizon,
                );
                if vis.max_altitude_deg < min_peak {
                    continue;
                }
            }
            let visible_now = is_above_horizon(eph.az_deg, eph.alt_deg, horizon);
            match filters.visibility {
                VisibilityFilter::Any => {}
                VisibilityFilter::UpNow => {
                    if !visible_now {
                        continue;
                    }
                }
                VisibilityFilter::Tonight => {
                    let up_tonight = night_mats.iter().any(|(epoch, _)| {
                        let e = body.ephemeris(obs_ctx.location, *epoch);
                        is_above_horizon(e.az_deg, e.alt_deg, horizon)
                    });
                    if !up_tonight {
                        continue;
                    }
                }
            }

            let rank_tier = match query.as_ref() {
                Some(q) => match match_solar_system(body, q) {
                    Some(tier) => tier,
                    None => continue,
                },
                None => 0,
            };

            candidates.push(RankedCandidate {
                target: CandidateRef::SolarSystem(body),
                rank_tier,
                mag: eph.apparent_magnitude,
                has_known_mag: true,
                current_alt_deg: eph.alt_deg,
                is_visible_now: visible_now,
                has_proper_name: true,
                has_bayer: true,
            });
        }
    }

    // 2. Messier & NGC deep-sky objects
    if filters.category.includes_dsos()
        && filters.spectral_class.is_none()
        && let Some(index) = catalog
    {
        for (dso_idx, dso) in index.dsos.iter().enumerate() {
            match filters.category {
                CategoryFilter::Messier if !dso.record.is_messier() => continue,
                CategoryFilter::Ngc if !dso.record.is_ngc() => continue,
                _ => {}
            }
            if let Some(group) = filters.dso_group
                && dso.record.kind.group() != group
            {
                continue;
            }
            if let Some(min_mag) = filters.min_mag {
                let Some(m) = dso.known_mag else {
                    continue;
                };
                if m < min_mag {
                    continue;
                }
            }
            if let Some(max_mag) = filters.max_mag {
                let Some(m) = dso.known_mag else {
                    continue;
                };
                if m > max_mag {
                    continue;
                }
            }
            if let Some(min_peak) = filters.min_peak_alt_deg
                && analytical_max_altitude_deg(lat_deg, dso.record.dec_deg) < min_peak
            {
                continue;
            }

            let rank_tier = match query.as_ref() {
                Some(q) => match match_dso(dso, q) {
                    Some(tier) => tier,
                    None => continue,
                },
                None => {
                    if dso.record.is_messier() {
                        1
                    } else if dso.record.primary_common_name().is_some() {
                        2
                    } else {
                        3
                    }
                }
            };

            let Some((alt_deg, visible_now)) = fixed_ctx.evaluate(dso.icrs_dir) else {
                continue;
            };

            candidates.push(RankedCandidate {
                target: CandidateRef::Dso(dso_idx),
                rank_tier,
                mag: dso.display_mag,
                has_known_mag: dso.known_mag.is_some(),
                current_alt_deg: alt_deg,
                is_visible_now: visible_now,
                has_proper_name: dso.record.primary_common_name().is_some(),
                has_bayer: dso.record.is_messier(),
            });
        }
    }

    // 3. Catalog stars (zero heap allocations during scan!)
    if filters.category.includes_stars()
        && filters.dso_group.is_none()
        && let Some(index) = catalog
    {
        for (star_idx, star) in index.stars.iter().enumerate() {
            if let Some(max_mag) = filters.max_mag
                && star.mag > max_mag
            {
                // Stars are sorted ascending by magnitude, so we can stop early!
                break;
            }
            if let Some(min_mag) = filters.min_mag
                && star.mag < min_mag
            {
                continue;
            }
            if let Some(sp) = filters.spectral_class
                && star.spectral_class != sp
            {
                continue;
            }
            if let Some(min_peak) = filters.min_peak_alt_deg
                && analytical_max_altitude_deg(lat_deg, star.dec_deg) < min_peak
            {
                continue;
            }

            let rank_tier = match query.as_ref() {
                Some(q) => match match_star(star, q) {
                    Some(tier) => tier,
                    None => continue,
                },
                None => {
                    let has_proper = star
                        .metadata
                        .as_ref()
                        .and_then(|m| m.proper_name.as_deref())
                        .is_some_and(|s| !s.is_empty());
                    let has_bayer = star
                        .metadata
                        .as_ref()
                        .and_then(|m| m.bayer_flamsteed.as_deref())
                        .is_some_and(|s| !s.is_empty());
                    if has_proper {
                        1
                    } else if has_bayer {
                        2
                    } else if star.metadata.is_some() {
                        3
                    } else {
                        4
                    }
                }
            };

            let Some((alt_deg, visible_now)) = fixed_ctx.evaluate(star.icrs_dir) else {
                continue;
            };

            let has_proper_name = star
                .metadata
                .as_ref()
                .and_then(|m| m.proper_name.as_deref())
                .is_some_and(|s| !s.is_empty());
            let has_bayer = star
                .metadata
                .as_ref()
                .and_then(|m| m.bayer_flamsteed.as_deref())
                .is_some_and(|s| !s.is_empty());

            candidates.push(RankedCandidate {
                target: CandidateRef::Star(star_idx),
                rank_tier,
                mag: star.mag,
                has_known_mag: true,
                current_alt_deg: alt_deg,
                is_visible_now: visible_now,
                has_proper_name,
                has_bayer,
            });
        }
    }

    let total_matches = candidates.len();

    let cmp_candidates = |a: &RankedCandidate, b: &RankedCandidate| {
        if filters.sort == SortMode::Magnitude {
            return b.has_known_mag.cmp(&a.has_known_mag).then_with(|| {
                let ord = a
                    .mag
                    .partial_cmp(&b.mag)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.rank_tier.cmp(&b.rank_tier));
                match filters.sort_direction {
                    SortDirection::Ascending => ord,
                    SortDirection::Descending => ord.reverse(),
                }
            });
        }

        let base_ord = match filters.sort {
            SortMode::Relevance => a
                .rank_tier
                .cmp(&b.rank_tier)
                .then_with(|| b.has_proper_name.cmp(&a.has_proper_name))
                .then_with(|| b.has_bayer.cmp(&a.has_bayer))
                .then_with(|| b.has_known_mag.cmp(&a.has_known_mag))
                .then_with(|| a.mag.partial_cmp(&b.mag).unwrap_or(Ordering::Equal)),
            SortMode::Altitude => a
                .current_alt_deg
                .partial_cmp(&b.current_alt_deg)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.mag.partial_cmp(&b.mag).unwrap_or(Ordering::Equal)),
            SortMode::Magnitude => Ordering::Equal,
        };

        match filters.sort_direction {
            SortDirection::Ascending => base_ord,
            SortDirection::Descending => base_ord.reverse(),
        }
    };

    if candidates.len() > MAX_RESULTS {
        candidates.select_nth_unstable_by(MAX_RESULTS, cmp_candidates);
        candidates.truncate(MAX_RESULTS);
    }
    candidates.sort_by(cmp_candidates);

    let decode_alt = |val: f64| {
        if need_exact_alt_during_scan {
            val
        } else {
            val.clamp(-1.0, 1.0).asin().to_degrees()
        }
    };

    let fixed_equatorial = |ra_deg: f64, dec_deg: f64| {
        let ra = Angle::from_degrees(ra_deg);
        let dec = Angle::from_degrees(dec_deg);
        let track = sample_day_equatorial_track(
            obs_ctx.location,
            local_midnight_unix_secs,
            ra,
            dec,
            MINI_GRAPH_SAMPLES,
        );
        let vis = compute_target_visibility_with_horizon(
            obs_ctx.location,
            local_midnight_unix_secs,
            ra,
            dec,
            horizon,
        );
        (track, vis)
    };

    let results = candidates
        .into_iter()
        .filter_map(|cand| {
            let (object, current_alt_deg) = match cand.target {
                CandidateRef::SolarSystem(body) => {
                    (SelectedObject::SolarSystem(body), cand.current_alt_deg)
                }
                CandidateRef::Star(idx) => {
                    let star = catalog?.stars.get(idx)?;
                    (
                        SelectedObject::Star(star.to_selected_star()),
                        decode_alt(cand.current_alt_deg),
                    )
                }
                CandidateRef::Dso(idx) => {
                    let dso = catalog?.dsos.get(idx)?;
                    (
                        SelectedObject::Dso(dso.to_selected_dso()),
                        decode_alt(cand.current_alt_deg),
                    )
                }
            };

            let (title, subtitle, mag_label, track, visibility) = match &object {
                SelectedObject::Star(star) => {
                    let (title, subtitle) = star.display_title_and_subtitle();
                    let (track, vis) = fixed_equatorial(star.ra_deg, star.dec_deg);
                    (title, subtitle, format!("G {:.2}", star.mag), track, vis)
                }
                SelectedObject::Dso(dso) => {
                    let (title, subtitle) = dso.record.display_title_and_subtitle();
                    let (track, vis) = fixed_equatorial(dso.record.ra_deg, dso.record.dec_deg);
                    (title, subtitle, dso.record.mag_label(), track, vis)
                }
                SelectedObject::SolarSystem(body) => {
                    let eph = body.ephemeris(obs_ctx.location, obs_ctx.epoch);
                    let (title, subtitle) = eph.display_title_and_subtitle();
                    let track = sample_day_solar_system_track(
                        obs_ctx.location,
                        local_midnight_unix_secs,
                        *body,
                        MINI_GRAPH_SAMPLES,
                    );
                    let vis = compute_solar_system_visibility_with_horizon(
                        obs_ctx.location,
                        local_midnight_unix_secs,
                        *body,
                        horizon,
                    );
                    (
                        title,
                        Some(subtitle),
                        format!("V {:.1}", eph.apparent_magnitude),
                        track,
                        vis,
                    )
                }
            };

            let alt_profile: Vec<f64> = track.iter().map(|(_, alt)| *alt).collect();
            let horizon_profile: Vec<f64> = if let Some(prof) = horizon {
                track
                    .iter()
                    .map(|(az, _)| prof.altitude_at_azimuth_deg(*az))
                    .collect()
            } else {
                Vec::new()
            };

            let category_badge = object.category_badge();
            let srgb = object.srgb();

            Some(SearchResultItem {
                object,
                title,
                subtitle,
                category_badge,
                mag_label,
                current_alt_deg,
                is_visible_now: cand.is_visible_now,
                visibility,
                alt_profile: Arc::new(alt_profile),
                horizon_profile: Arc::new(horizon_profile),
                srgb,
            })
        })
        .collect();

    (results, total_matches)
}

#[derive(Default)]
pub struct SkyCatalogModel {
    stars: Option<Arc<Vec<SearchableStar>>>,
    dsos: Option<Arc<Vec<SearchableDso>>>,
    stars_building: bool,
    dsos_building: bool,
    index: Option<Arc<SkySearchIndex>>,
}

impl SkyCatalogModel {
    pub fn index(&self) -> Option<&Arc<SkySearchIndex>> {
        self.index.as_ref()
    }

    pub fn is_ready(&self) -> bool {
        self.stars.is_some() && self.dsos.is_some()
    }

    pub fn is_ready_for(&self, category: CategoryFilter) -> bool {
        match category {
            CategoryFilter::SolarSystem => true,
            CategoryFilter::Messier | CategoryFilter::Ngc => self.dsos.is_some(),
            CategoryFilter::Stars => self.stars.is_some(),
            CategoryFilter::All => self.is_ready(),
        }
    }

    fn rebuild_index(&mut self) {
        self.index = Some(Arc::new(SkySearchIndex {
            stars: self.stars.clone().unwrap_or_default(),
            dsos: self.dsos.clone().unwrap_or_default(),
        }));
    }
}

#[derive(Clone)]
pub struct GlobalSkyCatalog(pub Entity<SkyCatalogModel>);

impl gpui::Global for GlobalSkyCatalog {}

#[derive(Clone)]
struct GlobalSkySearchHandle {
    overlay: WeakEntity<SkySearchOverlay>,
    window: AnyWindowHandle,
}

impl gpui::Global for GlobalSkySearchHandle {}

/// Publishes a loaded [`StarSpatialIndex`] to the global sky search catalog, indexing it on a background thread.
pub fn publish_star_index(spatial_index: Arc<StarSpatialIndex>, cx: &mut App) {
    let Some(catalog_entity) = cx.try_global::<GlobalSkyCatalog>().map(|g| g.0.clone()) else {
        return;
    };

    let should_build = catalog_entity.update(cx, |model, _| {
        if model.stars.is_some() || model.stars_building {
            false
        } else {
            model.stars_building = true;
            true
        }
    });
    if !should_build {
        return;
    }

    cx.spawn(async move |cx| {
        let built = cx
            .background_executor()
            .spawn(async move { build_searchable_stars(&spatial_index) })
            .await;
        catalog_entity.update(cx, |model, cx| {
            model.stars = Some(built);
            model.stars_building = false;
            model.rebuild_index();
            cx.notify();
        });
    })
    .detach();
}

/// Publishes a loaded [`DsoSpatialIndex`] to the global sky search catalog, indexing it on a background thread.
pub fn publish_dso_index(dso_index: Arc<DsoSpatialIndex>, cx: &mut App) {
    let Some(catalog_entity) = cx.try_global::<GlobalSkyCatalog>().map(|g| g.0.clone()) else {
        return;
    };

    let should_build = catalog_entity.update(cx, |model, _| {
        if model.dsos.is_some() || model.dsos_building {
            false
        } else {
            model.dsos_building = true;
            true
        }
    });
    if !should_build {
        return;
    }

    cx.spawn(async move |cx| {
        let built = cx
            .background_executor()
            .spawn(async move { build_searchable_dsos(&dso_index) })
            .await;
        catalog_entity.update(cx, |model, cx| {
            model.dsos = Some(built);
            model.dsos_building = false;
            model.rebuild_index();
            cx.notify();
        });
    })
    .detach();
}

pub fn init(cx: &mut App) {
    let catalog = cx.new(|_| SkyCatalogModel::default());
    cx.set_global(GlobalSkyCatalog(catalog));

    cx.on_action(|_: &OpenSkySearch, cx| {
        let Some(handle) = cx.try_global::<GlobalSkySearchHandle>().cloned() else {
            tracing::warn!("OpenSkySearch ignored: GlobalSkySearchHandle not initialized");
            return;
        };
        cx.defer(move |cx| {
            if let Err(err) = handle.window.update(cx, |_, window, cx| {
                if let Some(overlay) = handle.overlay.upgrade() {
                    overlay.update(cx, |this, cx| {
                        this.open(window, cx);
                    });
                }
            }) {
                tracing::error!("Failed to open sky search overlay: {err:?}");
            }
        });
    });

    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        let Some(window) = window else {
            return;
        };
        let workbench_weak = cx.entity().downgrade();
        let overlay = cx.new(|cx| SkySearchOverlay::new(workbench_weak, window, cx));
        cx.set_global(GlobalSkySearchHandle {
            overlay: overlay.downgrade(),
            window: window.window_handle(),
        });
        workbench.add_overlay(overlay.into(), cx);
    })
    .detach();
}

pub struct SkySearchOverlay {
    workbench: WeakEntity<Workbench>,
    is_open: bool,
    filters_expanded: bool,
    query_input: Entity<TextField>,
    mag_slider: Entity<DoubleSlider>,
    min_alt_slider: Entity<Slider>,
    min_peak_slider: Entity<Slider>,
    query_text: String,
    filters: SkySearchFilters,
    results: Vec<SearchResultItem>,
    total_matches: usize,
    selected_index: usize,
    day_key: Option<SearchDayKey>,
    day_gen: u64,
    cached_window_start_secs: f64,
    cached_solar_profile: Arc<Vec<f64>>,
    graph_cache: Arc<Mutex<HashMap<MiniGraphCacheKey, Arc<RenderImage>>>>,
    search_generation: u64,
    search_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for SkySearchOverlay {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SkySearchOverlay {
    pub fn new(
        workbench: WeakEntity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query_input = cx.new(|cx| {
            TextField::new("", window, cx)
                .placeholder("Search Messier, NGC, stars, planets, Bayer (α CMa), HIP, HD, HR…")
                .select_all_on_focus(true)
                .frameless(true)
        });

        let mag_slider = cx.new(|_cx| DoubleSlider::new(0.0, 1.0));
        let min_alt_slider = cx.new(|_cx| Slider::new(0.0).default_value(0.0).center_mark(0.5));
        let min_peak_slider = cx.new(|_cx| Slider::new(0.0).default_value(0.0));

        let mut subscriptions = Vec::new();

        let window_id = window.window_handle().window_id();
        let this_weak = cx.entity().downgrade();
        subscriptions.push(
            cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
                if window.window_handle().window_id() != window_id {
                    return;
                }
                let Some(entity) = this_weak.upgrade() else {
                    return;
                };
                if !entity.read(cx).is_open {
                    return;
                }
                if event.keystroke.modifiers.modified() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        entity.update(cx, |this, cx| this.close(cx));
                    }
                    "up" => {
                        cx.stop_propagation();
                        entity.update(cx, |this, cx| this.move_selection(-1, cx));
                    }
                    "down" => {
                        cx.stop_propagation();
                        entity.update(cx, |this, cx| this.move_selection(1, cx));
                    }
                    "enter" => {
                        cx.stop_propagation();
                        entity.update(cx, |this, cx| this.confirm_selected(cx));
                    }
                    _ => {}
                }
            }),
        );

        subscriptions.push(cx.subscribe_in(
            &query_input,
            window,
            |this, _, event: &TextFieldEvent, _window, cx| match event {
                TextFieldEvent::Change(text) => {
                    this.query_text.clone_from(text);
                    this.selected_index = 0;
                    this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                    this.schedule_search(None, cx);
                    cx.notify();
                }
                TextFieldEvent::Step { direction, .. } => {
                    if *direction > 0 {
                        this.move_selection(-1, cx);
                    } else if *direction < 0 {
                        this.move_selection(1, cx);
                    }
                }
                TextFieldEvent::Submit(_) => {
                    this.confirm_selected(cx);
                }
                TextFieldEvent::Cancel => {
                    this.close(cx);
                }
                TextFieldEvent::Blur(_) => {}
            },
        ));

        subscriptions.push(
            cx.subscribe(
                &mag_slider,
                |this, _, event: &DoubleSliderEvent, cx| match *event {
                    DoubleSliderEvent::Change { min, max } => {
                        let min_m = mag_from_slider(min);
                        let max_m = mag_from_slider(max);
                        this.filters.min_mag = if min <= 0.005 { None } else { Some(min_m) };
                        this.filters.max_mag = if max >= 0.995 { None } else { Some(max_m) };
                        this.selected_index = 0;
                        this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                        this.schedule_search(Some(SLIDER_DEBOUNCE), cx);
                        cx.notify();
                    }
                },
            ),
        );

        subscriptions.push(
            cx.subscribe(&min_alt_slider, |this, _, event: &SliderEvent, cx| {
                let (val, debounce) = match *event {
                    SliderEvent::Change(v) => (v, Some(SLIDER_DEBOUNCE)),
                    SliderEvent::Commit(v) => (v, None),
                };
                let deg = alt_from_slider(val);
                this.filters.min_alt_deg = if val <= 0.005 { None } else { Some(deg) };
                this.selected_index = 0;
                this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                this.schedule_search(debounce, cx);
                cx.notify();
            }),
        );

        subscriptions.push(
            cx.subscribe(&min_peak_slider, |this, _, event: &SliderEvent, cx| {
                let (val, debounce) = match *event {
                    SliderEvent::Change(v) => (v, Some(SLIDER_DEBOUNCE)),
                    SliderEvent::Commit(v) => (v, None),
                };
                let deg = peak_from_slider(val);
                this.filters.min_peak_alt_deg = if val <= 0.005 { None } else { Some(deg) };
                this.selected_index = 0;
                this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                this.schedule_search(debounce, cx);
                cx.notify();
            }),
        );

        if let Some(catalog) = cx.try_global::<GlobalSkyCatalog>().map(|g| g.0.clone()) {
            subscriptions.push(cx.observe(&catalog, |this, _, cx| {
                if this.is_open {
                    this.schedule_search(None, cx);
                    cx.notify();
                }
            }));
        }

        if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.clone()) {
            subscriptions.push(cx.observe(&observer, |this, _, cx| {
                if this.is_open {
                    this.schedule_search(None, cx);
                    cx.notify();
                }
            }));
        }

        if let Some(horizons) = cx.try_global::<GlobalHorizons>().map(|g| g.0.clone()) {
            subscriptions.push(cx.observe(&horizons, |this, _, cx| {
                if this.is_open {
                    this.schedule_search(None, cx);
                    cx.notify();
                }
            }));
        }

        if let Some(prefs) = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.clone())
        {
            subscriptions.push(cx.observe(&prefs, |this, _, cx| {
                if this.is_open {
                    this.schedule_search(None, cx);
                    cx.notify();
                }
            }));
        }

        Self {
            workbench,
            is_open: false,
            filters_expanded: false,
            query_input,
            mag_slider,
            min_alt_slider,
            min_peak_slider,
            query_text: String::new(),
            filters: SkySearchFilters::default(),
            results: Vec::new(),
            total_matches: 0,
            selected_index: 0,
            day_key: None,
            day_gen: 0,
            cached_window_start_secs: 0.0,
            cached_solar_profile: Arc::new(Vec::new()),
            graph_cache: Arc::new(Mutex::new(HashMap::new())),
            search_generation: 0,
            search_task: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_open = true;
        self.schedule_search(None, cx);
        self.query_input.update(cx, |field, cx| {
            field.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.is_open {
            self.is_open = false;
            self.search_task = None;
            cx.notify();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let last = (self.results.len() - 1) as isize;
        let next = (self.selected_index as isize + delta).clamp(0, last) as usize;
        if next != self.selected_index {
            self.selected_index = next;
            self.scroll_handle.scroll_to_item(next);
            cx.notify();
        }
    }

    fn confirm_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.results.get(self.selected_index) {
            let object = item.object.clone();
            self.select_object(object, cx);
        }
    }

    fn select_object(&mut self, object: SelectedObject, cx: &mut Context<Self>) {
        if let Some(workbench) = self.workbench.upgrade() {
            workbench.update(cx, |wb, cx| {
                if wb.active_workspace_kind() != WorkspaceKind::Explore {
                    wb.switch_workspace(WorkspaceKind::Explore, cx);
                }
            });
        }
        if let Some(selection) = cx.try_global::<GlobalStarSelection>().map(|g| g.0.clone()) {
            selection.update(cx, |model, cx| {
                model.select_and_center(object, cx);
            });
        }
        self.close(cx);
    }

    fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query_text.clear();
        self.query_input.update(cx, |field, cx| {
            field.set_text("", cx);
            field.focus(window, cx);
        });
        self.selected_index = 0;
        self.scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.schedule_search(None, cx);
        cx.notify();
    }

    fn reset_filters(&mut self, cx: &mut Context<Self>) {
        self.filters = SkySearchFilters::default();
        self.mag_slider
            .update(cx, |slider, cx| slider.set_range(0.0, 1.0, cx));
        self.min_alt_slider
            .update(cx, |slider, cx| slider.set_value(0.0, cx));
        self.min_peak_slider
            .update(cx, |slider, cx| slider.set_value(0.0, cx));
        self.selected_index = 0;
        self.scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.schedule_search(None, cx);
        cx.notify();
    }

    fn schedule_search(&mut self, debounce: Option<Duration>, cx: &mut Context<Self>) {
        let graph_center = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.read(cx).center())
            .unwrap_or_default();

        let (obs_ctx, window_start_secs, solar_profile, active_horizon, next_day_key) =
            if let Some(observer) = cx.try_global::<GlobalObserver>().map(|g| g.0.read(cx)) {
                let loc = observer.geodetic_position();
                let window_start = graph_center.window_start_unix_seconds(
                    observer.local_midnight_unix_seconds(),
                    observer.local_hour_of_day(),
                );
                let horizon_info = observer.active_horizon_id().and_then(|id| {
                    let horizons = cx.try_global::<GlobalHorizons>()?.0.read(cx);
                    let prof = horizons.profile_for(id)?;
                    Some(((id.to_string(), horizons.live_generation()), prof))
                });
                let horizon_key = horizon_info.as_ref().map(|(k, _)| k.clone());
                let active_profile = horizon_info.map(|(_, p)| p);
                let key = SearchDayKey {
                    lat_bits: loc.latitude_deg().to_bits(),
                    lon_bits: loc.longitude_deg().to_bits(),
                    elev_bits: loc.elevation_m().to_bits(),
                    local_midnight_secs: window_start.round() as i64,
                    horizon_key,
                };
                let solar = if self.day_key.as_ref() == Some(&key)
                    && !self.cached_solar_profile.is_empty()
                {
                    self.cached_solar_profile.clone()
                } else {
                    Arc::new(celestial::solar::sample_day_solar_profile(
                        loc,
                        window_start,
                        MINI_GRAPH_SAMPLES,
                    ))
                };
                (
                    observer.observation_context(),
                    window_start,
                    solar,
                    active_profile,
                    Some(key),
                )
            } else {
                let obs = ObservationContext::default();
                let midnight = (obs.epoch_unix_seconds() / 86_400.0).floor() * 86_400.0;
                let local_hour = (obs.epoch_unix_seconds() - midnight) / 3600.0;
                let window_start = graph_center.window_start_unix_seconds(midnight, local_hour);
                (obs, window_start, Arc::new(Vec::new()), None, None)
            };

        self.cached_window_start_secs = window_start_secs;
        if self.day_key != next_day_key {
            self.day_key = next_day_key;
            self.day_gen = self.day_gen.wrapping_add(1);
            self.cached_solar_profile = solar_profile.clone();
            if let Ok(mut cache) = self.graph_cache.lock() {
                cache.clear();
            }
        }

        let catalog = cx
            .try_global::<GlobalSkyCatalog>()
            .and_then(|g| g.0.read(cx).index().cloned());
        let query = self.query_text.clone();
        let filters = self.filters;
        let solar_for_bg = self.cached_solar_profile.clone();

        self.search_generation = self.search_generation.wrapping_add(1);
        let search_gen = self.search_generation;

        self.search_task = Some(cx.spawn(async move |this, cx| {
            if let Some(delay) = debounce {
                cx.background_executor().timer(delay).await;
            }
            let (results, total_matches) = cx
                .background_executor()
                .spawn(async move {
                    execute_search(
                        catalog.as_deref(),
                        &query,
                        filters,
                        obs_ctx,
                        window_start_secs,
                        &solar_for_bg,
                        active_horizon.as_ref(),
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.search_generation != search_gen {
                    return;
                }
                this.results = results;
                this.total_matches = total_matches;
                if this.results.is_empty() {
                    this.selected_index = 0;
                } else {
                    this.selected_index = this.selected_index.min(this.results.len() - 1);
                }
                cx.notify();
            });
        }));
    }

    fn render_filter_pill(
        id: impl Into<gpui::ElementId>,
        label: String,
        active: bool,
        dot_color: Option<Hsla>,
        tooltip: Option<&'static str>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .id(id)
            .px_2()
            .h(px(22.0))
            .gap_1()
            .items_center()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(11.0))
            .border_1()
            .bg(if active {
                colors.element_selected
            } else {
                colors.element_background.opacity(0.45)
            })
            .border_color(if active {
                colors.text_accent.opacity(0.6)
            } else {
                colors.border_variant
            })
            .text_color(if active {
                colors.text
            } else {
                colors.text_muted
            })
            .hover(|s| s.bg(colors.element_hover).text_color(colors.text))
            .when_some(tooltip, |el, tip| el.tooltip(Tooltip::text(tip)))
            .on_click(cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
            }))
            .when_some(dot_color, |el, color| {
                el.child(div().size(px(6.0)).rounded_full().flex_none().bg(color))
            })
            .child(label)
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let text = cx.theme().colors().text;
        let text_muted = cx.theme().colors().text_muted;
        let panel_background = cx.theme().colors().panel_background;
        let element_background = cx.theme().colors().element_background;
        let element_selected = cx.theme().colors().element_selected;
        let element_hover = cx.theme().colors().element_hover;
        let border_variant = cx.theme().colors().border_variant;
        let text_accent = cx.theme().colors().text_accent;
        let ghost_element_hover = cx.theme().colors().ghost_element_hover;

        let mut category_row = h_flex().items_center().gap_1();
        for cat in CategoryFilter::ALL {
            category_row = category_row.child(Self::render_filter_pill(
                ("sky_search_cat", cat as usize),
                cat.label().to_string(),
                self.filters.category == cat,
                None,
                None,
                move |this, cx| {
                    this.filters.category = cat;
                    if !cat.includes_stars() {
                        this.filters.spectral_class = None;
                    }
                    if !cat.includes_dsos() {
                        this.filters.dso_group = None;
                    }
                    this.selected_index = 0;
                    this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                    this.schedule_search(None, cx);
                    cx.notify();
                },
                cx,
            ));
        }

        let mut vis_row = h_flex().items_center().gap_1();
        for vis in VisibilityFilter::ALL {
            vis_row = vis_row.child(Self::render_filter_pill(
                ("sky_search_vis", vis as usize),
                vis.label().to_string(),
                self.filters.visibility == vis,
                None,
                None,
                move |this, cx| {
                    this.filters.visibility = vis;
                    this.selected_index = 0;
                    this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                    this.schedule_search(None, cx);
                    cx.notify();
                },
                cx,
            ));
        }

        let mut sort_row = h_flex().items_center().gap_1();
        for sort in SortMode::ALL {
            let is_active = self.filters.sort == sort;
            let label = if is_active {
                format!("{} {}", sort.label(), self.filters.sort_direction.glyph())
            } else {
                sort.label().to_string()
            };
            let tip = if is_active {
                Some("Click to reverse sort order")
            } else {
                Some("Sort results (click again to reverse)")
            };
            sort_row = sort_row.child(Self::render_filter_pill(
                ("sky_search_sort", sort as usize),
                label,
                is_active,
                None,
                tip,
                move |this, cx| {
                    if this.filters.sort == sort {
                        this.filters.sort_direction = this.filters.sort_direction.toggle();
                    } else {
                        this.filters.sort = sort;
                        this.filters.sort_direction = sort.default_direction();
                    }
                    this.selected_index = 0;
                    this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                    this.schedule_search(None, cx);
                    cx.notify();
                },
                cx,
            ));
        }

        let center_toggle = observer::graph_center_toggle(
            "sky_search_graph_center_toggle",
            observer::GraphCenterToggleStyle::Inline,
            cx,
        );

        let advanced_count = self.filters.advanced_active_count();
        let filters_label = if advanced_count > 0 {
            format!("Filters ({advanced_count})")
        } else {
            "Filters".to_string()
        };

        let filters_toggle = h_flex()
            .id("sky_search_toggle_filters")
            .px_2()
            .h(px(22.0))
            .gap_1()
            .items_center()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(11.0))
            .border_1()
            .bg(if self.filters_expanded || advanced_count > 0 {
                element_selected
            } else {
                element_background.opacity(0.45)
            })
            .border_color(if self.filters_expanded || advanced_count > 0 {
                text_accent.opacity(0.6)
            } else {
                border_variant
            })
            .text_color(if self.filters_expanded || advanced_count > 0 {
                text
            } else {
                text_muted
            })
            .hover(|s| s.bg(element_hover).text_color(text))
            .tooltip(Tooltip::text(if self.filters_expanded {
                "Collapse advanced filters"
            } else {
                "Expand magnitude, altitude, DSO & spectral filters"
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.filters_expanded = !this.filters_expanded;
                cx.notify();
            }))
            .child(Icon::new(IconName::Filter).size(IconSize::XSmall).color(
                if self.filters_expanded || advanced_count > 0 {
                    ui::Color::Accent
                } else {
                    ui::Color::Muted
                },
            ))
            .child(filters_label)
            .child(
                Icon::new(if self.filters_expanded {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size(IconSize::XSmall)
                .color(ui::Color::Muted),
            );

        let expanded_drawer = if self.filters_expanded {
            let dso_group_tint = |group: DsoGroup| -> Hsla {
                let srgb = group.srgb();
                Rgba {
                    r: srgb[0],
                    g: srgb[1],
                    b: srgb[2],
                    a: 1.0,
                }
                .into()
            };

            let mut dso_type_row = h_flex().items_center().gap_1().flex_wrap().child(
                div()
                    .text_size(px(10.5))
                    .text_color(text_muted)
                    .mr_1()
                    .child("DSO Type"),
            );
            for (i, group) in DsoGroup::ALL.iter().enumerate() {
                let group = *group;
                dso_type_row = dso_type_row.child(Self::render_filter_pill(
                    ("sky_search_dso_group", i),
                    group.label().to_string(),
                    self.filters.dso_group == Some(group),
                    Some(dso_group_tint(group)),
                    None,
                    move |this, cx| {
                        if this.filters.dso_group == Some(group) {
                            this.filters.dso_group = None;
                        } else {
                            this.filters.dso_group = Some(group);
                            this.filters.spectral_class = None;
                            if !this.filters.category.includes_dsos() {
                                this.filters.category = CategoryFilter::All;
                            }
                        }
                        this.selected_index = 0;
                        this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                        this.schedule_search(None, cx);
                        cx.notify();
                    },
                    cx,
                ));
            }

            let spectral_tint = |sp: &str| -> Hsla {
                let bp_rp = match sp {
                    "O" => -0.35,
                    "B" => -0.12,
                    "A" => 0.15,
                    "F" => 0.52,
                    "G" => 0.85,
                    "K" => 1.40,
                    _ => 2.20,
                };
                let srgb = bp_rp_to_srgb(bp_rp);
                Rgba {
                    r: srgb[0],
                    g: srgb[1],
                    b: srgb[2],
                    a: 1.0,
                }
                .into()
            };

            let mut spec_row = h_flex().items_center().gap_1().child(
                div()
                    .text_size(px(10.5))
                    .text_color(text_muted)
                    .mr_1()
                    .child("Spectral"),
            );
            for (i, sp) in SPECTRAL_CLASSES.iter().enumerate() {
                let sp = *sp;
                spec_row = spec_row.child(Self::render_filter_pill(
                    ("sky_search_spec", i),
                    sp.to_string(),
                    self.filters.spectral_class == Some(sp),
                    Some(spectral_tint(sp)),
                    None,
                    move |this, cx| {
                        if this.filters.spectral_class == Some(sp) {
                            this.filters.spectral_class = None;
                        } else {
                            this.filters.spectral_class = Some(sp);
                            this.filters.dso_group = None;
                            if !this.filters.category.includes_stars() {
                                this.filters.category = CategoryFilter::Stars;
                            }
                        }
                        this.selected_index = 0;
                        this.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                        this.schedule_search(None, cx);
                        cx.notify();
                    },
                    cx,
                ));
            }

            let mag_readout = match (self.filters.min_mag, self.filters.max_mag) {
                (None, None) => "Any".to_string(),
                (Some(lo), None) => format!("≥ {lo:.1}"),
                (None, Some(hi)) => format!("≤ {hi:.1}"),
                (Some(lo), Some(hi)) => format!("{lo:.1} .. {hi:.1}"),
            };
            let min_alt_readout = match self.filters.min_alt_deg {
                None => "Any".to_string(),
                Some(deg) => format!("≥ {deg:+.0}°"),
            };
            let min_peak_readout = match self.filters.min_peak_alt_deg {
                None => "Any".to_string(),
                Some(deg) => format!("≥ {deg:+.0}°"),
            };

            let slider_card =
                |label: &'static str, readout: String, active: bool, control: gpui::AnyElement| {
                    v_flex()
                        .flex_1()
                        .min_w(px(170.0))
                        .gap_0p5()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(text_muted)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .font_buffer(cx)
                                        .text_size(px(10.5))
                                        .text_color(if active { text_accent } else { text_muted })
                                        .child(readout),
                                ),
                        )
                        .child(control)
                };

            Some(
                v_flex()
                    .w_full()
                    .pt_2()
                    .mt_1()
                    .gap_2()
                    .border_t_1()
                    .border_color(border_variant.opacity(0.65))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_4()
                            .child(slider_card(
                                "Magnitude",
                                mag_readout,
                                self.filters.min_mag.is_some() || self.filters.max_mag.is_some(),
                                self.mag_slider.clone().into_any_element(),
                            ))
                            .child(slider_card(
                                "Alt now",
                                min_alt_readout,
                                self.filters.min_alt_deg.is_some(),
                                self.min_alt_slider.clone().into_any_element(),
                            ))
                            .child(slider_card(
                                "Peak alt",
                                min_peak_readout,
                                self.filters.min_peak_alt_deg.is_some(),
                                self.min_peak_slider.clone().into_any_element(),
                            )),
                    )
                    .when(self.filters.category.includes_dsos(), |el| {
                        el.child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .child(dso_type_row),
                        )
                    })
                    .when(self.filters.category.includes_stars(), |el| {
                        el.child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .child(spec_row),
                        )
                    }),
            )
        } else {
            None
        };

        v_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_1()
            .bg(panel_background)
            .border_b_1()
            .border_color(border_variant)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2p5()
                            .child(category_row)
                            .child(div().w(px(1.0)).h(px(14.0)).bg(border_variant))
                            .child(vis_row),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(sort_row)
                            .child(div().w(px(1.0)).h(px(14.0)).bg(border_variant))
                            .child(center_toggle)
                            .child(filters_toggle)
                            .when(!self.filters.is_default(), |el| {
                                el.child(
                                    div()
                                        .id("sky_search_reset_filters")
                                        .px_1p5()
                                        .h(px(22.0))
                                        .flex()
                                        .items_center()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(text_accent)
                                        .hover(|s| s.bg(ghost_element_hover))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reset_filters(cx);
                                        }))
                                        .child("Reset"),
                                )
                            }),
                    ),
            )
            .children(expanded_drawer)
            .into_any_element()
    }

    fn render_result_row(
        &self,
        idx: usize,
        item: &SearchResultItem,
        window_offset_hour: f64,
        graph_center: VisibilityGraphCenter,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let is_selected = idx == self.selected_index;
        let is_accent_badge = matches!(
            item.object,
            SelectedObject::SolarSystem(_) | SelectedObject::Dso(_)
        );

        let star_rgb = Rgba {
            r: item.srgb[0],
            g: item.srgb[1],
            b: item.srgb[2],
            a: 1.0,
        };
        let star_color: Hsla = star_rgb.into();

        let vis_summary = if item.visibility.circumpolar {
            format!(
                "Circumpolar · Peak {:+.0}° @ {}",
                item.visibility.max_altitude_deg,
                format_local_hour_hm(graph_center.clock_hour(item.visibility.transit_hour))
            )
        } else if item.visibility.never_rises {
            "Below horizon all day".to_string()
        } else {
            let mut parts = Vec::with_capacity(2);
            if let Some(rise) = item.visibility.rise_hour {
                parts.push(format!(
                    "↑{}",
                    format_local_hour_hm(graph_center.clock_hour(rise))
                ));
            }
            parts.push(format!(
                "Peak {:+.0}° @ {}",
                item.visibility.max_altitude_deg,
                format_local_hour_hm(graph_center.clock_hour(item.visibility.transit_hour))
            ));
            if let Some(set) = item.visibility.set_hour {
                parts.push(format!(
                    "↓{}",
                    format_local_hour_hm(graph_center.clock_hour(set))
                ));
            }
            parts.join(" · ")
        };

        let alt_text = format!("{:+.0}°", item.current_alt_deg);

        let solar_profile = self.cached_solar_profile.clone();
        let alt_profile = item.alt_profile.clone();
        let horizon_profile = item.horizon_profile.clone();
        let is_visible_now = item.is_visible_now;
        let target_id = item.object.id();
        let day_gen = self.day_gen;
        let graph_cache = self.graph_cache.clone();

        let mini_graph_canvas = gpui::canvas(
            |_bounds, _window, _cx| {},
            move |bounds, (), window, _cx| {
                paint_mini_visibility_graph(
                    bounds,
                    window_offset_hour,
                    &solar_profile,
                    &alt_profile,
                    &horizon_profile,
                    is_visible_now,
                    star_rgb,
                    target_id,
                    day_gen,
                    &graph_cache,
                    window,
                );
            },
        )
        .size_full();

        let object_for_click = item.object.clone();

        h_flex()
            .id(("sky_search_row", idx))
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .border_b_1()
            .border_color(colors.border_variant.opacity(0.45))
            .bg(if is_selected {
                colors.element_selected
            } else {
                gpui::transparent_black()
            })
            .hover(|s| {
                if is_selected {
                    s
                } else {
                    s.bg(colors.element_hover.opacity(0.75))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_object(object_for_click.clone(), cx);
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
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
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .py(px(0.5))
                                    .rounded_sm()
                                    .bg(if is_accent_badge {
                                        star_color.opacity(0.18)
                                    } else {
                                        colors.element_background
                                    })
                                    .border_1()
                                    .border_color(if is_accent_badge {
                                        star_color.opacity(0.45)
                                    } else {
                                        colors.border_variant
                                    })
                                    .text_size(px(9.5))
                                    .text_color(if is_accent_badge {
                                        star_color
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(item.category_badge),
                            )
                            .when_some(item.subtitle.clone(), |el, sub| {
                                el.child(
                                    div()
                                        .min_w_0()
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
                            .gap_2()
                            .min_w_0()
                            .font_buffer(cx)
                            .text_size(px(10.5))
                            .child(
                                div()
                                    .text_color(colors.text_muted)
                                    .child(item.mag_label.clone()),
                            )
                            .child(div().text_color(colors.border).child("·"))
                            .child(
                                div()
                                    .text_color(if item.is_visible_now {
                                        star_color
                                    } else {
                                        colors.text_muted
                                    })
                                    .child(format!("Alt {alt_text}")),
                            )
                            .child(div().text_color(colors.border).child("·"))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_color(colors.text_muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(vis_summary),
                            ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(MINI_GRAPH_WIDTH_PX))
                    .h(px(MINI_GRAPH_HEIGHT_PX))
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(if is_selected {
                        colors.text_accent.opacity(0.65)
                    } else {
                        colors.border_variant
                    })
                    .child(mini_graph_canvas)
                    .child(
                        div()
                            .absolute()
                            .top(px(2.0))
                            .right(px(3.0))
                            .px(px(3.5))
                            .py(px(0.5))
                            .rounded_xs()
                            .bg(rgb(0x060a14).alpha(0.55))
                            .font_buffer(cx)
                            .text_size(px(9.0))
                            .text_color(if item.is_visible_now {
                                rgb(0xe8f0ff)
                            } else {
                                rgb(0x9aa6bd)
                            })
                            .child(alt_text),
                    ),
            )
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_mini_visibility_graph(
    bounds: Bounds<Pixels>,
    window_offset_hour: f64,
    solar_profile: &[f64],
    alt_profile: &[f64],
    horizon_profile: &[f64],
    is_visible_now: bool,
    star_rgb: Rgba,
    target_id: SelectedObjectId,
    day_gen: u64,
    graph_cache: &Mutex<HashMap<MiniGraphCacheKey, Arc<RenderImage>>>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 2.0 || height <= 2.0 {
        return;
    }

    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    // 1. Clean 24h twilight sky gradient across the continuous window (no star specks, no horizon).
    paint_twilight_gradient(bounds, solar_profile, window);

    // 2. Cached antialiased visibility curve (grayed when below horizon, colored when visible).
    let scale = window.scale_factor().max(1.0);
    let phys_w = (width * scale).round().max(2.0) as u32;
    let phys_h = (height * scale).round().max(2.0) as u32;
    let key = MiniGraphCacheKey {
        target_id,
        day_gen,
        phys_w,
        phys_h,
    };

    let overlay_img = graph_cache.lock().ok().map(|mut cache| {
        if let Some(img) = cache.get(&key) {
            return img.clone();
        }
        if cache.len() > 256 {
            cache.clear();
        }
        let img = rasterize_visibility_curve_only(
            phys_w,
            phys_h,
            scale,
            MINI_GRAPH_PAD_Y,
            0.82,
            alt_profile,
            horizon_profile,
            star_rgb,
        );
        cache.insert(key, img.clone());
        img
    });
    if let Some(img) = overlay_img {
        let _ = window.paint_image(bounds, bounds, Corners::default(), img, 0, false);
    }

    // 3. Current-time playhead beam & dot (muted gray when not currently visible).
    let t = (window_offset_hour / 24.0).clamp(0.0, 1.0) as f32;
    let playhead_x = origin_x + t * width;
    let current_alt = sample_day_profile(alt_profile, window_offset_hour);
    let dot_y = origin_y + alt_to_y_fraction(current_alt, MINI_GRAPH_PAD_Y, height);
    let dot_pt = point(px(playhead_x), px(dot_y));

    window.paint_quad(fill(
        Bounds::new(
            point(px(playhead_x - 0.5), px(origin_y)),
            gpui::size(px(1.0), px(height)),
        ),
        hsla(0.0, 0.0, 1.0, 0.28),
    ));

    let dot_fill = if is_visible_now {
        star_rgb.alpha(1.0)
    } else {
        rgb(0x8a94a8).alpha(0.85)
    };

    paint_disc(
        window,
        dot_pt,
        2.7,
        rgb(0xf2f6fc).alpha(if is_visible_now { 0.95 } else { 0.55 }),
        0.7,
        rgb(0x090d1a).alpha(0.75),
    );
    paint_disc(window, dot_pt, 1.7, dot_fill, 0.0, rgb(0x090d1a).alpha(0.0));
}

impl Render for SkySearchOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return gpui::Empty.into_any_element();
        }

        let has_query = !self.query_text.is_empty();
        let catalog_ready = cx
            .try_global::<GlobalSkyCatalog>()
            .is_some_and(|g| g.0.read(cx).is_ready_for(self.filters.category));
        let epoch_secs = cx
            .try_global::<GlobalObserver>()
            .map(|g| {
                let model: &ObserverModel = g.0.read(cx);
                model.observation_context().epoch_unix_seconds()
            })
            .unwrap_or(self.cached_window_start_secs);
        let window_offset_hour =
            ((epoch_secs - self.cached_window_start_secs) / 3600.0).clamp(0.0, 24.0);

        let graph_center = cx
            .try_global::<GlobalVisibilityGraphPrefs>()
            .map(|g| g.0.read(cx).center())
            .unwrap_or_default();

        let filters_el = self.render_filters(cx);
        let mut result_rows = Vec::with_capacity(self.results.len());
        for (idx, item) in self.results.iter().enumerate() {
            result_rows.push(self.render_result_row(
                idx,
                item,
                window_offset_hour,
                graph_center,
                cx,
            ));
        }
        let colors = cx.theme().colors();

        let match_summary = if !catalog_ready && self.total_matches == 0 {
            "Indexing sky catalogs…".to_string()
        } else if !catalog_ready {
            format!("{} objects (indexing catalogs…)", self.total_matches)
        } else if self.total_matches == 0 {
            "No matching objects".to_string()
        } else if self.total_matches > self.results.len() {
            format!(
                "Showing {} of {} matches",
                self.results.len(),
                self.total_matches
            )
        } else if self.total_matches == 1 {
            "1 object".to_string()
        } else {
            format!("{} objects", self.total_matches)
        };

        gpui::deferred(
            div()
                .id("sky_search_backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .bg(rgb(0x02040a).alpha(0.55))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(52.0))
                .px_4()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.close(cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(MouseButton::Right, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_move(|_, _, cx| {
                    cx.stop_propagation();
                })
                .on_scroll_wheel(|_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .id("sky_search_modal")
                        .track_focus(&self.focus_handle)
                        .w(px(700.0))
                        .max_w_full()
                        .max_h(px(600.0))
                        .rounded_lg()
                        .overflow_hidden()
                        .bg(colors.elevated_surface_background)
                        .border_1()
                        .border_color(colors.border)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        // 1. Search Input Header
                        .child(
                            h_flex()
                                .w_full()
                                .h(px(44.0))
                                .px_3()
                                .gap_2()
                                .items_center()
                                .bg(colors.editor_background)
                                .border_b_1()
                                .border_color(colors.border)
                                .child(
                                    Icon::new(IconName::MagnifyingGlass)
                                        .size(IconSize::Small)
                                        .color(ui::Color::Muted),
                                )
                                .child(div().flex_1().min_w_0().child(self.query_input.clone()))
                                .when(has_query, |el| {
                                    el.child(
                                        IconButton::new("sky_search_clear_query", IconName::Close)
                                            .icon_size(IconSize::Small)
                                            .icon_color(ui::Color::Muted)
                                            .tooltip(Tooltip::text("Clear search"))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.clear_query(window, cx);
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.5))
                                        .rounded_xs()
                                        .bg(colors.element_background)
                                        .border_1()
                                        .border_color(colors.border_variant)
                                        .font_buffer(cx)
                                        .text_size(px(10.0))
                                        .text_color(colors.text_muted)
                                        .child("Esc"),
                                ),
                        )
                        // 2. Filter Bar (+ expandable sliders drawer)
                        .child(filters_el)
                        // 3. Results List
                        .child(if result_rows.is_empty() {
                            v_flex()
                                .w_full()
                                .h(px(160.0))
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(colors.text_muted)
                                        .child("No sky objects match your search and filters"),
                                )
                                .when(!self.filters.is_default(), |el| {
                                    el.child(
                                        div()
                                            .id("sky_search_empty_reset")
                                            .px_2p5()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .bg(colors.element_background)
                                            .border_1()
                                            .border_color(colors.border_variant)
                                            .text_size(px(11.5))
                                            .text_color(colors.text)
                                            .hover(|s| s.bg(colors.element_hover))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.reset_filters(cx);
                                            }))
                                            .child("Reset filters"),
                                    )
                                })
                                .into_any_element()
                        } else {
                            v_flex()
                                .id("sky_search_results_scroll")
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .max_h(px(380.0))
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll_handle)
                                .children(result_rows)
                                .into_any_element()
                        })
                        // 4. Footer Bar
                        .child(
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_1p5()
                                .items_center()
                                .justify_between()
                                .bg(colors.panel_background)
                                .border_t_1()
                                .border_color(colors.border_variant)
                                .text_size(px(10.5))
                                .text_color(colors.text_muted)
                                .child(match_summary)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2p5()
                                        .font_buffer(cx)
                                        .child("↑↓ Navigate")
                                        .child("↵ Focus & Track")
                                        .child("Esc Close"),
                                ),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use celestial::{StarRecord, coords::geodetic::GeodeticPosition};

    fn sample_catalog() -> SkySearchIndex {
        let records = vec![
            // Sirius (RA 101.29°, Dec -16.72°, mag -1.46, A-type)
            StarRecord::from_icrs_spherical(
                101.287_f64.to_radians(),
                (-16.716_f64).to_radians(),
                -1.46,
                0.01,
            ),
            // Polaris (RA 37.95°, Dec +89.26°, mag 1.98, F-type, circumpolar in Paris)
            StarRecord::from_icrs_spherical(
                37.9546_f64.to_radians(),
                89.2641_f64.to_radians(),
                1.98,
                0.60,
            ),
            // Betelgeuse (RA 88.79°, Dec +7.41°, mag 0.42, M-type)
            StarRecord::from_icrs_spherical(
                88.793_f64.to_radians(),
                7.407_f64.to_radians(),
                0.42,
                2.15,
            ),
            // Acrux (RA 186.65°, Dec -63.10°, mag 0.77, B-type, never rises in Paris)
            StarRecord::from_icrs_spherical(
                186.6496_f64.to_radians(),
                (-63.0991_f64).to_radians(),
                0.77,
                -0.18,
            ),
            // Faint TYC star (mag 7.40, K-type)
            StarRecord::from_icrs_spherical(
                10.0_f64.to_radians(),
                45.0_f64.to_radians(),
                7.40,
                1.30,
            ),
        ];
        let metadata = vec![
            (
                0,
                StarMetadata {
                    proper_name: Some("Sirius".to_string()),
                    bayer_flamsteed: Some("α CMa".to_string()),
                    hip: Some(32349),
                    hd: Some(48915),
                    hr: Some(2491),
                    tyc: None,
                },
            ),
            (
                1,
                StarMetadata {
                    proper_name: Some("Polaris".to_string()),
                    bayer_flamsteed: Some("α UMi".to_string()),
                    hip: Some(11767),
                    hd: Some(8890),
                    hr: Some(424),
                    tyc: None,
                },
            ),
            (
                2,
                StarMetadata {
                    proper_name: Some("Betelgeuse".to_string()),
                    bayer_flamsteed: Some("α Ori".to_string()),
                    hip: Some(27989),
                    hd: Some(39801),
                    hr: Some(2061),
                    tyc: None,
                },
            ),
            (
                3,
                StarMetadata {
                    proper_name: Some("Acrux".to_string()),
                    bayer_flamsteed: Some("α Cru".to_string()),
                    hip: Some(60718),
                    hd: Some(108248),
                    hr: Some(4730),
                    tyc: None,
                },
            ),
            (
                4,
                StarMetadata {
                    tyc: Some("3663-2668-1".to_string()),
                    ..Default::default()
                },
            ),
        ];
        let spatial = StarSpatialIndex::build_with_metadata(&records, metadata);
        SkySearchIndex::from_spatial_index(&spatial)
    }

    fn paris_context() -> (ObservationContext, f64, Vec<f64>) {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        let epoch = Epoch::from_gregorian_utc(2026, 12, 21, 22, 0, 0, 0);
        let midnight = Epoch::from_gregorian_utc(2026, 12, 21, 0, 0, 0, 0).to_unix_seconds();
        let solar = celestial::solar::sample_day_solar_profile(paris, midnight, 65);
        (ObservationContext::new(paris, epoch), midnight, solar)
    }

    #[test]
    fn searches_proper_bayer_greek_ascii_and_catalog_ids() {
        let catalog = sample_catalog();
        let (obs, midnight, solar) = paris_context();
        let default_filters = SkySearchFilters::default();

        for query in [
            "Sirius",
            "sir",
            "α CMa",
            "alpha cma",
            "alf cma",
            "HIP 32349",
            "hip32349",
            "HD 48915",
            "hr2491",
        ] {
            let (results, _) = execute_search(
                Some(&catalog),
                query,
                default_filters,
                obs,
                midnight,
                &solar,
                None,
            );
            assert!(!results.is_empty(), "expected match for query '{query}'");
            assert_eq!(
                results[0].title, "Sirius",
                "query '{query}' should rank Sirius first"
            );
        }

        let (tyc_results, _) = execute_search(
            Some(&catalog),
            "tyc 3663-2668",
            default_filters,
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(tyc_results.len(), 1);
        assert_eq!(tyc_results[0].title, "TYC 3663-2668-1");

        let (moon_results, _) = execute_search(
            Some(&catalog),
            "moon",
            default_filters,
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(moon_results.len(), 1);
        assert_eq!(moon_results[0].title, "Moon");
    }

    #[test]
    fn filters_by_category_magnitude_range_altitude_spectral_and_visibility() {
        let catalog = sample_catalog();
        let (obs, midnight, solar) = paris_context();

        // Category: Solar System only
        let (solar_only, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::SolarSystem,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(solar_only.len(), SolarSystemBody::ALL.len());

        // Spectral class M -> Betelgeuse only
        let (m_stars, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                spectral_class: Some("M"),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(m_stars.len(), 1);
        assert_eq!(m_stars[0].title, "Betelgeuse");

        // Magnitude range [0.0 .. 2.0] includes Betelgeuse (0.42), Acrux (0.77), Polaris (1.98)
        let (mid_bright, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Stars,
                min_mag: Some(0.0),
                max_mag: Some(2.0),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(mid_bright.len(), 3);
        assert!(mid_bright.iter().all(|r| r.title != "Sirius"));
        assert!(mid_bright.iter().all(|r| r.title != "TYC 3663-2668-1"));

        // Min peak altitude >= 80° in Paris (lat 48.9°) excludes Sirius (peak ~24°), Acrux (<0°), Polaris (~49.6°)
        let (high_peak, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Stars,
                min_peak_alt_deg: Some(80.0),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(high_peak.len(), 1);
        assert_eq!(high_peak[0].title, "TYC 3663-2668-1");

        // UpNow from Paris excludes Acrux (never rises) and includes Polaris (circumpolar)
        let (up_now, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Stars,
                visibility: VisibilityFilter::UpNow,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert!(up_now.iter().any(|r| r.title == "Polaris"));
        assert!(up_now.iter().all(|r| r.title != "Acrux"));

        // Tonight with high 60° custom horizon blocks Polaris (~49° in Paris)
        let wall = HorizonProfile::flat(60.0, 360);
        let (blocked, _) = execute_search(
            Some(&catalog),
            "Polaris",
            SkySearchFilters {
                visibility: VisibilityFilter::Tonight,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            Some(&wall),
        );
        assert!(blocked.is_empty());
    }

    #[test]
    fn sorts_are_reversible() {
        let catalog = sample_catalog();
        let (obs, midnight, solar) = paris_context();

        let (highest, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Stars,
                sort: SortMode::Altitude,
                sort_direction: SortDirection::Descending,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        let (lowest, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Stars,
                sort: SortMode::Altitude,
                sort_direction: SortDirection::Ascending,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert!(highest.len() >= 2);
        for pair in highest.windows(2) {
            assert!(pair[0].current_alt_deg >= pair[1].current_alt_deg);
        }
        for pair in lowest.windows(2) {
            assert!(pair[0].current_alt_deg <= pair[1].current_alt_deg);
        }
        assert_eq!(highest[0].title, lowest.last().unwrap().title);
    }

    #[test]
    fn midnight_centered_moon_curve_is_continuous_across_midnight() {
        let paris = GeodeticPosition::from_degrees(48.8566, 2.3522, 35.0);
        // 2026-11-22 18:30 UTC -> Midnight-centered window spans 2026-11-22 12:00 .. 2026-11-23 12:00
        let local_midnight = Epoch::from_gregorian_utc(2026, 11, 22, 0, 0, 0, 0).to_unix_seconds();
        let window_start =
            VisibilityGraphCenter::Midnight.window_start_unix_seconds(local_midnight, 18.5);
        assert!((window_start - (local_midnight + 12.0 * 3600.0)).abs() < 1e-6);

        let track = sample_day_solar_system_track(paris, window_start, SolarSystemBody::Moon, 97);
        // Every adjacent 15-minute step across the entire 24h window (including across midnight at idx 48)
        // must vary smoothly without a day-boundary jump.
        for pair in track.windows(2) {
            let delta_alt = (pair[1].1 - pair[0].1).abs();
            assert!(
                delta_alt < 4.5,
                "unexpected discontinuity across midnight: {} -> {} (delta={delta_alt})",
                pair[0].1,
                pair[1].1
            );
        }
    }

    #[gpui::test]
    fn open_sky_search_escape_and_saturating_arrow_navigation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(theme::LoadThemes::JustBase, cx);
            crate::theme_provider::init(cx);
            crate::star_details::init(cx);
            cx.bind_keys([gpui::KeyBinding::new("ctrl-f", OpenSkySearch, None)]);
            init(cx);
        });

        let window = cx.add_window(Workbench::new);
        let overlay = cx.update(|cx| {
            cx.global::<GlobalSkySearchHandle>()
                .overlay
                .upgrade()
                .expect("overlay should be registered on workbench")
        });

        // Pre-select the Moon so we can verify Escape closes the overlay WITHOUT clearing selection
        cx.update(|cx| {
            let selection = cx.global::<GlobalStarSelection>().0.clone();
            selection.update(cx, |model, cx| {
                model.select_object(SelectedObject::SolarSystem(SolarSystemBody::Moon), cx);
            });
        });

        assert!(!overlay.read_with(cx, |o, _| o.is_open));

        // 1. Dispatching OpenSkySearch from inside the active window (e.g. TitleBar button)
        window
            .update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OpenSkySearch), cx);
            })
            .expect("window update should succeed");
        cx.run_until_parked();
        assert!(overlay.read_with(cx, |o, _| o.is_open));

        // 2. Up/down arrows saturate at first and last elements (all solar-system bodies)
        let last_index = SolarSystemBody::ALL.len() - 1;
        overlay.update(cx, |this, cx| {
            assert_eq!(this.selected_index, 0);
            this.move_selection(-1, cx);
            assert_eq!(this.selected_index, 0, "up arrow should saturate at 0");
            this.move_selection(1, cx);
            assert_eq!(this.selected_index, 1);
            for _ in 0..SolarSystemBody::ALL.len() + 2 {
                this.move_selection(1, cx);
            }
            assert_eq!(
                this.selected_index, last_index,
                "down arrow should saturate at last element"
            );
        });

        // 3. Escape closes the overlay and preserves the active sky object selection
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(
            !overlay.read_with(&vcx, |o, _| o.is_open),
            "Escape must close the search overlay"
        );
        let still_selected = vcx.update(|_window, cx| {
            cx.global::<GlobalStarSelection>()
                .0
                .read(cx)
                .selected_object()
                .is_some()
        });
        assert!(
            still_selected,
            "Escape while closing search overlay must not clear GlobalStarSelection"
        );

        // 4. Reopening via Ctrl+F keystroke
        vcx.simulate_keystrokes("ctrl-f");
        vcx.run_until_parked();

        assert!(
            overlay.read_with(&vcx, |o, _| o.is_open),
            "Ctrl+F keybinding must open the overlay"
        );
    }

    #[derive(Clone, Copy)]
    struct TestDso<'a> {
        ra_deg: f64,
        dec_deg: f64,
        kind: celestial::DsoKind,
        constellation: [u8; 3],
        messier: Option<u16>,
        ngc: Option<&'a str>,
        ic: Option<&'a str>,
        alt_ngc: &'a [&'a str],
        common_names: &'a [&'a str],
        v_mag: Option<f32>,
    }

    fn make_dso(spec: TestDso<'_>) -> DsoRecord {
        DsoRecord {
            ra_deg: spec.ra_deg,
            dec_deg: spec.dec_deg,
            kind: spec.kind,
            constellation: spec.constellation,
            messier: spec.messier,
            ngc: spec.ngc.map(str::to_string),
            ic: spec.ic.map(str::to_string),
            alt_ngc: spec.alt_ngc.iter().map(|s| (*s).to_string()).collect(),
            common_names: spec.common_names.iter().map(|s| (*s).to_string()).collect(),
            v_mag: spec.v_mag,
            b_mag: None,
            surface_brightness: None,
            major_arcmin: Some(30.0),
            minor_arcmin: Some(20.0),
            position_angle_deg: None,
            hubble_type: None,
        }
    }

    fn sample_dso_index() -> DsoSpatialIndex {
        use celestial::DsoKind;
        let records = vec![
            // M31 / NGC 224 — Andromeda Galaxy (Dec +41.27°, circumpolar-high peak in Paris ~82.4°)
            make_dso(TestDso {
                ra_deg: 10.6847,
                dec_deg: 41.2687,
                kind: DsoKind::Galaxy,
                constellation: *b"And",
                messier: Some(31),
                ngc: Some("224"),
                ic: None,
                alt_ngc: &[],
                common_names: &["Andromeda Galaxy"],
                v_mag: Some(3.44),
            }),
            // M1 / NGC 1952 — Crab Nebula
            make_dso(TestDso {
                ra_deg: 83.6331,
                dec_deg: 22.0145,
                kind: DsoKind::SupernovaRemnant,
                constellation: *b"Tau",
                messier: Some(1),
                ngc: Some("1952"),
                ic: None,
                alt_ngc: &[],
                common_names: &["Crab Nebula"],
                v_mag: Some(8.4),
            }),
            // M10 / NGC 6254 — Globular Cluster
            make_dso(TestDso {
                ra_deg: 254.2877,
                dec_deg: -4.1003,
                kind: DsoKind::GlobularCluster,
                constellation: *b"Oph",
                messier: Some(10),
                ngc: Some("6254"),
                ic: None,
                alt_ngc: &[],
                common_names: &[],
                v_mag: Some(6.6),
            }),
            // M101 / NGC 5457 — Pinwheel Galaxy (Dec +54.35°, peak in Paris ~84.5°)
            make_dso(TestDso {
                ra_deg: 210.8024,
                dec_deg: 54.3488,
                kind: DsoKind::Galaxy,
                constellation: *b"UMa",
                messier: Some(101),
                ngc: Some("5457"),
                ic: None,
                alt_ngc: &[],
                common_names: &["Pinwheel Galaxy"],
                v_mag: Some(7.86),
            }),
            // M45 — Messier-only (no NGC ID)
            make_dso(TestDso {
                ra_deg: 56.75,
                dec_deg: 24.1167,
                kind: DsoKind::OpenCluster,
                constellation: *b"Tau",
                messier: Some(45),
                ngc: None,
                ic: None,
                alt_ngc: &[],
                common_names: &["Pleiades"],
                v_mag: Some(1.6),
            }),
            // NGC 7000 — North America Nebula (unknown magnitude, Dec +44.33°)
            make_dso(TestDso {
                ra_deg: 314.68,
                dec_deg: 44.33,
                kind: DsoKind::HiiRegion,
                constellation: *b"Cyg",
                messier: None,
                ngc: Some("7000"),
                ic: None,
                alt_ngc: &[],
                common_names: &["North America Nebula"],
                v_mag: None,
            }),
            // NGC 2237 — Rosette Nebula with alt_ngc ["2244"]
            make_dso(TestDso {
                ra_deg: 98.0,
                dec_deg: 4.95,
                kind: DsoKind::EmissionNebula,
                constellation: *b"Mon",
                messier: None,
                ngc: Some("2237"),
                ic: None,
                alt_ngc: &["2244"],
                common_names: &["Rosette Nebula"],
                v_mag: Some(9.0),
            }),
            // NGC 5139 — Omega Centauri (Dec -47.48°, never rises in Paris)
            make_dso(TestDso {
                ra_deg: 201.697,
                dec_deg: -47.479,
                kind: DsoKind::GlobularCluster,
                constellation: *b"Cen",
                messier: None,
                ngc: Some("5139"),
                ic: None,
                alt_ngc: &[],
                common_names: &["Omega Centauri"],
                v_mag: Some(3.9),
            }),
            // IC 1805 — IC-only Heart Nebula (not Messier, not NGC)
            make_dso(TestDso {
                ra_deg: 38.17,
                dec_deg: 61.45,
                kind: DsoKind::ClusterNebula,
                constellation: *b"Cas",
                messier: None,
                ngc: None,
                ic: Some("1805"),
                alt_ngc: &[],
                common_names: &["Heart Nebula"],
                v_mag: Some(6.5),
            }),
        ];
        DsoSpatialIndex::build(records)
    }

    fn sample_combined_catalog() -> SkySearchIndex {
        let stars = sample_catalog();
        let dsos = sample_dso_index();
        SkySearchIndex {
            stars: stars.stars,
            dsos: build_searchable_dsos(&dsos),
        }
    }

    #[test]
    fn searches_messier_ngc_alt_ngc_and_common_names() {
        let catalog = sample_combined_catalog();
        let (obs, midnight, solar) = paris_context();
        let default_filters = SkySearchFilters::default();

        for (query, expected) in [
            ("M31", "M31"),
            ("m 031", "M31"),
            ("messier 31", "M31"),
            ("NGC 7000", "NGC 7000"),
            ("ngc0224", "M31"),
            ("NGC 2244", "NGC 2237"),
            ("Andromeda", "M31"),
            ("North America", "NGC 7000"),
            ("IC 1805", "IC 1805"),
        ] {
            let (results, _) = execute_search(
                Some(&catalog),
                query,
                default_filters,
                obs,
                midnight,
                &solar,
                None,
            );
            assert!(!results.is_empty(), "expected match for query '{query}'");
            assert_eq!(
                results[0].title, expected,
                "query '{query}' should rank {expected} first"
            );
        }

        // Exact Messier match "M1" must rank strictly ahead of prefix matches "M10" / "M101"
        let (m1_results, _) = execute_search(
            Some(&catalog),
            "M1",
            SkySearchFilters {
                category: CategoryFilter::Messier,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        let titles: Vec<&str> = m1_results.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles.first().copied(), Some("M1"));
        assert!(titles.contains(&"M10"));
        assert!(titles.contains(&"M101"));
    }

    #[test]
    fn filters_and_sorts_dsos_by_category_group_magnitude_and_visibility() {
        let catalog = sample_combined_catalog();
        let (obs, midnight, solar) = paris_context();

        // Category: Messier excludes non-Messier NGCs, IC objects, stars, and solar system bodies
        let (messier_only, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Messier,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(messier_only.len(), 5);
        assert!(
            messier_only
                .iter()
                .all(|r| matches!(r.object, SelectedObject::Dso(ref d) if d.record.is_messier()))
        );

        // Category: NGC includes NGC-designated entries (incl. M31/M1/M10/M101) and excludes M45 & IC 1805
        let (ngc_only, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Ngc,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        let ngc_titles: Vec<&str> = ngc_only.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(ngc_only.len(), 7);
        assert!(!ngc_titles.contains(&"M45"));
        assert!(!ngc_titles.contains(&"IC 1805"));
        assert!(ngc_titles.contains(&"NGC 7000"));
        assert!(ngc_titles.contains(&"NGC 2237"));

        // DSO group: Galaxy returns only galaxies (M31, M101)
        let (galaxies, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                dso_group: Some(DsoGroup::Galaxy),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(galaxies.len(), 2);
        assert!(
            galaxies
                .iter()
                .all(|r| r.title == "M31" || r.title == "M101")
        );

        // DSO group: Nebula returns all 4 nebulae
        let (nebulae, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                dso_group: Some(DsoGroup::Nebula),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(nebulae.len(), 4);

        // Magnitude bounds exclude unknown-magnitude NGC 7000
        let (bounded, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Ngc,
                max_mag: Some(15.0),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert!(bounded.iter().all(|r| r.title != "NGC 7000"));
        assert_eq!(bounded.len(), 6);

        // Magnitude sort places unknown-magnitude NGC 7000 last in BOTH ascending and descending
        let (mag_asc, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Ngc,
                sort: SortMode::Magnitude,
                sort_direction: SortDirection::Ascending,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(mag_asc.first().map(|r| r.title.as_str()), Some("M31"));
        assert_eq!(mag_asc.last().map(|r| r.title.as_str()), Some("NGC 7000"));

        let (mag_desc, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Ngc,
                sort: SortMode::Magnitude,
                sort_direction: SortDirection::Descending,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert_eq!(mag_desc.first().map(|r| r.title.as_str()), Some("NGC 2237"));
        assert_eq!(mag_desc.last().map(|r| r.title.as_str()), Some("NGC 7000"));

        // Min peak altitude >= 75° in Paris excludes M1 (~63°) & Omega Cen (<0°) and keeps high-dec DSOs
        let (high_peak, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Messier,
                min_peak_alt_deg: Some(75.0),
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        let peak_titles: Vec<&str> = high_peak.iter().map(|r| r.title.as_str()).collect();
        assert!(peak_titles.contains(&"M31"));
        assert!(peak_titles.contains(&"M101"));
        assert!(!peak_titles.contains(&"M1"));

        // Tonight visibility in Paris excludes NGC 5139 (Omega Centauri, Dec -47.5°) and keeps M31
        let (tonight_ngc, _) = execute_search(
            Some(&catalog),
            "",
            SkySearchFilters {
                category: CategoryFilter::Ngc,
                visibility: VisibilityFilter::Tonight,
                ..Default::default()
            },
            obs,
            midnight,
            &solar,
            None,
        );
        assert!(tonight_ngc.iter().all(|r| r.title != "NGC 5139"));
        assert!(tonight_ngc.iter().any(|r| r.title == "M31"));
    }
}
