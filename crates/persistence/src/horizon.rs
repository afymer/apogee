use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::PersistenceError;
use crate::store::Persistable;

/// Current schema version for the persisted [`HorizonCatalog`] document.
pub const HORIZON_SCHEMA_VERSION: u32 = 1;

/// Asset namespace under the persistence directory storing managed 360° background images.
pub const HORIZON_ASSET_NAMESPACE: &str = "horizons";

/// Default sample count for newly created or repaired horizon profiles.
pub const DEFAULT_HORIZON_SAMPLE_COUNT: usize = 720;

const MIN_HORIZON_ALT_DEG: f32 = -89.5;
const MAX_HORIZON_ALT_DEG: f32 = 89.5;

fn default_schema_version() -> u32 {
    HORIZON_SCHEMA_VERSION
}

fn default_revision() -> u64 {
    1
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[inline]
fn sanitize_sample(alt_deg: f32) -> f32 {
    if alt_deg.is_finite() {
        alt_deg.clamp(MIN_HORIZON_ALT_DEG, MAX_HORIZON_ALT_DEG)
    } else {
        0.0
    }
}

#[inline]
fn normalize_azimuth_offset(offset_deg: f64) -> f64 {
    if offset_deg.is_finite() {
        let r = offset_deg.rem_euclid(360.0);
        if r >= 360.0 { 0.0 } else { r }
    } else {
        0.0
    }
}

fn sanitize_samples(samples: Vec<f32>) -> Vec<f32> {
    if samples.is_empty() {
        vec![0.0; DEFAULT_HORIZON_SAMPLE_COUNT]
    } else {
        samples.into_iter().map(sanitize_sample).collect()
    }
}

fn normalize_opt_string(opt: &mut Option<String>) {
    if let Some(val) = opt {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            *opt = None;
        } else if trimmed.len() != val.len() {
            *val = trimmed.to_string();
        }
    }
}

/// Custom 360° horizon curve and optional equirectangular background panorama asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonBackground {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_dimensions: Option<(u32, u32)>,
    pub samples_deg: Vec<f32>,
    #[serde(default)]
    pub azimuth_offset_deg: f64,
    #[serde(default = "default_revision")]
    pub revision: u64,
    #[serde(default = "default_revision")]
    pub image_revision: u64,
    #[serde(default)]
    pub updated_at_unix_ms: u64,
}

impl HorizonBackground {
    /// Normalizes name, samples, azimuth offset, and revision invariants in place.
    pub fn normalize(&mut self) {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            self.name = "Horizon".to_string();
        } else if trimmed.len() != self.name.len() {
            self.name = trimmed.to_string();
        }
        normalize_opt_string(&mut self.image_asset);
        normalize_opt_string(&mut self.source_filename);
        if self.samples_deg.is_empty() {
            self.samples_deg = vec![0.0; DEFAULT_HORIZON_SAMPLE_COUNT];
        } else {
            for s in &mut self.samples_deg {
                *s = sanitize_sample(*s);
            }
        }
        self.azimuth_offset_deg = normalize_azimuth_offset(self.azimuth_offset_deg);
        if self.revision == 0 {
            self.revision = 1;
        }
        if self.image_revision == 0 {
            self.image_revision = 1;
        }
    }
}

/// Persisted collection of user-imported 360° horizon backgrounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonCatalog {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub backgrounds: Vec<HorizonBackground>,
}

impl Default for HorizonCatalog {
    fn default() -> Self {
        Self {
            schema_version: HORIZON_SCHEMA_VERSION,
            backgrounds: Vec::new(),
        }
    }
}

impl Persistable for HorizonCatalog {
    const DOCUMENT_KEY: &'static str = "horizons";
    const SCHEMA_VERSION: u32 = HORIZON_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn normalize_after_load(&mut self) {
        self.schema_version = HORIZON_SCHEMA_VERSION;
        for bg in &mut self.backgrounds {
            bg.normalize();
        }
    }
}

impl HorizonCatalog {
    /// Looks up a background by ID.
    pub fn background(&self, id: &str) -> Option<&HorizonBackground> {
        self.backgrounds.iter().find(|b| b.id == id)
    }

    /// Looks up a mutable background by ID.
    pub fn background_mut(&mut self, id: &str) -> Option<&mut HorizonBackground> {
        self.backgrounds.iter_mut().find(|b| b.id == id)
    }

