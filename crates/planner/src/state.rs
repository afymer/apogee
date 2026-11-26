use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use celestial::coords::{Angle, canonical_spherical_to_cartesian};
use client::GlobalDaemonState;
use gpui::{App, AppContext, Context, Entity, Global};
use jiff::tz::Offset;
use jiff::{Span, Timestamp};
use observer::ObserverModel;
use persistence::{DocumentStore, PlannerCatalog};
use protocol::{
    MIN_BLOCK_FOOTPRINT_SECS, NightPlan, NightPlanPolicy, PlannedTarget, SequencePhase,
    SequenceStep, SequenceTemplate, builtin_sequence_templates, snap_group_delta_without_overlap,
};
use tracing::warn;

#[derive(Clone, Debug, PartialEq)]
pub struct ExploreTargetRequest {
    pub nonce: u64,
    pub name: String,
    pub ra_deg: f64,
    pub dec_deg: f64,
}

#[derive(Clone)]
pub struct GlobalNightPlan(pub Entity<PlanModel>);

impl Global for GlobalNightPlan {}

pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let catalog: PlannerCatalog = store.load_or_default();
    let model = cx.new(|_cx| PlanModel::new(store, catalog));
    cx.set_global(GlobalNightPlan(model));
}

/// Computes the evening calendar date (`YYYY-MM-DD`) and default start Unix timestamp
/// (max of astronomical dusk/sunset and current effective time, rounded up to 5-minute steps).
pub fn observer_evening_date_and_start(observer: &ObserverModel) -> (String, i64) {
    let local_dt = observer.effective_local_datetime();
    let local_hour = observer.local_hour_of_day();
    let evening_date = if local_hour < 12.0 {
        local_dt
            .date()
            .checked_sub(Span::new().days(1))
            .unwrap_or_else(|_| local_dt.date())
    } else {
        local_dt.date()
    };

    let events = observer.daily_solar_events();
    let dusk_hour = events.astro_dusk_hour.or(events.sunset_hour).unwrap_or(21.0);
    let midnight = observer.local_midnight_unix_seconds();
    let dusk_secs = if local_hour < 12.0 {
        midnight - (24.0 - dusk_hour) * 3600.0
    } else {
        midnight + dusk_hour * 3600.0
    };

    let now_secs = observer.effective_unix_seconds();
    let base_secs = dusk_secs.max(now_secs);
    let rounded_secs = ((base_secs / 300.0).ceil() * 300.0) as i64;

    (evening_date.to_string(), rounded_secs)
}

fn next_unique_id(prefix: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}")
}

fn sanitize_slug(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed
    }
}

pub struct PlanModel {
    store: DocumentStore,
    catalog: PlannerCatalog,
    selected_target_id: Option<String>,
    selected_target_ids: HashSet<String>,
    explore_request: Option<ExploreTargetRequest>,
    explore_nonce: u64,
    plan_revision: u64,
}

