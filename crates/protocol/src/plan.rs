use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capture::{CaptureParams, FrameType, ImagingSequence, SaveLocationConfig, SequenceStep};

pub const TARGET_HUE_PALETTE: [f32; 8] = [205.0, 32.0, 145.0, 285.0, 345.0, 52.0, 175.0, 240.0];

fn default_true() -> bool {
    true
}

fn default_plan_id() -> String {
    "tonight".to_string()
}

fn default_plan_name() -> String {
    "Tonight".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedTarget {
    pub id: String,
    pub name: String,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub color_hue_deg: f32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub start_unix_secs: i64,
    #[serde(default)]
    pub steps: Vec<SequenceStep>,
    #[serde(default = "default_true")]
    pub slew_to_target: bool,
    #[serde(default = "default_true")]
    pub auto_meridian_flip: bool,
    #[serde(default)]
    pub rotation_deg: Option<f64>,
}

impl PlannedTarget {
    pub fn estimated_duration(&self) -> Duration {
        self.steps
            .iter()
            .fold(Duration::ZERO, |acc, step| acc.saturating_add(step.estimated_duration()))
    }

    pub fn end_unix_secs(&self) -> i64 {
        self.start_unix_secs
            .saturating_add(self.estimated_duration().as_secs() as i64)
    }

    pub fn total_frames(&self) -> u32 {
        self.steps
            .iter()
            .filter(|step| step.enabled)
            .fold(0u32, |acc, step| acc.saturating_add(step.count))
    }

    pub fn to_imaging_sequence(
        &self,
        sequence_id: u64,
        save_config: SaveLocationConfig,
    ) -> ImagingSequence {
        ImagingSequence {
            id: sequence_id,
            target_name: self.name.clone(),
            steps: self.steps.clone(),
            save_config,
            auto_meridian_flip: self.auto_meridian_flip,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightPlanPolicy {
    #[serde(default = "default_true")]
    pub park_at_end: bool,
    #[serde(default = "default_true")]
    pub default_auto_meridian_flip: bool,
    #[serde(default = "default_true")]
    pub default_slew_to_target: bool,
}

impl Default for NightPlanPolicy {
    fn default() -> Self {
        Self {
            park_at_end: true,
            default_auto_meridian_flip: true,
            default_slew_to_target: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NightPlan {
    #[serde(default = "default_plan_id")]
    pub id: String,
    #[serde(default = "default_plan_name")]
    pub name: String,
    #[serde(default)]
    pub night_date: String,
    #[serde(default)]
    pub targets: Vec<PlannedTarget>,
    #[serde(default)]
    pub policy: NightPlanPolicy,
    #[serde(default)]
    pub save_config: SaveLocationConfig,
}

impl Default for NightPlan {
    fn default() -> Self {
        Self {
            id: default_plan_id(),
            name: default_plan_name(),
            night_date: String::new(),
            targets: Vec::new(),
            policy: NightPlanPolicy::default(),
            save_config: SaveLocationConfig::default(),
        }
    }
}

impl NightPlan {
    pub fn next_palette_hue(&self) -> f32 {
        for &hue in &TARGET_HUE_PALETTE {
            let used = self.targets.iter().any(|t| {
                let diff = (t.color_hue_deg - hue).abs().rem_euclid(360.0);
                diff.min(360.0 - diff) < 15.0
            });
            if !used {
                return hue;
            }
        }
        TARGET_HUE_PALETTE[self.targets.len() % TARGET_HUE_PALETTE.len()]
    }

    pub fn overlapping_target_indices(&self) -> Vec<(usize, usize)> {
        let mut overlaps = Vec::new();
        for i in 0..self.targets.len() {
            let a = &self.targets[i];
            if !a.enabled || a.estimated_duration().is_zero() {
                continue;
            }
            let a_end = a.end_unix_secs();
            for j in (i + 1)..self.targets.len() {
                let b = &self.targets[j];
                if !b.enabled || b.estimated_duration().is_zero() {
                    continue;
                }
                let b_end = b.end_unix_secs();
                if a.start_unix_secs < b_end && b.start_unix_secs < a_end {
                    overlaps.push((i, j));
                }
            }
        }
        overlaps
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NightPlanRunRequest {
    pub run_id: u64,
    pub plan: NightPlan,
    #[serde(default)]
    pub start_immediately: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceTemplate {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub steps: Vec<SequenceStep>,
}

pub fn builtin_sequence_templates() -> Vec<SequenceTemplate> {
    vec![
        SequenceTemplate {
            id: "dslr-broadband".to_string(),
            name: "DSLR Broadband".to_string(),
            builtin: true,
            steps: vec![
                SequenceStep {
                    enabled: true,
                    frame_type: FrameType::Light,
                    count: 30,
                    params: CaptureParams {
                        exposure: Duration::from_secs(120),
                        iso: Some(800),
                    },
                    delay_between_frames: Duration::from_secs(2),
                },
                SequenceStep {
                    enabled: true,
                    frame_type: FrameType::Dark,
                    count: 10,
                    params: CaptureParams {
                        exposure: Duration::from_secs(120),
                        iso: Some(800),
                    },
                    delay_between_frames: Duration::ZERO,
                },
            ],
        },
        SequenceTemplate {
            id: "quick-test".to_string(),
            name: "Quick Test".to_string(),
            builtin: true,
            steps: vec![SequenceStep {
                enabled: true,
                frame_type: FrameType::Light,
                count: 5,
                params: CaptureParams {
                    exposure: Duration::from_secs(2),
                    iso: Some(800),
                },
                delay_between_frames: Duration::ZERO,
            }],
        },
        SequenceTemplate {
            id: "calibration".to_string(),
            name: "Calibration (Dark + Bias)".to_string(),
            builtin: true,
            steps: vec![
                SequenceStep {
                    enabled: true,
                    frame_type: FrameType::Dark,
                    count: 15,
                    params: CaptureParams {
                        exposure: Duration::from_secs(60),
                        iso: Some(800),
                    },
                    delay_between_frames: Duration::ZERO,
                },
                SequenceStep {
                    enabled: true,
                    frame_type: FrameType::Bias,
                    count: 20,
                    params: CaptureParams {
                        exposure: Duration::from_millis(10),
                        iso: Some(800),
                    },
                    delay_between_frames: Duration::ZERO,
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_step(enabled: bool, count: u32, exposure_secs: u64, delay_secs: u64) -> SequenceStep {
        SequenceStep {
            enabled,
            frame_type: FrameType::Light,
            count,
            params: CaptureParams {
                exposure: Duration::from_secs(exposure_secs),
                iso: Some(800),
            },
            delay_between_frames: Duration::from_secs(delay_secs),
        }
    }

    #[test]
    fn planned_target_duration_and_sequence_conversion() {
        let target = PlannedTarget {
            id: "m31".to_string(),
            name: "M31".to_string(),
            ra_deg: 10.6847,
            dec_deg: 41.2687,
            color_hue_deg: 205.0,
            enabled: true,
            start_unix_secs: 1_000,
            steps: vec![
                sample_step(true, 10, 60, 2), // 10*60 + 9*2 = 618s
                sample_step(false, 5, 120, 0),
            ],
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: None,
        };

        assert_eq!(target.estimated_duration(), Duration::from_secs(618));
        assert_eq!(target.end_unix_secs(), 1_618);
        assert_eq!(target.total_frames(), 10);

        let seq = target.to_imaging_sequence(42, SaveLocationConfig::default());
        assert_eq!(seq.id, 42);
        assert_eq!(seq.target_name, "M31");
        assert_eq!(seq.total_frames(), 10);
        assert!(seq.auto_meridian_flip);
    }

    #[test]
    fn night_plan_palette_and_overlap_detection() {
        let mut plan = NightPlan::default();
        assert_eq!(plan.next_palette_hue(), TARGET_HUE_PALETTE[0]);

        plan.targets.push(PlannedTarget {
            id: "t1".to_string(),
            name: "M31".to_string(),
            ra_deg: 10.0,
            dec_deg: 41.0,
            color_hue_deg: TARGET_HUE_PALETTE[0],
            enabled: true,
            start_unix_secs: 1000,
            steps: vec![sample_step(true, 10, 60, 0)], // 1000..1600
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: None,
        });
        assert_eq!(plan.next_palette_hue(), TARGET_HUE_PALETTE[1]);

        // Overlaps t1 (1400 < 1600)
        plan.targets.push(PlannedTarget {
            id: "t2".to_string(),
            name: "M42".to_string(),
            ra_deg: 83.8,
            dec_deg: -5.4,
            color_hue_deg: TARGET_HUE_PALETTE[1],
            enabled: true,
            start_unix_secs: 1400,
            steps: vec![sample_step(true, 5, 60, 0)], // 1400..1700
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: None,
        });

        // Adjacent / non-overlapping (starts at 1700)
        plan.targets.push(PlannedTarget {
            id: "t3".to_string(),
            name: "NGC 7000".to_string(),
            ra_deg: 314.7,
            dec_deg: 44.3,
            color_hue_deg: TARGET_HUE_PALETTE[2],
            enabled: true,
            start_unix_secs: 1700,
            steps: vec![sample_step(true, 5, 60, 0)],
            slew_to_target: true,
            auto_meridian_flip: true,
            rotation_deg: None,
        });

        assert_eq!(plan.overlapping_target_indices(), vec![(0, 1)]);

        let json = serde_json::to_string(&plan).unwrap();
        let decoded: NightPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, plan);

        let req = NightPlanRunRequest {
            run_id: 99,
            plan,
            start_immediately: true,
        };
        let req_json = serde_json::to_string(&req).unwrap();
        let req_decoded: NightPlanRunRequest = serde_json::from_str(&req_json).unwrap();
        assert_eq!(req_decoded, req);
    }
}
