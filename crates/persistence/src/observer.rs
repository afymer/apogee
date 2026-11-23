use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::PersistenceError;
use crate::store::Persistable;

/// Current schema version for the persisted [`ObserverCatalog`] document.
pub const OBSERVER_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    OBSERVER_SCHEMA_VERSION
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

/// Geographic coordinate axis for parsing and validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinateAxis {
    Latitude,
    Longitude,
}

/// Geodetic coordinates (WGS84) persisted with an observatory profile.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoLocation {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub elevation_m: f64,
}

impl GeoLocation {
    pub const MIN_LAT_DEG: f64 = -90.0;
    pub const MAX_LAT_DEG: f64 = 90.0;
    pub const MIN_LON_DEG: f64 = -180.0;
    pub const MAX_LON_DEG: f64 = 180.0;
    pub const MIN_ELEV_M: f64 = -500.0;
    pub const MAX_ELEV_M: f64 = 9000.0;

    /// Constructs a sanitized [`GeoLocation`].
    pub fn new(latitude_deg: f64, longitude_deg: f64, elevation_m: f64) -> Self {
        Self {
            latitude_deg,
            longitude_deg,
            elevation_m,
        }
        .sanitized()
    }

    /// Replaces non-finite values with `0.0` and clamps all fields to valid bounds.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let lat = if self.latitude_deg.is_finite() {
            self.latitude_deg
                .clamp(Self::MIN_LAT_DEG, Self::MAX_LAT_DEG)
        } else {
            0.0
        };
        let lon = if self.longitude_deg.is_finite() {
            self.longitude_deg
                .clamp(Self::MIN_LON_DEG, Self::MAX_LON_DEG)
        } else {
            0.0
        };
        let elev = if self.elevation_m.is_finite() {
            self.elevation_m.clamp(Self::MIN_ELEV_M, Self::MAX_ELEV_M)
        } else {
            0.0
        };
        Self {
            latitude_deg: lat,
            longitude_deg: lon,
            elevation_m: elev,
        }
    }

    /// Formats an angle in degrees as DMS with hemisphere suffix (e.g. `48°50'11" N`).
    pub fn format_dms(degrees: f64, pos_hemi: char, neg_hemi: char) -> String {
        let safe_deg = if degrees.is_finite() { degrees } else { 0.0 };
        let total_arcsec = (safe_deg.abs() * 3600.0).round() as u64;
        let hemi = if safe_deg.is_sign_negative() && total_arcsec > 0 {
            neg_hemi
        } else {
            pos_hemi
        };
        let d = total_arcsec / 3600;
        let m = (total_arcsec % 3600) / 60;
        let s = total_arcsec % 60;
        format!("{d}°{m:02}'{s:02}\" {hemi}")
    }

    /// Formats latitude as DMS (e.g. `48°50'11" N`).
    pub fn format_latitude_dms(&self) -> String {
        Self::format_dms(self.latitude_deg, 'N', 'S')
    }

    /// Formats longitude as DMS (e.g. `2°20'11" E`).
    pub fn format_longitude_dms(&self) -> String {
        Self::format_dms(self.longitude_deg, 'E', 'W')
    }

    /// Formats a compact one-line summary (`48°50'11" N · 2°20'11" E · 67 m`).
    pub fn format_summary_dms(&self) -> String {
        format!(
            "{} · {} · {:.0} m",
            self.format_latitude_dms(),
            self.format_longitude_dms(),
            self.elevation_m.round()
        )
    }

    /// Formats latitude as decimal degrees with hemisphere (e.g. `48.8364° N`).
    pub fn format_latitude_decimal(&self) -> String {
        let hemi = if self.latitude_deg < 0.0 { 'S' } else { 'N' };
        format!("{:.4}° {}", self.latitude_deg.abs(), hemi)
    }

    /// Formats longitude as decimal degrees with hemisphere (e.g. `2.3364° E`).
    pub fn format_longitude_decimal(&self) -> String {
        let hemi = if self.longitude_deg < 0.0 { 'W' } else { 'E' };
        format!("{:.4}° {}", self.longitude_deg.abs(), hemi)
    }

    /// Returns the standard (non-DST) civil UTC offset in minutes for this site's coordinates.
    pub fn standard_utc_offset_minutes(&self) -> i32 {
        crate::tz_offset::lookup_standard_utc_offset_minutes(self.latitude_deg, self.longitude_deg)
    }
}

