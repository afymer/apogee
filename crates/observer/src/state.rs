use std::time::{Duration, SystemTime, UNIX_EPOCH};

use celestial::coords::geodetic::GeodeticPosition;
use celestial::observer::ObservationContext;
use celestial::solar::{DailySolarEvents, SkyPhase, find_sky_events, sample_day_solar_profile, solar_altitude_deg};
use chrono::{
    DateTime, Datelike, Days, FixedOffset, Local, NaiveDate, NaiveTime, Offset, TimeDelta,
    TimeZone, Timelike, Utc,
};
use gpui::{App, AppContext, Context, Entity, Global};
use persistence::{
    DocumentStore, GeoLocation, MergeOutcome, ObservatoryIcon, ObservatoryProfile,
    ObserverCatalog, TimezoneConfig, TimezoneMode, estimate_utc_offset_minutes_from_longitude,
};

/// Observation time synchronization mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ObserverClock {
    /// Automatically tracks system wall-clock time.
    Live,
    /// Pinned to a user-selected timestamp (Unix seconds).
    Manual { unix_seconds: f64 },
}

impl ObserverClock {
    /// Returns `true` when tracking live wall-clock time.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    /// Returns the effective Unix timestamp in seconds.
    pub fn unix_seconds_or_now(self) -> f64 {
        match self {
            Self::Live => now_unix_seconds(),
            Self::Manual { unix_seconds } => unix_seconds,
        }
    }
}

/// Global GPUI entity wrapper for [`ObserverModel`].
#[derive(Clone)]
pub struct GlobalObserver(pub Entity<ObserverModel>);

impl Global for GlobalObserver {}

/// Reactive observer state owning observatory profiles, active coordinates,
/// observation clock, and atomic disk persistence.
pub struct ObserverModel {
    store: DocumentStore,
    catalog: ObserverCatalog,
    clock: ObserverClock,
    dirty_location: bool,
}

/// Initializes [`GlobalObserver`] using the default configuration directory.
pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

/// Initializes [`GlobalObserver`] backed by a specific [`DocumentStore`].
pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let catalog = store.load_or_else(|| {
        let (host_std_mins, _) = host_standard_offset_and_dst(now_unix_seconds());
        ObserverCatalog::default_for_utc_offset_minutes(host_std_mins)
    });
    let model = cx.new(|cx| {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let res = this.update(cx, |model: &mut ObserverModel, cx| {
                    if model.clock.is_live() {
                        cx.notify();
                    }
                });
                if res.is_err() {
                    break;
                }
            }
        })
        .detach();

        ObserverModel {
            store,
            catalog,
            clock: ObserverClock::Live,
            dirty_location: false,
        }
    });

    cx.set_global(GlobalObserver(model));
}

impl ObserverModel {
    /// Returns the full observatory catalog.
    pub fn catalog(&self) -> &ObserverCatalog {
        &self.catalog
    }

    /// Returns all configured observatory profiles.
    pub fn profiles(&self) -> &[ObservatoryProfile] {
        &self.catalog.profiles
    }

    /// Returns the currently active observatory profile.
    pub fn active_profile(&self) -> &ObservatoryProfile {
        self.catalog.active_profile()
    }

    /// Returns the active profile's geodetic coordinates.
    pub fn active_location(&self) -> GeoLocation {
        self.catalog.active_profile().location
    }

    /// Converts the active location into a celestial [`GeodeticPosition`].
    pub fn geodetic_position(&self) -> GeodeticPosition {
        let loc = self.active_location();
        GeodeticPosition::from_degrees(loc.latitude_deg, loc.longitude_deg, loc.elevation_m)
    }

    /// Returns the current clock mode.
    pub fn clock(&self) -> ObserverClock {
        self.clock
    }

    /// Returns the active timestamp in Unix seconds.
    pub fn effective_unix_seconds(&self) -> f64 {
        self.clock.unix_seconds_or_now()
    }