impl PlanModel {
    pub fn new(store: DocumentStore, mut catalog: PlannerCatalog) -> Self {
        for target in &mut catalog.plan.targets {
            target.enabled = true;
            for step in &mut target.steps {
                step.enabled = true;
            }
        }
        Self::resolve_overlaps_in_plan(&mut catalog.plan);
        let selected_target_id = catalog.plan.targets.first().map(|t| t.id.clone());
        let mut selected_target_ids = HashSet::new();
        if let Some(id) = &selected_target_id {
            selected_target_ids.insert(id.clone());
        }
        Self {
            store,
            catalog,
            selected_target_id,
            selected_target_ids,
            explore_request: None,
            explore_nonce: 0,
            plan_revision: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.plan_revision
    }

    pub fn plan(&self) -> &NightPlan {
        &self.catalog.plan
    }

    pub fn all_templates(&self) -> Vec<SequenceTemplate> {
        self.catalog.all_templates()
    }

    pub fn selected_target_id(&self) -> Option<&str> {
        self.selected_target_id.as_deref()
    }

    pub fn selected_target_ids(&self) -> &HashSet<String> {
        &self.selected_target_ids
    }

    pub fn is_target_selected(&self, id: &str) -> bool {
        self.selected_target_ids.contains(id)
    }

    pub fn selected_target(&self) -> Option<&PlannedTarget> {
        let id = self.selected_target_id.as_deref()?;
        self.catalog.plan.targets.iter().find(|t| t.id == id)
    }

    pub fn select_target(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let mut next_set = HashSet::new();
        if let Some(ref tid) = id {
            next_set.insert(tid.clone());
        }
        if self.selected_target_id != id || self.selected_target_ids != next_set {
            self.selected_target_id = id;
            self.selected_target_ids = next_set;
            cx.notify();
        }
    }

    pub fn toggle_target_selection(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.catalog.plan.targets.iter().any(|t| t.id == id) {
            return;
        }
        if self.selected_target_ids.contains(id) {
            self.selected_target_ids.remove(id);
            if self.selected_target_id.as_deref() == Some(id) {
                self.selected_target_id = self
                    .catalog
                    .plan
                    .targets
                    .iter()
                    .find(|t| self.selected_target_ids.contains(&t.id))
                    .map(|t| t.id.clone());
            }
        } else {
            self.selected_target_ids.insert(id.to_string());
            self.selected_target_id = Some(id.to_string());
        }
        cx.notify();
    }

    pub fn set_selected_targets(
        &mut self,
        ids: Vec<String>,
        primary: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let valid_ids: HashSet<String> = self
            .catalog
            .plan
            .targets
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let next_set: HashSet<String> = ids
            .into_iter()
            .filter(|id| valid_ids.contains(id))
            .collect();
        let next_primary = primary
            .filter(|id| next_set.contains(id))
            .or_else(|| {
                self.catalog
                    .plan
                    .targets
                    .iter()
                    .find(|t| next_set.contains(&t.id))
                    .map(|t| t.id.clone())
            });
        if self.selected_target_ids != next_set || self.selected_target_id != next_primary {
            self.selected_target_ids = next_set;
            self.selected_target_id = next_primary;
            cx.notify();
        }
    }

    pub fn set_night_date(&mut self, night_date: String, cx: &mut Context<Self>) {
        if self.catalog.plan.night_date != night_date {
            self.catalog.plan.night_date = night_date;
            self.save_and_notify(cx);
        }
    }

    fn resolve_overlaps_in_plan(plan: &mut NightPlan) {
        plan.targets.sort_by_key(|t| t.start_unix_secs);
        let mut cursor_end: Option<i64> = None;
        for target in &mut plan.targets {
            if let Some(prev_end) = cursor_end
                && target.start_unix_secs < prev_end
            {
                target.start_unix_secs = prev_end;
            }
            cursor_end = Some(target.footprint_end_unix_secs());
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_target(
        &mut self,
        name: impl Into<String>,
        ra_deg: f64,
        dec_deg: f64,
        night_date: Option<String>,
        default_start_unix_secs: i64,
        template_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> String {
        let name = name.into();
        let trimmed_name = if name.trim().is_empty() {
            "Target".to_string()
        } else {
            name.trim().to_string()
        };

        if (self.catalog.plan.targets.is_empty() || self.catalog.plan.night_date.is_empty())
            && let Some(date) = night_date
            && !date.trim().is_empty()
        {
            self.catalog.plan.night_date = date;
        }

        let start_unix_secs = self
            .catalog
            .plan
            .targets
            .iter()
            .map(PlannedTarget::footprint_end_unix_secs)
            .max()
            .unwrap_or(default_start_unix_secs);

        let templates = self.all_templates();
        let selected_tpl_id = template_id.unwrap_or("quick-test");
        let steps = templates
            .iter()
            .find(|t| t.id == selected_tpl_id)
            .map(|t| t.steps.clone())
            .unwrap_or_else(|| {
                builtin_sequence_templates()
                    .into_iter()
                    .find(|t| t.id == "quick-test")
                    .map(|t| t.steps)
                    .unwrap_or_default()
            });

        let id = next_unique_id(&sanitize_slug(&trimmed_name));
        let color_hue_deg = self.catalog.plan.next_palette_hue();
        let policy = self.catalog.plan.policy;

        let target = PlannedTarget {
            id: id.clone(),
            name: trimmed_name,
            ra_deg: if ra_deg.is_finite() {
                ra_deg.rem_euclid(360.0)
            } else {
                0.0
            },
            dec_deg: if dec_deg.is_finite() {
                dec_deg.clamp(-90.0, 90.0)
            } else {
                0.0
            },
            color_hue_deg,
            enabled: true,
            start_unix_secs,
            steps,
            slew_to_target: policy.default_slew_to_target,
            auto_meridian_flip: policy.default_auto_meridian_flip,
            rotation_deg: None,
        };

        self.catalog.plan.targets.push(target);
        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.selected_target_id = Some(id.clone());
        self.selected_target_ids.clear();
        self.selected_target_ids.insert(id.clone());
        self.save_and_notify(cx);
        id
    }

    pub fn remove_target(&mut self, target_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(pos) = self
            .catalog
            .plan
            .targets
            .iter()
            .position(|t| t.id == target_id)
        else {
            return false;
        };
        self.catalog.plan.targets.remove(pos);
        self.selected_target_ids.remove(target_id);
        if self.selected_target_id.as_deref() == Some(target_id) {
            self.selected_target_id = self
                .catalog
                .plan
                .targets
                .iter()
                .find(|t| self.selected_target_ids.contains(&t.id))
                .map(|t| t.id.clone())
                .or_else(|| {
                    let next_idx = pos.min(self.catalog.plan.targets.len().saturating_sub(1));
                    let next = self.catalog.plan.targets.get(next_idx).map(|t| t.id.clone());
                    if let Some(ref nid) = next {
                        self.selected_target_ids.insert(nid.clone());
                    }
                    next
                });
        }
        self.save_and_notify(cx);
        true
    }

    pub fn move_target(&mut self, from_idx: usize, to_idx: usize, cx: &mut Context<Self>) -> bool {
        let len = self.catalog.plan.targets.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        let lo = from_idx.min(to_idx);
        let hi = from_idx.max(to_idx);
        let anchor_start = self.catalog.plan.targets[lo].start_unix_secs;
        let gaps: Vec<i64> = (lo..hi)
            .map(|i| {
                self.catalog.plan.targets[i + 1]
                    .start_unix_secs
                    .saturating_sub(self.catalog.plan.targets[i].footprint_end_unix_secs())
                    .max(0)
            })
            .collect();

        let target = self.catalog.plan.targets.remove(from_idx);
        self.catalog.plan.targets.insert(to_idx, target);

        self.catalog.plan.targets[lo].start_unix_secs = anchor_start;
        for (offset, gap) in gaps.into_iter().enumerate() {
            let prev_end = self.catalog.plan.targets[lo + offset].footprint_end_unix_secs();
            self.catalog.plan.targets[lo + offset + 1].start_unix_secs =
                prev_end.saturating_add(gap);
        }

        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.save_and_notify(cx);
        true
    }

    pub fn move_targets_from_baseline(
        &mut self,
        base_starts: &[(String, i64)],
        requested_delta: i64,
        window_start: i64,
        window_end: i64,
        cx: &mut Context<Self>,
    ) -> bool {
        if base_starts.is_empty() {
            return false;
        }

        let moving_ids: HashSet<&str> = base_starts.iter().map(|(id, _)| id.as_str()).collect();
        let mut moving_intervals: Vec<(i64, i64)> = Vec::with_capacity(base_starts.len());
        let mut min_base_start = i64::MAX;
        let mut max_base_start = i64::MIN;
        let mut max_base_end = i64::MIN;

        for (id, base_start) in base_starts {
            let Some(target) = self.catalog.plan.targets.iter().find(|t| t.id == *id) else {
                continue;
            };
            let dur = (target.estimated_duration().as_secs() as i64).max(MIN_BLOCK_FOOTPRINT_SECS);
            let base_end = base_start.saturating_add(dur);
            moving_intervals.push((*base_start, base_end));
            min_base_start = min_base_start.min(*base_start);
            max_base_start = max_base_start.max(*base_start);
            max_base_end = max_base_end.max(base_end);
        }

        if moving_intervals.is_empty() {
            return false;
        }

        let stationary: Vec<(i64, i64)> = self
            .catalog
            .plan
            .targets
            .iter()
            .filter(|t| !moving_ids.contains(t.id.as_str()))
            .map(|t| (t.start_unix_secs, t.footprint_end_unix_secs()))
            .collect();

        let min_delta = window_start.saturating_sub(min_base_start);
        let max_start_cap = window_end
            .saturating_sub(MIN_BLOCK_FOOTPRINT_SECS)
            .saturating_sub(max_base_start);
        let max_end_cap = window_end.saturating_sub(max_base_end);
        let max_delta = if max_end_cap >= min_delta {
            max_end_cap
        } else {
            max_start_cap.max(min_delta)
        };

        let resolved_delta = snap_group_delta_without_overlap(
            &moving_intervals,
            &stationary,
            requested_delta,
            min_delta,
            max_delta,
        );

        let mut changed = false;
        for (id, base_start) in base_starts {
            if let Some(target) = self.catalog.plan.targets.iter_mut().find(|t| t.id == *id) {
                let next_start = base_start.saturating_add(resolved_delta);
                if target.start_unix_secs != next_start {
                    target.start_unix_secs = next_start;
                    changed = true;
                }
            }
        }
        if changed {
            self.catalog.plan.targets.sort_by_key(|t| t.start_unix_secs);
            self.save_and_notify(cx);
        }
        changed
    }

    pub fn nudge_target_start(
        &mut self,
        target_id: &str,
        delta_secs: i64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(base_start) = self
            .catalog
            .plan
            .targets
            .iter()
            .find(|t| t.id == target_id)
            .map(|t| t.start_unix_secs)
        else {
            return false;
        };
        let wide_min = i64::MIN / 4;
        let wide_max = i64::MAX / 4;
        self.move_targets_from_baseline(
            &[(target_id.to_string(), base_start)],
            delta_secs,
            wide_min,
            wide_max,
            cx,
        )
    }

    pub fn set_target_start_unix_secs(
        &mut self,
        target_id: &str,
        start_unix_secs: i64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(cur_start) = self
            .catalog
            .plan
            .targets
            .iter()
            .find(|t| t.id == target_id)
            .map(|t| t.start_unix_secs)
        else {
            return false;
        };
        let delta = start_unix_secs.saturating_sub(cur_start);
        self.nudge_target_start(target_id, delta, cx)
    }

    pub fn set_target_enabled(
        &mut self,
        target_id: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        if target.enabled != enabled {
            target.enabled = enabled;
            self.save_and_notify(cx);
        }
        true
    }

    pub fn set_target_policy(
        &mut self,
        target_id: &str,
        slew_to_target: bool,
        auto_meridian_flip: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        if target.slew_to_target != slew_to_target
            || target.auto_meridian_flip != auto_meridian_flip
        {
            target.slew_to_target = slew_to_target;
            target.auto_meridian_flip = auto_meridian_flip;
            self.save_and_notify(cx);
        }
        true
    }

    pub fn apply_template_to_target(
        &mut self,
        target_id: &str,
        template_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(steps) = self
            .all_templates()
            .into_iter()
            .find(|tpl| tpl.id == template_id)
            .map(|tpl| tpl.steps)
        else {
            return false;
        };
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        target.steps = steps;
        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.save_and_notify(cx);
        true
    }

    pub fn apply_template_to_selected(
        &mut self,
        template_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(steps) = self
            .all_templates()
            .into_iter()
            .find(|tpl| tpl.id == template_id)
            .map(|tpl| tpl.steps)
        else {
            return false;
        };
        let mut applied = false;
        for target in &mut self.catalog.plan.targets {
            if self.selected_target_ids.contains(&target.id)
                || self.selected_target_id.as_deref() == Some(target.id.as_str())
            {
                target.steps.clone_from(&steps);
                applied = true;
            }
        }
        if applied {
            Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
            self.save_and_notify(cx);
        }
        applied
    }

    pub fn add_step(
        &mut self,
        target_id: &str,
        step: SequenceStep,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        target.steps.push(step);
        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.save_and_notify(cx);
        true
    }

    pub fn update_step(
        &mut self,
        target_id: &str,
        step_idx: usize,
        f: impl FnOnce(&mut SequenceStep),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        let Some(step) = target.steps.get_mut(step_idx) else {
            return false;
        };
        f(step);
        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.save_and_notify(cx);
        true
    }

    pub fn remove_step(
        &mut self,
        target_id: &str,
        step_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        if step_idx >= target.steps.len() {
            return false;
        }
        target.steps.remove(step_idx);
        self.save_and_notify(cx);
        true
    }

    pub fn move_step(
        &mut self,
        target_id: &str,
        from_idx: usize,
        to_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        let len = target.steps.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        let step = target.steps.remove(from_idx);
        target.steps.insert(to_idx, step);
        self.save_and_notify(cx);
        true
    }

    pub fn duplicate_step(
        &mut self,
        target_id: &str,
        step_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self
            .catalog
            .plan
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
        else {
            return false;
        };
        let Some(step) = target.steps.get(step_idx).cloned() else {
            return false;
        };
        target.steps.insert(step_idx + 1, step);
        Self::resolve_overlaps_in_plan(&mut self.catalog.plan);
        self.save_and_notify(cx);
        true
    }

    pub fn save_template_from_target(
        &mut self,
        target_id: &str,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let steps = self
            .catalog
            .plan
            .targets
            .iter()
            .find(|t| t.id == target_id)?
            .steps
            .clone();
        let name_str = name.into();
        let clean_name = if name_str.trim().is_empty() {
            "Custom Template".to_string()
        } else {
            name_str.trim().to_string()
        };
        let id = next_unique_id(&sanitize_slug(&clean_name));
        self.catalog.user_templates.push(SequenceTemplate {
            id: id.clone(),
            name: clean_name,
            builtin: false,
            steps,
        });
        self.save_and_notify(cx);
        Some(id)
    }

    pub fn delete_user_template(&mut self, template_id: &str, cx: &mut Context<Self>) -> bool {
        let before = self.catalog.user_templates.len();
        self.catalog.user_templates.retain(|t| t.id != template_id);
        if self.catalog.user_templates.len() == before {
            false
        } else {
            self.save_and_notify(cx);
            true
        }
    }

    pub fn update_policy(&mut self, f: impl FnOnce(&mut NightPlanPolicy), cx: &mut Context<Self>) {
        f(&mut self.catalog.plan.policy);
        self.save_and_notify(cx);
    }

    /// Counts planned targets within 0.1 degrees angular separation on the celestial sphere.
    pub fn count_targets_near(&self, ra_deg: f64, dec_deg: f64) -> usize {
        if !ra_deg.is_finite() || !dec_deg.is_finite() {
            return 0;
        }
        let target_vec = canonical_spherical_to_cartesian(
            Angle::from_degrees(ra_deg),
            Angle::from_degrees(dec_deg),
        )
        .normalize_or_zero();
        let cos_threshold = 0.1_f64.to_radians().cos();

        self.catalog
            .plan
            .targets
            .iter()
            .filter(|t| {
                let v = canonical_spherical_to_cartesian(
                    Angle::from_degrees(t.ra_deg),
                    Angle::from_degrees(t.dec_deg),
                )
                .normalize_or_zero();
                target_vec.dot(v) >= cos_threshold
            })
            .count()
    }

    pub fn request_show_in_explore(&mut self, target_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.catalog.plan.targets.iter().find(|t| t.id == target_id) else {
            return false;
        };
        self.explore_nonce = self.explore_nonce.wrapping_add(1);
        self.explore_request = Some(ExploreTargetRequest {
            nonce: self.explore_nonce,
            name: target.name.clone(),
            ra_deg: target.ra_deg,
            dec_deg: target.dec_deg,
        });
        cx.notify();
        true
    }

    pub fn explore_request(&self) -> Option<&ExploreTargetRequest> {
        self.explore_request.as_ref()
    }

    fn save_and_notify(&mut self, cx: &mut Context<Self>) {
        self.plan_revision = self.plan_revision.wrapping_add(1);
        let store = self.store.clone();
        let catalog = self.catalog.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(move || {
                if let Err(err) = store.save(&catalog) {
                    warn!("Failed to persist night plan: {err}");
                }
            });
        } else if let Err(err) = store.save(&catalog) {
            warn!("Failed to persist night plan: {err}");
        }
        cx.notify();
    }
}

/// Returns `true` when an imaging sequence/plan is actively running (unlocked when idle or `Paused`).
pub fn is_session_locked(cx: &App) -> bool {
    cx.try_global::<GlobalDaemonState>().is_some_and(|g| {
        g.0.read(cx)
            .state
            .sequence
            .as_ref()
            .is_some_and(|seq| seq.phase.is_active() && seq.phase != SequencePhase::Paused)
    })
}

pub fn format_duration_compact(dur: Duration) -> String {
    let total_secs = dur.as_secs();
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else {
        format!("{mins}m")
    }
}

pub fn format_unix_hm(unix_secs: i64, utc_offset_secs: i32) -> String {
    let Ok(ts) = Timestamp::from_second(unix_secs) else {
        return "--:--".to_string();
    };
    let Ok(offset) = Offset::from_seconds(utc_offset_secs) else {
        return "--:--".to_string();
    };
    let dt = ts.to_zoned(offset.to_time_zone()).datetime();
    format!("{:02}:{:02}", dt.hour(), dt.minute())
}

/// Parses `"HH:MM"` / `"H:MM"` / `"HHMM"` / `"HH"` against the observation night (`night_date`:
/// `>= 12:00` on `night_date`, `< 12:00` on `night_date + 1d`, or closest match to `reference_unix_secs`).
pub fn parse_hm_to_night_unix_secs(
    input: &str,
    night_date: &str,
    utc_offset_secs: i32,
    reference_unix_secs: i64,
) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (hour, minute) = if trimmed.contains(':') {
        let mut parts = trimmed.split(':');
        let h_str = parts.next()?.trim();
        let m_str = parts.next()?.trim();
        let s_str = parts.next();
        if parts.next().is_some()
            || h_str.is_empty()
            || m_str.is_empty()
            || !h_str.bytes().all(|b| b.is_ascii_digit())
            || !m_str.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        if let Some(sec_raw) = s_str {
            let sec_trim = sec_raw.trim();
            if sec_trim.is_empty() || !sec_trim.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if sec_trim.parse::<u32>().ok()? >= 60 {
                return None;
            }
        }
        (h_str.parse::<u32>().ok()?, m_str.parse::<u32>().ok()?)
    } else if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        match trimmed.len() {
            1 | 2 => (trimmed.parse::<u32>().ok()?, 0),
            4 => (
                trimmed[..2].parse::<u32>().ok()?,
                trimmed[2..].parse::<u32>().ok()?,
            ),
            _ => return None,
        }
    } else {
        return None;
    };

    if hour >= 24 || minute >= 60 {
        return None;
    }

    let offset = Offset::from_seconds(utc_offset_secs).ok()?;
    let tz = offset.to_time_zone();

    if let Ok(evening_date) = night_date.trim().parse::<jiff::civil::Date>() {
        let target_date = if hour >= 12 {
            evening_date
        } else {
            evening_date.checked_add(Span::new().days(1)).ok()?
        };
        let dt = target_date.at(hour as i8, minute as i8, 0, 0);
        return dt.to_zoned(tz).ok().map(|z| z.timestamp().as_second());
    }

    let base_date = Timestamp::from_second(reference_unix_secs)
        .ok()?
        .to_zoned(tz.clone())
        .date();
    let candidates = [
        base_date.checked_sub(Span::new().days(1)).ok(),
        Some(base_date),
        base_date.checked_add(Span::new().days(1)).ok(),
    ];
    candidates
        .into_iter()
        .flatten()
        .filter_map(|d| {
            d.at(hour as i8, minute as i8, 0, 0)
                .to_zoned(tz.clone())
                .ok()
                .map(|z| z.timestamp().as_second())
        })
        .min_by_key(|&ts| (ts - reference_unix_secs).abs())
}

pub fn format_radec_compact(ra_deg: f64, dec_deg: f64) -> String {
    let ra_hours = ra_deg.rem_euclid(360.0) / 15.0;
    let ra_h = ra_hours.floor() as u32;
    let ra_m = ((ra_hours - f64::from(ra_h)) * 60.0).round() as u32;
    let (ra_h, ra_m) = if ra_m >= 60 {
        ((ra_h + 1) % 24, 0)
    } else {
        (ra_h, ra_m)
    };

    let sign = if dec_deg < 0.0 { '-' } else { '+' };
    let abs_dec = dec_deg.abs();
    let dec_d = abs_dec.floor() as u32;
    let dec_m = ((abs_dec - f64::from(dec_d)) * 60.0).round() as u32;
    let (dec_d, dec_m) = if dec_m >= 60 {
        (dec_d + 1, 0)
    } else {
        (dec_d, dec_m)
    };

    format!("{ra_h:02}h{ra_m:02}m {sign}{dec_d:02}°{dec_m:02}'")
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use protocol::{CaptureParams, FrameType, StageKind, TARGET_HUE_PALETTE};

    use super::*;

    #[gpui::test]
    fn plan_model_target_and_step_lifecycle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let model = cx.new(|_cx| PlanModel::new(store.clone(), PlannerCatalog::default()));

        model.update(cx, |m, cx| {
            let id1 = m.add_target(
                "M31",
                10.68,
                41.27,
                Some("2026-11-26".to_string()),
                1_700_000_000,
                None,
                cx,
            );
            let id2 = m.add_target(
                "M42",
                83.82,
                -5.39,
                Some("2026-11-26".to_string()),
                1_700_000_000,
                Some("dslr-broadband"),
                cx,
            );

            assert_eq!(m.plan().targets.len(), 2);
            assert_eq!(m.plan().targets[0].color_hue_deg, TARGET_HUE_PALETTE[0]);
            assert_eq!(m.plan().targets[1].color_hue_deg, TARGET_HUE_PALETTE[1]);
            // Second target starts at first target's footprint_end_unix_secs
            assert_eq!(
                m.plan().targets[1].start_unix_secs,
                m.plan().targets[0].footprint_end_unix_secs()
            );

            // Step CRUD updates derived duration and pushes overlapping subsequent targets forward
            let before_dur = m.plan().targets[0].estimated_duration();
            assert!(m.add_step(
                &id1,
                SequenceStep {
                    enabled: true,
                    kind: StageKind::Capture,
                    frame_type: FrameType::Dark,
                    count: 5,
                    params: CaptureParams {
                        exposure: Duration::from_secs(10),
                        iso: None,
                    },
                    delay_between_frames: Duration::ZERO,
                },
                cx,
            ));
            assert_eq!(
                m.plan().targets[0].estimated_duration(),
                before_dur + Duration::from_secs(50)
            );
            assert!(
                m.plan().targets[1].start_unix_secs >= m.plan().targets[0].footprint_end_unix_secs()
            );

            assert!(m.duplicate_step(&id1, 1, cx));
            assert_eq!(m.plan().targets[0].steps.len(), 3);
            assert!(m.remove_step(&id1, 2, cx));
            assert_eq!(m.plan().targets[0].steps.len(), 2);

            // Template save / apply / delete
            let tpl_id = m
                .save_template_from_target(&id1, "Saved Setup", cx)
                .expect("template saved");
            assert!(m.apply_template_to_target(&id2, &tpl_id, cx));
            assert_eq!(m.plan().targets[1].steps, m.plan().targets[0].steps);
            assert!(m.delete_user_template(&tpl_id, cx));

            // Explore handoff
            assert!(m.request_show_in_explore(&id1, cx));
            let req = m.explore_request().expect("explore request emitted");
            assert_eq!(req.nonce, 1);
            assert_eq!(req.name, "M31");

            // Remove target
            assert!(m.remove_target(&id2, cx));
            assert_eq!(m.plan().targets.len(), 1);
        });

        let persisted: PlannerCatalog = store.load().expect("catalog saved");
        assert_eq!(persisted.plan.targets.len(), 1);
    }