impl Default for GeoLocation {
    fn default() -> Self {
        Self {
            latitude_deg: 48.8364,
            longitude_deg: 2.3364,
            elevation_m: 67.0,
        }
    }
}

fn hemisphere_sign(ch: char, axis: CoordinateAxis) -> Option<f64> {
    match (axis, ch.to_ascii_uppercase()) {
        (CoordinateAxis::Latitude, 'N') | (CoordinateAxis::Longitude, 'E') => Some(1.0),
        (CoordinateAxis::Latitude, 'S') | (CoordinateAxis::Longitude, 'W') => Some(-1.0),
        _ => None,
    }
}

fn is_hemisphere_letter(ch: char) -> bool {
    matches!(ch.to_ascii_uppercase(), 'N' | 'S' | 'E' | 'W')
}

/// Parses either decimal degrees (e.g. `48.8364`, `-73.98`, `48.8364° N`, `W 2.33`)
/// or DMS (`48° 50' 11" N`, `48d50m11s`, `-122 25 10`).
pub fn parse_coordinate(input: &str, axis: CoordinateAxis) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut body = trimmed;
    let mut hemi_char: Option<char> = None;

    if let Some(first) = body.chars().next() {
        if is_hemisphere_letter(first) {
            let rest = &body[first.len_utf8()..];
            if rest
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '+' || c == '-' || c.is_ascii_digit())
            {
                hemi_char = Some(first);
                body = rest.trim_start();
            }
        }
    }

    if hemi_char.is_none() {
        if let Some(last) = body.chars().next_back() {
            if is_hemisphere_letter(last) {
                let prefix = &body[..body.len() - last.len_utf8()];
                let prev = prefix.chars().next_back();
                let is_seconds_unit = matches!(last, 's' | 'S')
                    && prev.is_some_and(|c| c.is_ascii_digit() || c == '.')
                    && prefix.chars().any(|c| matches!(c, 'd' | 'D' | 'm' | 'M'));
                if !is_seconds_unit && !prefix.trim().is_empty() {
                    hemi_char = Some(last);
                    body = prefix.trim_end();
                }
            }
        }
    }

    let explicit_hemi = match hemi_char {
        Some(ch) => Some(hemisphere_sign(ch, axis)?),
        None => None,
    };

    let (sign, rest) = if let Some(after) = body.strip_prefix('-') {
        if explicit_hemi.is_some() {
            return None;
        }
        (-1.0, after.trim_start())
    } else if let Some(after) = body.strip_prefix('+') {
        (explicit_hemi.unwrap_or(1.0), after.trim_start())
    } else {
        (explicit_hemi.unwrap_or(1.0), body)
    };

    if rest.is_empty() || rest.starts_with('+') || rest.starts_with('-') {
        return None;
    }

    let normalized: String = rest
        .chars()
        .map(|c| match c {
            '°' | '\'' | '"' | '′' | '″' | 'd' | 'D' | 'm' | 'M' | 's' | 'S' => ' ',
            _ => c,
        })
        .collect();

    let parts: Vec<&str> = normalized.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }

    let mut nums = [0.0_f64; 3];
    for (idx, part) in parts.iter().enumerate() {
        if part.starts_with('+') || part.starts_with('-') {
            return None;
        }
        let val: f64 = part.parse().ok()?;
        if !val.is_finite() || val < 0.0 {
            return None;
        }
        nums[idx] = val;
    }

    let magnitude = match parts.len() {
        1 => nums[0],
        2 => {
            if nums[0].fract() != 0.0 || nums[1] >= 60.0 {
                return None;
            }
            nums[0] + nums[1] / 60.0
        }
        3 => {
            if nums[0].fract() != 0.0
                || nums[1].fract() != 0.0
                || nums[1] >= 60.0
                || nums[2] >= 60.0
            {
                return None;
            }
            nums[0] + nums[1] / 60.0 + nums[2] / 3600.0
        }
        _ => return None,
    };

    let signed = sign * magnitude;
    let valid_range = match axis {
        CoordinateAxis::Latitude => {
            (GeoLocation::MIN_LAT_DEG..=GeoLocation::MAX_LAT_DEG).contains(&signed)
        }
        CoordinateAxis::Longitude => {
            (GeoLocation::MIN_LON_DEG..=GeoLocation::MAX_LON_DEG).contains(&signed)
        }
    };
    if valid_range { Some(signed) } else { None }
}