    /// Returns the active timestamp converted to the active site's configured timezone + DST.
    pub fn effective_local_datetime(&self) -> DateTime<FixedOffset> {
        unix_seconds_to_utc(self.effective_unix_seconds()).with_timezone(&self.effective_fixed_offset())
    }

    /// Returns the active site's configured timezone basis (`SiteLocal` or `Utc`).
    pub fn timezone_mode(&self) -> TimezoneMode {
        self.active_profile().timezone.mode
    }

    /// Returns the active site's standard (non-DST) UTC offset in minutes.
    pub fn effective_standard_utc_offset_minutes(&self) -> i32 {
        let tz = self.active_profile().timezone;
        let host_std_mins = match tz.mode {
            TimezoneMode::SiteLocal => 0,
            TimezoneMode::Utc if tz.utc_offset_minutes.is_some() => 0,
            TimezoneMode::Utc => host_standard_offset_and_dst(self.effective_unix_seconds()).0,
        };
        resolve_standard_utc_offset_minutes(tz, self.active_location().longitude_deg, host_std_mins)
    }

    /// Returns whether Daylight Saving Time (+1h) is currently active for the site/date.
    pub fn effective_dst_enabled(&self) -> bool {
        let tz = self.active_profile().timezone;
        if let Some(dst) = tz.dst_enabled {
            return dst;
        }
        let (host_std_mins, host_dst) = host_standard_offset_and_dst(self.effective_unix_seconds());
        let effective_std_mins = resolve_standard_utc_offset_minutes(
            tz,
            self.active_location().longitude_deg,
            host_std_mins,
        );
        resolve_dst_enabled(tz, effective_std_mins, host_std_mins, host_dst)
    }

    /// Returns `true` if standard UTC offset tracks the host timezone automatically.
    pub fn is_utc_offset_auto(&self) -> bool {
        self.active_profile().timezone.utc_offset_minutes.is_none()
    }

    /// Returns `true` if DST tracks the host timezone automatically.
    pub fn is_dst_auto(&self) -> bool {
        self.active_profile().timezone.dst_enabled.is_none()
    }

    /// Returns total effective UTC offset in seconds (standard offset + DST if active).
    pub fn effective_utc_offset_secs(&self) -> i32 {
        let base_secs = self.effective_standard_utc_offset_minutes() * 60;
        let dst_secs = if self.effective_dst_enabled() { 3600 } else { 0 };
        (base_secs + dst_secs).clamp(-18 * 3600, 18 * 3600)
    }

    /// Builds the [`FixedOffset`] corresponding to the active site's timezone + DST.
    pub fn effective_fixed_offset(&self) -> FixedOffset {
        FixedOffset::east_opt(self.effective_utc_offset_secs()).unwrap_or_else(|| Utc.fix())
    }

    /// Returns the Unix timestamp for local `00:00:00` of the active local calendar date.
    pub fn local_midnight_unix_seconds(&self) -> f64 {
        let dt = self.effective_local_datetime();
        let offset = *dt.offset();
        let naive_midnight = dt.date_naive().and_time(NaiveTime::MIN);
        if let Some(midnight) = offset.from_local_datetime(&naive_midnight).single() {
            return midnight.timestamp() as f64;
        }
        let secs_since_midnight = i64::from(dt.num_seconds_from_midnight());
        (dt - TimeDelta::seconds(secs_since_midnight)).timestamp() as f64
    }

    /// Returns the local hour of day in `[0.0, 24.0)`.
    pub fn local_hour_of_day(&self) -> f64 {
        let dt = self.effective_local_datetime();
        (f64::from(dt.num_seconds_from_midnight()) / 3600.0).clamp(0.0, 23.9999)
    }

    /// Builds a celestial [`ObservationContext`] for the active site and time.
    pub fn observation_context(&self) -> ObservationContext {
        let mut ctx = ObservationContext {
            location: self.geodetic_position(),
            ..ObservationContext::default()
        };
        ctx.set_epoch_unix_seconds(self.effective_unix_seconds());
        ctx
    }

