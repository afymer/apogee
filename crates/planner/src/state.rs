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
    NightPlan, NightPlanPolicy, PlannedTarget, SequencePhase, SequenceStep, SequenceTemplate,
    builtin_sequence_templates,
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
    explore_request: Option<ExploreTargetRequest>,
    explore_nonce: u64,
    plan_revision: u64,
}

impl PlanModel {
    pub fn new(store: DocumentStore, catalog: PlannerCatalog) -> Self {
        let selected_target_id = catalog.plan.targets.first().map(|t| t.id.clone());
        Self {
            store,
            catalog,
            selected_target_id,
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

    pub fn selected_target(&self) -> Option<&PlannedTarget> {
        let id = self.selected_target_id.as_deref()?;
        self.catalog.plan.targets.iter().find(|t| t.id == id)
    }

    pub fn select_target(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected_target_id != id {
            self.selected_target_id = id;
            cx.notify();
        }
    }

    pub fn set_night_date(&mut self, night_date: String, cx: &mut Context<Self>) {
        if self.catalog.plan.night_date != night_date {
            self.catalog.plan.night_date = night_date;
            self.save_and_notify(cx);
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
            .map(PlannedTarget::end_unix_secs)
            .max()
            .unwrap_or(default_start_unix_secs);

        let templates = self.all_templates();
        let selected_tpl_id = template_id.unwrap_or("quick-test");
        let steps = templates
            .iter()
            .find(|t| t.id == selected_tpl_id)
            .or_else(|| {
                builtin_sequence_templates()
                    .into_iter()
                    .find(|t| t.id == "quick-test")
                    .as_ref()
                    .map(|_| unreachable!())
            })
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
        self.selected_target_id = Some(id.clone());
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
        if self.selected_target_id.as_deref() == Some(target_id) {
            let next_idx = pos.min(self.catalog.plan.targets.len().saturating_sub(1));
            self.selected_target_id = self.catalog.plan.targets.get(next_idx).map(|t| t.id.clone());
        }
        self.save_and_notify(cx);
        true
    }

    pub fn move_target(&mut self, from_idx: usize, to_idx: usize, cx: &mut Context<Self>) -> bool {
        let len = self.catalog.plan.targets.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        let target = self.catalog.plan.targets.remove(from_idx);
        self.catalog.plan.targets.insert(to_idx, target);
        self.save_and_notify(cx);
        true
    }

    pub fn set_target_start_unix_secs(
        &mut self,
        target_id: &str,
        start_unix_secs: i64,
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
        if target.start_unix_secs != start_unix_secs {
            target.start_unix_secs = start_unix_secs;
            self.save_and_notify(cx);
        }
        true
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
        self.save_and_notify(cx);
        true
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
    use std::time::Duration;

    use gpui::TestAppContext;
    use protocol::{CaptureParams, FrameType, TARGET_HUE_PALETTE};

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
            // Second target starts immediately at first target's derived end_unix_secs
            assert_eq!(
                m.plan().targets[1].start_unix_secs,
                m.plan().targets[0].end_unix_secs()
            );

            // Step CRUD updates derived duration
            let before_dur = m.plan().targets[0].estimated_duration();
            assert!(m.add_step(
                &id1,
                SequenceStep {
                    enabled: true,
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
    fn count_targets_near_handles_ra_wrap(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let model = cx.new(|_cx| PlanModel::new(store, PlannerCatalog::default()));

        model.update(cx, |m, cx| {
            m.add_target("Wrap", 359.98, 12.0, None, 1000, None, cx);
            assert_eq!(m.count_targets_near(0.01, 12.0), 1);
            assert_eq!(m.count_targets_near(180.0, 12.0), 0);
        });
    }
}