/// Parses an elevation string in meters (default / `m`), feet (`ft`/`feet`), or kilometers (`km`).
pub fn parse_elevation_m(input: &str) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let (num_str, factor) = if let Some(prefix) = lower.strip_suffix("feet") {
        (&trimmed[..prefix.len()], 0.3048)
    } else if let Some(prefix) = lower.strip_suffix("ft") {
        (&trimmed[..prefix.len()], 0.3048)
    } else if let Some(prefix) = lower.strip_suffix("km") {
        (&trimmed[..prefix.len()], 1000.0)
    } else if let Some(prefix) = lower.strip_suffix('m') {
        (&trimmed[..prefix.len()], 1.0)
    } else {
        (trimmed, 1.0)
    };

    let raw: f64 = num_str.trim().parse().ok()?;
    let meters = raw * factor;
    if !meters.is_finite() || !(GeoLocation::MIN_ELEV_M..=GeoLocation::MAX_ELEV_M).contains(&meters)
    {
        return None;
    }
    Some(meters)
}

/// Visual icon badge associated with an observatory profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservatoryIcon {
    #[default]
    Dome,
    Field,
    Mountain,
    Rooftop,
    Balcony,
    Remote,
}

impl ObservatoryIcon {
    /// Ordered list of all available observatory icon variants.
    pub const ALL: [Self; 6] = [
        Self::Dome,
        Self::Field,
        Self::Mountain,
        Self::Rooftop,
        Self::Balcony,
        Self::Remote,
    ];

    /// Human-readable short label for UI icon pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dome => "Dome",
            Self::Field => "Field",
            Self::Mountain => "Mountain",
            Self::Rooftop => "Rooftop",
            Self::Balcony => "Balcony",
            Self::Remote => "Remote",
        }
    }
}

/// Origin/synchronization metadata for an [`ObservatoryProfile`].
///
/// Tracks whether a site is locally managed or synchronized from a remote
/// Apogee observatory daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileSource {
    #[default]
    Local,
    Remote {
        endpoint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_synced_unix_ms: Option<u64>,
    },
}

impl ProfileSource {
    /// Returns `true` if this profile originates from a remote observatory daemon.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Returns the remote daemon endpoint when [`Self::Remote`].
    pub fn endpoint(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Remote { endpoint, .. } => Some(endpoint.as_str()),
        }
    }
}

/// Basis used to determine an observatory profile's local timezone offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimezoneMode {
    /// Derives the standard timezone offset from the observation site's coordinates.
    #[default]
    SiteLocal,
    /// Uses an explicit UTC offset or falls back to the host computer's standard UTC offset.
    Utc,
}

/// Timezone and Daylight Saving Time preferences persisted per observatory profile.
///
/// Defaults to coordinate-derived site-local time with host-aligned auto DST when applicable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimezoneConfig {
    /// Basis for determining the standard UTC offset (`SiteLocal` or `Utc`).
    #[serde(default)]
    pub mode: TimezoneMode,
    /// Standard (non-DST) UTC offset in minutes (`-720..=840`, i.e. UTC-12..UTC+14).
    /// Used when `mode == TimezoneMode::Utc`; `None` falls back to the host standard offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_minutes: Option<i32>,
    /// Daylight Saving Time (`+1h`) override.
    /// `None` auto-detects DST when the active offset matches the host timezone for the date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_enabled: Option<bool>,
}

impl TimezoneConfig {
    pub const MIN_UTC_OFFSET_MINUTES: i32 = -720;
    pub const MAX_UTC_OFFSET_MINUTES: i32 = 840;

    /// Clamps any configured UTC offset to `[-720, 840]` minutes.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            mode: self.mode,
            utc_offset_minutes: self
                .utc_offset_minutes
                .map(|m| m.clamp(Self::MIN_UTC_OFFSET_MINUTES, Self::MAX_UTC_OFFSET_MINUTES)),
            dst_enabled: self.dst_enabled,
        }
    }
}

/// Estimates a standard 15°-band civil UTC offset in minutes from a site's longitude.
pub fn estimate_utc_offset_minutes_from_longitude(longitude_deg: f64) -> i32 {
    if !longitude_deg.is_finite() {
        return 0;
    }
    ((longitude_deg / 15.0).round() as i32 * 60).clamp(
        TimezoneConfig::MIN_UTC_OFFSET_MINUTES,
        TimezoneConfig::MAX_UTC_OFFSET_MINUTES,
    )
}