    /// Computes solar altitude in degrees for the active site and time.
    pub fn solar_altitude_deg(&self) -> f64 {
        let ctx = self.observation_context();
        solar_altitude_deg(ctx.location, ctx.epoch)
    }

    /// Returns the sky twilight classification for the active site and time.
    pub fn sky_phase(&self) -> SkyPhase {
        SkyPhase::from_solar_altitude_deg(self.solar_altitude_deg())
    }

    /// Computes solar sunrise, sunset, and astronomical twilight hours for the active local date.
    pub fn daily_solar_events(&self) -> DailySolarEvents {
        find_sky_events(self.geodetic_position(), self.local_midnight_unix_seconds())
    }

    /// Samples solar altitude across the active 24-hour local calendar day.
    pub fn solar_day_profile(&self, samples: usize) -> Vec<f64> {
        sample_day_solar_profile(
            self.geodetic_position(),
            self.local_midnight_unix_seconds(),
            samples,
        )
    }

    /// Switches the active observatory profile and persists the selection.
    pub fn select_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        self.flush_dirty_location_quiet();
        if self.catalog.active_profile_id == id {
            return;
        }
        if let Err(err) = self.catalog.set_active(id) {
            tracing::warn!("Failed to select observatory profile {id:?}: {err}");
            return;
        }
        self.persist();
        cx.notify();
    }

    /// Creates a new local observatory profile, makes it active, and persists.
    pub fn create_profile(
        &mut self,
        name: &str,
        icon: ObservatoryIcon,
        location: GeoLocation,
        cx: &mut Context<Self>,
    ) {
        self.flush_dirty_location_quiet();
        self.catalog.create_profile(name, icon, location);
        self.persist();
        cx.notify();
    }

    /// Updates an existing observatory profile's name and icon and persists.
    pub fn update_profile_meta(
        &mut self,
        id: &str,
        name: &str,
        icon: ObservatoryIcon,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.catalog.update_profile(id, name, icon) {
            tracing::warn!("Failed to update observatory profile {id:?}: {err}");
            return;
        }
        self.persist();
        cx.notify();
    }

    /// Deletes an observatory profile (unless it is the last remaining one) and persists.
    pub fn delete_profile(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        match self.catalog.delete_profile(id) {
            Ok(()) => {
                self.dirty_location = false;
                self.persist();
                cx.notify();
                true
            }
            Err(err) => {
                tracing::warn!("Could not delete observatory profile {id:?}: {err}");
                false
            }
        }
    }

    /// Reconciles an incoming profile received from a remote observatory daemon.
    pub fn apply_remote_profile(
        &mut self,
        remote: ObservatoryProfile,
        activate_if_new: bool,
        cx: &mut Context<Self>,
    ) -> MergeOutcome {
        let remote_id = remote.id.clone();
        let outcome = self.catalog.upsert_remote_profile(remote);
        if activate_if_new && outcome == MergeOutcome::Inserted {
            let _ = self.catalog.set_active(&remote_id);
        }
        if matches!(outcome, MergeOutcome::Inserted | MergeOutcome::Updated) {
            self.persist();
            cx.notify();
        }
        outcome
    }

    /// Updates the active site coordinates in memory for responsive live drag previews.
    pub fn preview_active_location(&mut self, location: GeoLocation, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        if self
            .catalog
            .set_location_in_memory(&active_id, location)
            .is_ok()
        {
            self.dirty_location = true;
            cx.notify();
        }
    }

    /// Commits active site coordinates, bumps revision, and persists atomically to disk.
    pub fn commit_active_location(&mut self, location: GeoLocation, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        if self.catalog.commit_location(&active_id, location).is_ok() {
            self.dirty_location = false;
            self.persist();
            cx.notify();
        }
    }

    /// Flushes any pending in-memory location preview edits to disk.
    pub fn flush_dirty_location(&mut self, cx: &mut Context<Self>) {
        if self.flush_dirty_location_quiet() {
            cx.notify();
        }
    }

    /// Switches observation time back to automatic live wall-clock sync.
    pub fn set_live(&mut self, cx: &mut Context<Self>) {
        if !self.clock.is_live() {
            self.clock = ObserverClock::Live;
            cx.notify();
        }
    }

    /// Pins observation time to a specific Unix timestamp in seconds.
    pub fn set_manual_unix_seconds(&mut self, unix_seconds: f64, cx: &mut Context<Self>) {
        let safe = if unix_seconds.is_finite() {
            unix_seconds
        } else {
            now_unix_seconds()
        };
        self.clock = ObserverClock::Manual { unix_seconds: safe };
        cx.notify();
    }

    /// Pins observation time to a fractional hour `[0.0, 24.0)` on the active local calendar date.
    pub fn set_local_hour_of_day(&mut self, hour_of_day: f64, cx: &mut Context<Self>) {
        let clamped_h = if hour_of_day.is_finite() {
            hour_of_day.clamp(0.0, 23.9997)
        } else {
            0.0
        };
        let total_secs = (clamped_h * 3600.0).round() as i64;
        let target = self.local_midnight_unix_seconds() + total_secs as f64;
        self.set_manual_unix_seconds(target, cx);
    }

    /// Pins observation time to `hour:minute:00` on the active local calendar date.
    pub fn set_local_hm(&mut self, hour: u32, minute: u32, cx: &mut Context<Self>) {
        let secs = i64::from(hour.min(23)) * 3600 + i64::from(minute.min(59)) * 60;
        let target = self.local_midnight_unix_seconds() + secs as f64;
        self.set_manual_unix_seconds(target, cx);
    }

    /// Shifts the observation date by `delta_days` while preserving local time of day.
    pub fn step_days(&mut self, delta_days: i64, cx: &mut Context<Self>) {
        let dt = self.effective_local_datetime();
        let next_date = if delta_days >= 0 {
            dt.date_naive().checked_add_days(Days::new(delta_days as u64))
        } else {
            dt.date_naive()
                .checked_sub_days(Days::new(delta_days.unsigned_abs()))
        };

        let full_host_auto = self.timezone_mode() == TimezoneMode::Utc
            && self.is_utc_offset_auto()
            && self.is_dst_auto();
        let target_secs = next_date
            .and_then(|d| {
                if full_host_auto {
                    resolve_local_date_time(d, dt.time()).map(|r| r.timestamp() as f64)
                } else {
                    self.effective_fixed_offset()
                        .from_local_datetime(&d.and_time(dt.time()))
                        .single()
                        .map(|r| r.timestamp() as f64)
                }
            })
            .unwrap_or_else(|| self.effective_unix_seconds() + delta_days as f64 * 86_400.0);

        self.set_manual_unix_seconds(target_secs, cx);
    }

    /// Jumps to today's local date while keeping the currently scrubbed local time of day.
    pub fn jump_to_today_keep_time(&mut self, cx: &mut Context<Self>) {
        let dt = self.effective_local_datetime();
        let full_host_auto = self.timezone_mode() == TimezoneMode::Utc
            && self.is_utc_offset_auto()
            && self.is_dst_auto();
        if full_host_auto {
            let today = Local::now().date_naive();
            if let Some(resolved) = resolve_local_date_time(today, dt.time()) {
                self.set_manual_unix_seconds(resolved.timestamp() as f64, cx);
            }
        } else {
            let offset = self.effective_fixed_offset();
            let today = Utc::now().with_timezone(&offset).date_naive();
            if let Some(resolved) = offset.from_local_datetime(&today.and_time(dt.time())).single() {
                self.set_manual_unix_seconds(resolved.timestamp() as f64, cx);
            }
        }
    }

    /// Sets the active site's timezone basis (`SiteLocal` or `Utc`).
    pub fn set_timezone_mode(&mut self, mode: TimezoneMode, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        let mut tz = self.active_profile().timezone;
        if tz.mode == mode {
            return;
        }
        tz.mode = mode;
        if self.catalog.update_timezone(&active_id, tz).is_ok() {
            self.persist();
            cx.notify();
        }
    }

    /// Sets an explicit standard UTC offset in minutes (`TimezoneMode::Utc`),
    /// or resets back to coordinate-based `TimezoneMode::SiteLocal` when `None`.
    pub fn set_utc_offset_minutes(&mut self, minutes: Option<i32>, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        let mut tz = self.active_profile().timezone;
        let (target_mode, clamped) = match minutes {
            Some(m) => (
                TimezoneMode::Utc,
                Some(m.clamp(
                    TimezoneConfig::MIN_UTC_OFFSET_MINUTES,
                    TimezoneConfig::MAX_UTC_OFFSET_MINUTES,
                )),
            ),
            None => (TimezoneMode::SiteLocal, None),
        };
        if tz.mode == target_mode && tz.utc_offset_minutes == clamped {
            return;
        }
        tz.mode = target_mode;
        tz.utc_offset_minutes = clamped;
        if self.catalog.update_timezone(&active_id, tz).is_ok() {
            self.persist();
            cx.notify();
        }
    }

    /// Restores a previously saved [`TimezoneConfig`] snapshot on the active site.
    pub fn restore_timezone_config(&mut self, timezone: TimezoneConfig, cx: &mut Context<Self>) {
        let sanitized = timezone.sanitized();
        if self.active_profile().timezone == sanitized {
            return;
        }
        let active_id = self.catalog.active_profile_id.clone();
        if self.catalog.update_timezone(&active_id, sanitized).is_ok() {
            self.persist();
            cx.notify();
        }
    }

    /// Steps the active site's standard UTC offset by `delta_minutes`.
    pub fn step_utc_offset_minutes(&mut self, delta_minutes: i32, cx: &mut Context<Self>) {
        let current = self.effective_standard_utc_offset_minutes();
        let next = (current + delta_minutes).clamp(
            TimezoneConfig::MIN_UTC_OFFSET_MINUTES,
            TimezoneConfig::MAX_UTC_OFFSET_MINUTES,
        );
        self.set_utc_offset_minutes(Some(next), cx);
    }

    /// Toggles Daylight Saving Time (`+1h`) explicitly on the active site.
    pub fn toggle_dst(&mut self, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        let mut tz = self.active_profile().timezone;
        tz.dst_enabled = Some(!self.effective_dst_enabled());
        if self.catalog.update_timezone(&active_id, tz).is_ok() {
            self.persist();
            cx.notify();
        }
    }

    /// Restores automatic host DST detection for the active site.
    pub fn reset_dst_auto(&mut self, cx: &mut Context<Self>) {
        let active_id = self.catalog.active_profile_id.clone();
        let mut tz = self.active_profile().timezone;
        if tz.dst_enabled.take().is_some()
            && self.catalog.update_timezone(&active_id, tz).is_ok()
        {
            self.persist();
            cx.notify();
        }
    }

    fn flush_dirty_location_quiet(&mut self) -> bool {
        if !self.dirty_location {
            return false;
        }
        let active_id = self.catalog.active_profile_id.clone();
        let loc = self.catalog.active_profile().location;
        if self.catalog.commit_location(&active_id, loc).is_ok() {
            self.dirty_location = false;
            self.persist();
            return true;
        }
        self.dirty_location = false;
        false
    }

    fn persist(&self) {
        if let Err(err) = self.store.save(&self.catalog) {
            tracing::warn!("Failed to persist observer catalog: {err}");
        }
    }
}