    #[gpui::test]
    fn multi_selection_and_non_overlapping_group_move(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let model = cx.new(|_cx| PlanModel::new(store, PlannerCatalog::default()));

        model.update(cx, |m, cx| {
            let id1 = m.add_target("T1", 10.0, 20.0, None, 10_000, None, cx);
            let id2 = m.add_target("T2", 20.0, 20.0, None, 10_000, None, cx);
            let id3 = m.add_target("T3", 30.0, 20.0, None, 10_000, None, cx);

            m.select_target(Some(id1.clone()), cx);
            assert_eq!(m.selected_target_ids().len(), 1);
            m.toggle_target_selection(&id2, cx);
            assert_eq!(m.selected_target_ids().len(), 2);
            assert!(m.is_target_selected(&id1));
            assert!(m.is_target_selected(&id2));

            let s1 = m.plan().targets[0].start_unix_secs;
            let s2 = m.plan().targets[1].start_unix_secs;
            let s3_start = m.plan().targets[2].start_unix_secs;
            let s3_end = m.plan().targets[2].footprint_end_unix_secs();

            // Drag T1+T2 across stationary T3 -> snaps cleanly past s3_end while preserving spacing
            let spacing = s2 - s1;
            m.move_targets_from_baseline(
                &[(id1.clone(), s1), (id2.clone(), s2)],
                (s3_start - s1) + 120,
                9_000,
                20_000,
                cx,
            );
            let ordered_ids: Vec<String> = m.plan().targets.iter().map(|t| t.id.clone()).collect();
            assert_eq!(ordered_ids, vec![id3.clone(), id1.clone(), id2.clone()]);
            let new_s1 = m
                .plan()
                .targets
                .iter()
                .find(|t| t.id == id1)
                .unwrap()
                .start_unix_secs;
            let new_s2 = m
                .plan()
                .targets
                .iter()
                .find(|t| t.id == id2)
                .unwrap()
                .start_unix_secs;
            assert!(s3_start <= new_s1);
            assert_eq!(new_s2 - new_s1, spacing);
            assert!(new_s1 >= s3_end || new_s2 <= s3_start);
            assert_eq!(m.plan().overlapping_target_indices(), [] as [(usize, usize); 0]);
        });
    }