/// Estimates an approximate longitude in degrees from a standard UTC offset in minutes.
pub fn estimate_longitude_from_utc_offset_minutes(offset_minutes: i32) -> f64 {
    ((f64::from(offset_minutes) / 60.0) * 15.0)
        .clamp(GeoLocation::MIN_LON_DEG, GeoLocation::MAX_LON_DEG)
}

/// Parses a UTC offset string such as `"+1"`, `"-5"`, `"UTC+5:30"`, `"0"`, `"site"`, or `"auto"`.
///
/// Returns `Some(None)` for `"auto"` / `"site"` / `"local"`, `Some(Some(minutes))` for valid
/// explicit offsets, or `None` when parsing fails or the offset falls outside `[-12:00, +14:00]`.
pub fn parse_utc_offset_minutes(input: &str) -> Option<Option<i32>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "auto" | "site" | "local") {
        return Some(None);
    }
    if matches!(lower.as_str(), "utc" | "gmt" | "z") {
        return Some(Some(0));
    }

    let rest = if let Some(after) = lower.strip_prefix("utc") {
        &trimmed[trimmed.len() - after.len()..]
    } else if let Some(after) = lower.strip_prefix("gmt") {
        &trimmed[trimmed.len() - after.len()..]
    } else {
        trimmed
    }
    .trim();

    if rest.is_empty() {
        return None;
    }

    let (sign, body) = if let Some(after) = rest.strip_prefix('+') {
        (1_i32, after.trim_start())
    } else if let Some(after) = rest.strip_prefix('-') {
        (-1_i32, after.trim_start())
    } else {
        (1_i32, rest)
    };

    if body.is_empty() || body.starts_with('+') || body.starts_with('-') {
        return None;
    }

    let total_abs_mins = if let Some((h_str, m_str)) = body.split_once(':') {
        let h: i32 = h_str.trim().parse().ok()?;
        let m: i32 = m_str.trim().parse().ok()?;
        if h < 0 || !(0..60).contains(&m) {
            return None;
        }
        h.checked_mul(60)?.checked_add(m)?
    } else if body.contains('.') {
        let hours: f64 = body.parse().ok()?;
        if !hours.is_finite() || hours < 0.0 {
            return None;
        }
        (hours * 60.0).round() as i32
    } else {
        let h: i32 = body.parse().ok()?;
        if h < 0 {
            return None;
        }
        h.checked_mul(60)?
    };

    let signed = sign * total_abs_mins;
    if !(TimezoneConfig::MIN_UTC_OFFSET_MINUTES..=TimezoneConfig::MAX_UTC_OFFSET_MINUTES)
        .contains(&signed)
    {
        return None;
    }
    Some(Some(signed))
}

/// Named observing site profile containing location and sync metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservatoryProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: ObservatoryIcon,
    pub location: GeoLocation,
    #[serde(default)]
    pub timezone: TimezoneConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon_id: Option<String>,
    #[serde(default)]
    pub source: ProfileSource,
    #[serde(default = "default_revision")]
    pub revision: u64,
    #[serde(default)]
    pub updated_at_unix_ms: u64,
}

/// Result of reconciling an incoming remote profile against the local catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    Inserted,
    Updated,
    Unchanged,
    LocalNewer,
}

/// Persisted collection of observatory profiles and the active selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObserverCatalog {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub active_profile_id: String,
    pub profiles: Vec<ObservatoryProfile>,
}

impl Default for ObserverCatalog {
    fn default() -> Self {
        Self {
            schema_version: OBSERVER_SCHEMA_VERSION,
            active_profile_id: "backyard".to_string(),
            profiles: vec![
                ObservatoryProfile {
                    id: "backyard".to_string(),
                    name: "Backyard".to_string(),
                    icon: ObservatoryIcon::Dome,
                    location: GeoLocation::default(),
                    timezone: TimezoneConfig::default(),
                    horizon_id: None,
                    source: ProfileSource::Local,
                    revision: 1,
                    updated_at_unix_ms: 0,
                },
                ObservatoryProfile {
                    id: "nearby-field".to_string(),
                    name: "Nearby Field".to_string(),
                    icon: ObservatoryIcon::Field,
                    location: GeoLocation::new(48.4047, 2.7016, 92.0),
                    timezone: TimezoneConfig::default(),
                    horizon_id: None,
                    source: ProfileSource::Local,
                    revision: 1,
                    updated_at_unix_ms: 0,
                },
            ],
        }
    }
}