fn resolve_standard_utc_offset_minutes(
    tz: TimezoneConfig,
    longitude_deg: f64,
    host_std_mins: i32,
) -> i32 {
    match tz.mode {
        TimezoneMode::SiteLocal => estimate_utc_offset_minutes_from_longitude(longitude_deg),
        TimezoneMode::Utc => tz.utc_offset_minutes.unwrap_or(host_std_mins),
    }
}

fn resolve_dst_enabled(
    tz: TimezoneConfig,
    effective_std_mins: i32,
    host_std_mins: i32,
    host_dst: bool,
) -> bool {
    tz.dst_enabled
        .unwrap_or(effective_std_mins == host_std_mins && host_dst)
}

fn now_unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn unix_seconds_to_utc(unix_seconds: f64) -> DateTime<Utc> {
    let safe = if unix_seconds.is_finite() {
        unix_seconds
    } else {
        now_unix_seconds()
    };
    let secs = safe.floor() as i64;
    let nanos = ((safe - secs as f64) * 1e9)
        .round()
        .clamp(0.0, 999_999_999.0) as u32;
    DateTime::from_timestamp(secs, nanos).unwrap_or_else(Utc::now)
}

fn host_standard_offset_and_dst(unix_seconds: f64) -> (i32, bool) {
    let local_dt = unix_seconds_to_utc(unix_seconds).with_timezone(&Local);
    let cur_secs = local_dt.offset().local_minus_utc();
    let year = local_dt.year();

    let jan_secs = NaiveDate::from_ymd_opt(year, 1, 1)
        .and_then(|d| resolve_local_date_time(d, NaiveTime::MIN))
        .map(|dt| dt.offset().local_minus_utc())
        .unwrap_or(cur_secs);
    let jul_secs = NaiveDate::from_ymd_opt(year, 7, 1)
        .and_then(|d| resolve_local_date_time(d, NaiveTime::MIN))
        .map(|dt| dt.offset().local_minus_utc())
        .unwrap_or(cur_secs);

    let std_secs = jan_secs.min(jul_secs);
    (std_secs / 60, cur_secs > std_secs)
}