    #[gpui::test]
    fn targets_stay_chronologically_ordered_on_load_edit_and_move(cx: &mut TestAppContext) {
        let make_target = |id: &str, name: &str, start_unix_secs: i64| PlannedTarget {
            id: id.to_string(),
            name: name.to_string(),
            ra_deg: 10.0,
            dec_deg: 20.0,
            color_hue_deg: 205.0,
            enabled: true,
            start_unix_secs,
            steps: vec![SequenceStep::capture(
                FrameType::Light,
                5,
                Duration::from_secs(60),
                Some(800),
                Duration::ZERO,
            )],
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: None,
        };

        let mut catalog = PlannerCatalog::default();
        catalog.plan.targets = vec![
            make_target("venus", "Venus", 5_000),
            make_target("mars", "Mars", 3_000),
            make_target("saturn", "Saturn", 1_000),
        ];

        let store = DocumentStore::in_memory();
        let model = cx.new(|_cx| PlanModel::new(store, catalog));

        model.update(cx, |m, cx| {
            let order_on_load: Vec<&str> =
                m.plan().targets.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(order_on_load, vec!["saturn", "mars", "venus"]);

            // Move Venus earlier than Mars via start-time edit -> reorders chronologically
            assert!(m.set_target_start_unix_secs("venus", 2_000, cx));
            let order_after_edit: Vec<&str> =
                m.plan().targets.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(order_after_edit, vec!["saturn", "venus", "mars"]);
            assert_eq!(m.plan().overlapping_target_indices(), [] as [(usize, usize); 0]);

            // Space Mars cleanly at 4000 and swap indices 1..=2 via move_target; saturn at index 0 stays unaffected.
            assert!(m.set_target_start_unix_secs("mars", 4_000, cx));
            let anchor_start = m.plan().targets[1].start_unix_secs;
            let mars_end_before = m.plan().targets[2].footprint_end_unix_secs();
            let gap_before = m.plan().targets[2].start_unix_secs
                - m.plan().targets[1].footprint_end_unix_secs();
            let saturn_start_before = m.plan().targets[0].start_unix_secs;

            assert!(m.move_target(2, 1, cx));
            let order_after_move: Vec<&str> =
                m.plan().targets.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(order_after_move, vec!["saturn", "mars", "venus"]);
            assert_eq!(m.plan().targets[0].start_unix_secs, saturn_start_before);
            assert_eq!(m.plan().targets[1].start_unix_secs, anchor_start);
            assert_eq!(
                m.plan().targets[2].start_unix_secs
                    - m.plan().targets[1].footprint_end_unix_secs(),
                gap_before
            );
            assert_eq!(m.plan().targets[2].footprint_end_unix_secs(), mars_end_before);
            assert_eq!(m.plan().overlapping_target_indices(), [] as [(usize, usize); 0]);
        });
    }