impl Persistable for ObserverCatalog {
    const DOCUMENT_KEY: &'static str = "observer";
    const SCHEMA_VERSION: u32 = OBSERVER_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn normalize_after_load(&mut self) {
        self.schema_version = OBSERVER_SCHEMA_VERSION;
        if self.profiles.is_empty() {
            *self = Self::default();
            return;
        }

        for profile in &mut self.profiles {
            profile.location = profile.location.sanitized();
            profile.timezone = profile.timezone.sanitized();
            if let Some(hid) = profile.horizon_id.as_mut() {
                let trimmed_h = hid.trim();
                if trimmed_h.is_empty() {
                    profile.horizon_id = None;
                } else if trimmed_h.len() != hid.len() {
                    *hid = trimmed_h.to_string();
                }
            }
            let trimmed = profile.name.trim();
            if trimmed.is_empty() {
                profile.name = "Observatory".to_string();
            } else if trimmed.len() != profile.name.len() {
                profile.name = trimmed.to_string();
            }
            if profile.revision == 0 {
                profile.revision = 1;
            }
        }

        if !self.profiles.iter().any(|p| p.id == self.active_profile_id) {
            self.active_profile_id = self.profiles[0].id.clone();
        }
    }
}

impl ObserverCatalog {
    /// Builds an initial observatory catalog whose longitudes match the given UTC offset.
    pub fn default_for_utc_offset_minutes(offset_minutes: i32) -> Self {
        let lon = estimate_longitude_from_utc_offset_minutes(offset_minutes);
        let field_lon = if lon + 0.36 <= GeoLocation::MAX_LON_DEG {
            lon + 0.36
        } else {
            lon - 0.36
        };
        Self {
            schema_version: OBSERVER_SCHEMA_VERSION,
            active_profile_id: "backyard".to_string(),
            profiles: vec![
                ObservatoryProfile {
                    id: "backyard".to_string(),
                    name: "Backyard".to_string(),
                    icon: ObservatoryIcon::Dome,
                    location: GeoLocation::new(45.0, lon, 115.0),
                    timezone: TimezoneConfig::default(),
                    horizon_id: None,
                    source: ProfileSource::Local,
                    revision: 1,
                    updated_at_unix_ms: 0,
                },
                ObservatoryProfile {
                    id: "nearby-field".to_string(),
                    name: "Nearby Field".to_string(),
                    icon: ObservatoryIcon::Field,
                    location: GeoLocation::new(44.57, field_lon, 92.0),
                    timezone: TimezoneConfig::default(),
                    horizon_id: None,
                    source: ProfileSource::Local,
                    revision: 1,
                    updated_at_unix_ms: 0,
                },
            ],
        }
    }