fn resolve_local_date_time(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    let naive = date.and_time(time);
    Local
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| Local.from_local_datetime(&naive).latest())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compute_effective_offset_secs(
        tz: TimezoneConfig,
        longitude_deg: f64,
        host_std_mins: i32,
        host_dst: bool,
    ) -> i32 {
        let base_mins = resolve_standard_utc_offset_minutes(tz, longitude_deg, host_std_mins);
        let dst = resolve_dst_enabled(tz, base_mins, host_std_mins, host_dst);
        base_mins * 60 + if dst { 3600 } else { 0 }
    }

    #[test]
    fn timezone_effective_offset_combines_standard_and_dst() {
        let tz = TimezoneConfig {
            mode: TimezoneMode::Utc,
            utc_offset_minutes: Some(60),
            dst_enabled: Some(false),
        };
        assert_eq!(compute_effective_offset_secs(tz, 0.0, 0, true), 3600);

        let tz_dst = TimezoneConfig {
            mode: TimezoneMode::Utc,
            utc_offset_minutes: Some(60),
            dst_enabled: Some(true),
        };
        assert_eq!(compute_effective_offset_secs(tz_dst, 0.0, 0, false), 7200);

        let tz_neg = TimezoneConfig {
            mode: TimezoneMode::Utc,
            utc_offset_minutes: Some(-300),
            dst_enabled: Some(true),
        };
        assert_eq!(compute_effective_offset_secs(tz_neg, 0.0, 0, false), -14400);
    }

    #[test]
    fn site_local_derives_offset_from_longitude_and_gates_auto_dst() {
        let site_tz = TimezoneConfig::default();
        assert_eq!(site_tz.mode, TimezoneMode::SiteLocal);

        // Tokyo (~139.7°E => +9h = +540m) does NOT inherit host DST when host is UTC+1.
        assert_eq!(compute_effective_offset_secs(site_tz, 139.7, 60, true), 9 * 3600);

        // Site near host band (+15°E => +1h = +60m) inherits host DST when host std == +60m.
        assert_eq!(compute_effective_offset_secs(site_tz, 15.0, 60, true), 2 * 3600);

        // Site near host band when host DST is inactive remains at standard offset.
        assert_eq!(compute_effective_offset_secs(site_tz, 15.0, 60, false), 3600);
    }
}