    /// Generates a unique slug ID derived from `name`.
    pub fn unique_id_for(&self, name: &str) -> String {
        let mut slug = String::new();
        let mut prev_dash = false;
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                prev_dash = false;
            } else if !prev_dash && !slug.is_empty() {
                slug.push('-');
                prev_dash = true;
            }
        }
        let base = slug.trim_matches('-');
        let stem = if base.is_empty() { "horizon" } else { base };

        if !self.backgrounds.iter().any(|b| b.id == stem) {
            return stem.to_string();
        }
        let mut suffix: u32 = 2;
        loop {
            let candidate = format!("{stem}-{suffix}");
            if !self.backgrounds.iter().any(|b| b.id == candidate) {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    /// Creates and appends a new background entry, returning a reference to it.
    #[allow(clippy::too_many_arguments)]
    pub fn create_background(
        &mut self,
        id: Option<String>,
        name: &str,
        image_asset: Option<String>,
        source_filename: Option<String>,
        image_dimensions: Option<(u32, u32)>,
        samples_deg: Vec<f32>,
        azimuth_offset_deg: f64,
    ) -> &HorizonBackground {
        let trimmed = name.trim();
        let display_name = if trimmed.is_empty() {
            "New Horizon".to_string()
        } else {
            trimmed.to_string()
        };
        let resolved_id = match id {
            Some(explicit) if !explicit.trim().is_empty() && self.background(&explicit).is_none() => {
                explicit
            }
            _ => self.unique_id_for(&display_name),
        };
        let mut entry = HorizonBackground {
            id: resolved_id,
            name: display_name,
            image_asset,
            source_filename,
            image_dimensions,
            samples_deg: sanitize_samples(samples_deg),
            azimuth_offset_deg: normalize_azimuth_offset(azimuth_offset_deg),
            revision: 1,
            image_revision: 1,
            updated_at_unix_ms: now_unix_ms(),
        };
        entry.normalize();
        self.backgrounds.push(entry);
        self.backgrounds
            .last()
            .expect("backgrounds non-empty after push")
    }

    /// Renames an existing background, bumping its revision and timestamp.
    pub fn rename_background(
        &mut self,
        id: &str,
        name: &str,
    ) -> Result<&HorizonBackground, PersistenceError> {
        let Some(bg) = self.backgrounds.iter_mut().find(|b| b.id == id) else {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        };
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            bg.name = trimmed.to_string();
        }
        bg.revision = bg.revision.saturating_add(1);
        bg.updated_at_unix_ms = now_unix_ms();
        Ok(bg)
    }

    /// Updates horizon curve samples and azimuth offset in memory for responsive live previews
    /// without bumping revision or `image_revision`.
    pub fn set_horizon_in_memory(
        &mut self,
        id: &str,
        samples_deg: Vec<f32>,
        azimuth_offset_deg: f64,
    ) -> Result<&HorizonBackground, PersistenceError> {
        let Some(bg) = self.backgrounds.iter_mut().find(|b| b.id == id) else {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        };
        bg.samples_deg = sanitize_samples(samples_deg);
        bg.azimuth_offset_deg = normalize_azimuth_offset(azimuth_offset_deg);
        Ok(bg)
    }

    /// Commits horizon curve samples and azimuth offset, bumping `revision` (not `image_revision`)
    /// and modification timestamp.
    pub fn commit_horizon(
        &mut self,
        id: &str,
        samples_deg: Vec<f32>,
        azimuth_offset_deg: f64,
    ) -> Result<&HorizonBackground, PersistenceError> {
        let Some(bg) = self.backgrounds.iter_mut().find(|b| b.id == id) else {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        };
        bg.samples_deg = sanitize_samples(samples_deg);
        bg.azimuth_offset_deg = normalize_azimuth_offset(azimuth_offset_deg);
        bg.revision = bg.revision.saturating_add(1);
        bg.updated_at_unix_ms = now_unix_ms();
        Ok(bg)
    }

    /// Replaces the associated 360° image asset metadata (and optionally extracted samples),
    /// bumping both `revision` and `image_revision` and returning the previous `image_asset` name.
    pub fn replace_image(
        &mut self,
        id: &str,
        image_asset: Option<String>,
        source_filename: Option<String>,
        image_dimensions: Option<(u32, u32)>,
        samples_deg: Option<Vec<f32>>,
    ) -> Result<(Option<String>, &HorizonBackground), PersistenceError> {
        let Some(bg) = self.backgrounds.iter_mut().find(|b| b.id == id) else {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        };
        let previous_asset = bg.image_asset.take();
        bg.image_asset = image_asset;
        bg.source_filename = source_filename;
        bg.image_dimensions = image_dimensions;
        if let Some(samples) = samples_deg {
            bg.samples_deg = sanitize_samples(samples);
        }
        bg.image_revision = bg.image_revision.saturating_add(1);
        bg.revision = bg.revision.saturating_add(1);
        bg.updated_at_unix_ms = now_unix_ms();
        bg.normalize();
        Ok((previous_asset, bg))
    }

    /// Deletes a background by ID, returning its `image_asset` if one was linked.
    pub fn delete_background(&mut self, id: &str) -> Result<Option<String>, PersistenceError> {
        let Some(idx) = self.backgrounds.iter().position(|b| b.id == id) else {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        };
        let removed = self.backgrounds.remove(idx);
        Ok(removed.image_asset)
    }
}