    /// Returns the currently active profile (falling back to the first profile if needed).
    pub fn active_profile(&self) -> &ObservatoryProfile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile_id)
            .unwrap_or(&self.profiles[0])
    }

    /// Looks up a profile by ID.
    pub fn profile(&self, id: &str) -> Option<&ObservatoryProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Sets the active profile ID if it exists.
    pub fn set_active(&mut self, id: &str) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(idx) = self.profiles.iter().position(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        self.active_profile_id = id.to_string();
        Ok(&self.profiles[idx])
    }

    /// Updates a profile's coordinates in memory without bumping its sync revision.
    ///
    /// Intended for smooth live slider previews before [`Self::commit_location`].
    pub fn set_location_in_memory(
        &mut self,
        id: &str,
        location: GeoLocation,
    ) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        profile.location = location.sanitized();
        Ok(profile)
    }

    /// Commits a profile's coordinates, bumping its revision and modification timestamp.
    pub fn commit_location(
        &mut self,
        id: &str,
        location: GeoLocation,
    ) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        profile.location = location.sanitized();
        profile.revision = profile.revision.saturating_add(1);
        profile.updated_at_unix_ms = now_unix_ms();
        Ok(profile)
    }

    /// Updates a profile's display name and icon, bumping its revision and timestamp.
    pub fn update_profile(
        &mut self,
        id: &str,
        name: &str,
        icon: ObservatoryIcon,
    ) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            profile.name = trimmed.to_string();
        }
        profile.icon = icon;
        profile.revision = profile.revision.saturating_add(1);
        profile.updated_at_unix_ms = now_unix_ms();
        Ok(profile)
    }

    /// Updates a profile's timezone preferences, bumping its revision and timestamp.
    pub fn update_timezone(
        &mut self,
        id: &str,
        timezone: TimezoneConfig,
    ) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        profile.timezone = timezone.sanitized();
        profile.revision = profile.revision.saturating_add(1);
        profile.updated_at_unix_ms = now_unix_ms();
        Ok(profile)
    }

    /// Assigns or clears the custom horizon background linked to a profile.
    pub fn set_horizon(
        &mut self,
        id: &str,
        horizon_id: Option<String>,
    ) -> Result<&ObservatoryProfile, PersistenceError> {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        profile.horizon_id = horizon_id.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
        profile.revision = profile.revision.saturating_add(1);
        profile.updated_at_unix_ms = now_unix_ms();
        Ok(profile)
    }

    /// Creates a new local profile, activates it, and returns a reference to it.
    pub fn create_profile(
        &mut self,
        name: &str,
        icon: ObservatoryIcon,
        location: GeoLocation,
    ) -> &ObservatoryProfile {
        let trimmed = name.trim();
        let display_name = if trimmed.is_empty() {
            "New Site".to_string()
        } else {
            trimmed.to_string()
        };
        let id = self.unique_id_for(&display_name);
        self.profiles.push(ObservatoryProfile {
            id: id.clone(),
            name: display_name,
            icon,
            location: location.sanitized(),
            timezone: TimezoneConfig::default(),
            horizon_id: None,
            source: ProfileSource::Local,
            revision: 1,
            updated_at_unix_ms: now_unix_ms(),
        });
        self.active_profile_id = id;
        self.profiles.last().unwrap_or(&self.profiles[0])
    }

    /// Deletes a profile by ID unless it is the sole remaining profile.
    pub fn delete_profile(&mut self, id: &str) -> Result<(), PersistenceError> {
        if self.profiles.len() <= 1 {
            return Err(PersistenceError::CannotDeleteLastProfile);
        }
        let Some(idx) = self.profiles.iter().position(|p| p.id == id) else {
            return Err(PersistenceError::ProfileNotFound(id.to_string()));
        };
        self.profiles.remove(idx);
        if self.active_profile_id == id {
            self.active_profile_id = self.profiles[0].id.clone();
        }
        Ok(())
    }

    /// Reconciles an incoming remote observatory profile against local state.
    ///
    /// Revision-based ordering allows deterministic sync when connecting to a
    /// remote observatory daemon without overwriting newer local edits.
    pub fn upsert_remote_profile(&mut self, mut remote: ObservatoryProfile) -> MergeOutcome {
        remote.location = remote.location.sanitized();
        remote.timezone = remote.timezone.sanitized();
        if let Some(hid) = remote.horizon_id.as_mut() {
            let trimmed_h = hid.trim();
            if trimmed_h.is_empty() {
                remote.horizon_id = None;
            } else if trimmed_h.len() != hid.len() {
                *hid = trimmed_h.to_string();
            }
        }
        if remote.name.trim().is_empty() {
            remote.name = "Remote Observatory".to_string();
        } else {
            remote.name = remote.name.trim().to_string();
        }
        if remote.id.trim().is_empty() {
            remote.id = self.unique_id_for(&remote.name);
        }
        if remote.revision == 0 {
            remote.revision = 1;
        }

        let Some(existing) = self.profiles.iter_mut().find(|p| p.id == remote.id) else {
            self.profiles.push(remote);
            return MergeOutcome::Inserted;
        };

        if *existing == remote {
            return MergeOutcome::Unchanged;
        }

        let remote_is_newer = remote.revision > existing.revision
            || (remote.revision == existing.revision
                && remote.updated_at_unix_ms > existing.updated_at_unix_ms);

        if remote_is_newer {
            *existing = remote;
            MergeOutcome::Updated
        } else {
            MergeOutcome::LocalNewer
        }
    }

    fn unique_id_for(&self, name: &str) -> String {
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
        let stem = if base.is_empty() { "site" } else { base };

        if !self.profiles.iter().any(|p| p.id == stem) {
            return stem.to_string();
        }
        let mut suffix: u32 = 2;
        loop {
            let candidate = format!("{stem}-{suffix}");
            if !self.profiles.iter().any(|p| p.id == candidate) {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }
}
