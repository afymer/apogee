use std::collections::HashSet;
use std::time::Duration;

use protocol::{
    FrameType, NightPlan, SequenceStep, SequenceTemplate, builtin_sequence_templates,
};
use serde::{Deserialize, Serialize};

use crate::store::Persistable;

pub const PLAN_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    PLAN_SCHEMA_VERSION
}

fn sanitize_steps(steps: &mut [SequenceStep]) {
    for step in steps.iter_mut() {
        if step.params.exposure.is_zero() {
            step.params.exposure = match step.frame_type {
                FrameType::Bias => Duration::from_millis(10),
                FrameType::Light | FrameType::Dark | FrameType::Flat => Duration::from_secs(1),
            };
        }
        if step.params.iso == Some(0) {
            step.params.iso = None;
        }
    }
}

/// Persisted single-night plan and user sequence templates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannerCatalog {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub plan: NightPlan,
    #[serde(default)]
    pub user_templates: Vec<SequenceTemplate>,
}

impl Default for PlannerCatalog {
    fn default() -> Self {
        Self {
            schema_version: PLAN_SCHEMA_VERSION,
            plan: NightPlan::default(),
            user_templates: Vec::new(),
        }
    }
}

impl PlannerCatalog {
    pub fn all_templates(&self) -> Vec<SequenceTemplate> {
        let mut all = builtin_sequence_templates();
        all.extend(self.user_templates.iter().cloned());
        all
    }
}

impl Persistable for PlannerCatalog {
    const DOCUMENT_KEY: &'static str = "night_plan";
    const SCHEMA_VERSION: u32 = PLAN_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn normalize_after_load(&mut self) {
        self.schema_version = PLAN_SCHEMA_VERSION;

        if self.plan.id.trim().is_empty() {
            self.plan.id = "tonight".to_string();
        }
        if self.plan.name.trim().is_empty() {
            self.plan.name = "Tonight".to_string();
        }

        let mut seen_target_ids = HashSet::new();
        for (idx, target) in self.plan.targets.iter_mut().enumerate() {
            if target.id.trim().is_empty() || !seen_target_ids.insert(target.id.clone()) {
                let mut candidate = format!("target-{}", idx + 1);
                while !seen_target_ids.insert(candidate.clone()) {
                    candidate.push('x');
                }
                target.id = candidate;
            }
            if target.name.trim().is_empty() {
                target.name = "Target".to_string();
            }
            target.ra_deg = if target.ra_deg.is_finite() {
                target.ra_deg.rem_euclid(360.0)
            } else {
                0.0
            };
            target.dec_deg = if target.dec_deg.is_finite() {
                target.dec_deg.clamp(-90.0, 90.0)
            } else {
                0.0
            };
            target.color_hue_deg = if target.color_hue_deg.is_finite() {
                target.color_hue_deg.rem_euclid(360.0)
            } else {
                205.0
            };
            target.rotation_deg = target
                .rotation_deg
                .filter(|r| r.is_finite())
                .map(|r| r.rem_euclid(360.0));
            sanitize_steps(&mut target.steps);
        }

        let mut seen_template_ids = HashSet::new();
        self.user_templates.retain_mut(|tpl| {
            let trimmed_id = tpl.id.trim();
            if trimmed_id.is_empty() {
                return false;
            }
            if trimmed_id.len() != tpl.id.len() {
                tpl.id = trimmed_id.to_string();
            }
            if !seen_template_ids.insert(tpl.id.clone()) {
                return false;
            }
            tpl.builtin = false;
            if tpl.name.trim().is_empty() {
                tpl.name = tpl.id.clone();
            }
            sanitize_steps(&mut tpl.steps);
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use protocol::{CaptureParams, PlannedTarget, TARGET_HUE_PALETTE};

    use super::*;
    use crate::{DocumentStore, PersistenceError};

    #[test]
    fn planner_catalog_round_trip_and_builtin_templates() {
        let store = DocumentStore::in_memory();
        let mut catalog = PlannerCatalog::default();
        catalog.plan.night_date = "2026-11-26".to_string();
        catalog.plan.targets.push(PlannedTarget {
            id: "m31".to_string(),
            name: "M31".to_string(),
            ra_deg: 10.6847,
            dec_deg: 41.2687,
            color_hue_deg: TARGET_HUE_PALETTE[0],
            enabled: true,
            start_unix_secs: 1_700_000_000,
            steps: vec![SequenceStep {
                enabled: true,
                frame_type: FrameType::Light,
                count: 10,
                params: CaptureParams {
                    exposure: Duration::from_secs(60),
                    iso: Some(800),
                },
                delay_between_frames: Duration::from_secs(1),
            }],
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: Some(15.0),
        });
        catalog.user_templates.push(SequenceTemplate {
            id: "my-ha".to_string(),
            name: "My Ha".to_string(),
            builtin: false,
            steps: vec![],
        });

        store.save(&catalog).expect("save succeeds");
        let loaded: PlannerCatalog = store.load().expect("load succeeds");
        assert_eq!(loaded, catalog);
        assert_eq!(loaded.all_templates().len(), 4);
    }

    #[test]
    fn planner_catalog_normalizes_invalid_fields_and_rejects_future_schema() {
        let store = DocumentStore::in_memory();
        let future = PlannerCatalog {
            schema_version: PLAN_SCHEMA_VERSION + 5,
            ..PlannerCatalog::default()
        };
        store.save(&future).expect("save succeeds");
        let err = store.load::<PlannerCatalog>().unwrap_err();
        assert!(matches!(
            err,
            PersistenceError::UnsupportedSchemaVersion { .. }
        ));

        let mut dirty = PlannerCatalog::default();
        dirty.plan.id = " ".to_string();
        dirty.plan.name = String::new();
        dirty.plan.targets.push(PlannedTarget {
            id: String::new(),
            name: " ".to_string(),
            ra_deg: -30.0,
            dec_deg: 120.0,
            color_hue_deg: f32::NAN,
            enabled: true,
            start_unix_secs: 1000,
            steps: vec![SequenceStep {
                enabled: true,
                frame_type: FrameType::Light,
                count: 1,
                params: CaptureParams {
                    exposure: Duration::ZERO,
                    iso: Some(0),
                },
                delay_between_frames: Duration::ZERO,
            }],
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: Some(f64::INFINITY),
        });
        dirty.user_templates.push(SequenceTemplate {
            id: " ".to_string(),
            name: "empty id".to_string(),
            builtin: true,
            steps: vec![],
        });
        dirty.user_templates.push(SequenceTemplate {
            id: "dup".to_string(),
            name: String::new(),
            builtin: true,
            steps: vec![],
        });
        dirty.user_templates.push(SequenceTemplate {
            id: "dup".to_string(),
            name: "duplicate".to_string(),
            builtin: false,
            steps: vec![],
        });

        dirty.normalize_after_load();
        assert_eq!(dirty.plan.id, "tonight");
        assert_eq!(dirty.plan.name, "Tonight");
        let t = &dirty.plan.targets[0];
        assert_ne!(t.id, "");
        assert_eq!(t.name, "Target");
        assert!((t.ra_deg - 330.0).abs() < 1e-9);
        assert_eq!(t.dec_deg, 90.0);
        assert_eq!(t.color_hue_deg, 205.0);
        assert_eq!(t.rotation_deg, None);
        assert_eq!(t.steps[0].params.exposure, Duration::from_secs(1));
        assert_eq!(t.steps[0].params.iso, None);
        assert_eq!(dirty.user_templates.len(), 1);
        assert!(!dirty.user_templates[0].builtin);
        assert_eq!(dirty.user_templates[0].name, "dup");
    }
}