    #[gpui::test]
    fn count_targets_near_handles_ra_wrap(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let model = cx.new(|_cx| PlanModel::new(store, PlannerCatalog::default()));

        model.update(cx, |m, cx| {
            m.add_target("Wrap", 359.98, 12.0, None, 1000, None, cx);
            assert_eq!(m.count_targets_near(0.01, 12.0), 1);
            assert_eq!(m.count_targets_near(180.0, 12.0), 0);
        });
    }

    #[test]
    fn parse_hm_resolves_evening_and_post_midnight_dates() {
        let utc_plus_1 = 3600;
        let night = "2026-11-26";

        let evening_ts =
            parse_hm_to_night_unix_secs("22:30", night, utc_plus_1, 0).expect("valid evening");
        assert_eq!(format_unix_hm(evening_ts, utc_plus_1), "22:30");

        let midnight_ts =
            parse_hm_to_night_unix_secs("00:00", night, utc_plus_1, 0).expect("valid midnight");
        assert_eq!(format_unix_hm(midnight_ts, utc_plus_1), "00:00");
        assert_eq!(midnight_ts - evening_ts, 90 * 60);

        let morning_ts =
            parse_hm_to_night_unix_secs("02:15", night, utc_plus_1, 0).expect("valid morning");
        assert_eq!(format_unix_hm(morning_ts, utc_plus_1), "02:15");
        assert_eq!(morning_ts - midnight_ts, (2 * 60 + 15) * 60);

        let compact_hm =
            parse_hm_to_night_unix_secs("6:25", night, utc_plus_1, 0).expect("compact h:mm");
        assert_eq!(format_unix_hm(compact_hm, utc_plus_1), "06:25");

        let four_digit =
            parse_hm_to_night_unix_secs("0625", night, utc_plus_1, 0).expect("4-digit hhmm");
        assert_eq!(four_digit, compact_hm);

        let hour_only =
            parse_hm_to_night_unix_secs("21", night, utc_plus_1, 0).expect("hour only");
        assert_eq!(format_unix_hm(hour_only, utc_plus_1), "21:00");

        assert!(parse_hm_to_night_unix_secs("", night, utc_plus_1, 0).is_none());
        assert!(parse_hm_to_night_unix_secs("25:00", night, utc_plus_1, 0).is_none());
        assert!(parse_hm_to_night_unix_secs("12:60", night, utc_plus_1, 0).is_none());
        assert!(parse_hm_to_night_unix_secs("abc", night, utc_plus_1, 0).is_none());
    }
}
