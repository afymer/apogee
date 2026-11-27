use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capture::{FrameType, ImagingSequence, SaveLocationConfig, SequenceStep};

pub const TARGET_HUE_PALETTE: [f32; 8] = [205.0, 32.0, 145.0, 285.0, 345.0, 52.0, 175.0, 240.0];
pub const MIN_BLOCK_FOOTPRINT_SECS: i64 = 60;

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
        self.steps.iter().fold(Duration::ZERO, |acc, step| {
            acc.saturating_add(step.estimated_duration())
        })
    }

    pub fn end_unix_secs(&self) -> i64 {
        self.start_unix_secs
            .saturating_add(self.estimated_duration().as_secs() as i64)
    }

    pub fn footprint_end_unix_secs(&self) -> i64 {
        self.start_unix_secs.saturating_add(
            (self.estimated_duration().as_secs() as i64).max(MIN_BLOCK_FOOTPRINT_SECS),
        )
    }

    pub fn total_frames(&self) -> u32 {
        self.steps
            .iter()
            .fold(0u32, |acc, step| acc.saturating_add(step.frame_count()))
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

/// Resolves `requested_delta` for a rigid group of `moving` intervals `[(start, end)]`
/// against `stationary` intervals `[(start, end)]` within `[min_delta, max_delta]`.
/// Overlaps are strictly forbidden: if `requested_delta` would cause any overlap, snaps
/// flush before or after the merged forbidden region (picking the nearest valid snap).
pub fn snap_group_delta_without_overlap(
    moving: &[(i64, i64)],
    stationary: &[(i64, i64)],
    requested_delta: i64,
    min_delta: i64,
    max_delta: i64,
) -> i64 {
    if moving.is_empty() || min_delta > max_delta {
        return 0;
    }

    let req = requested_delta.clamp(min_delta, max_delta);

    let mut forbidden: Vec<(i64, i64)> = Vec::new();
    for &(m_start, m_end) in moving {
        for &(s_start, s_end) in stationary {
            let lo = s_start.saturating_sub(m_end);
            let hi = s_end.saturating_sub(m_start);
            if lo < hi {
                forbidden.push((lo, hi));
            }
        }
    }

    if forbidden.is_empty() {
        return req;
    }

    forbidden.sort_unstable_by_key(|&(lo, hi)| (lo, hi));
    let mut merged: Vec<(i64, i64)> = Vec::with_capacity(forbidden.len());
    for (lo, hi) in forbidden {
        if let Some(last) = merged.last_mut()
            && lo < last.1
        {
            last.1 = last.1.max(hi);
        } else {
            merged.push((lo, hi));
        }
    }

    let Some(&(l, r)) = merged.iter().find(|&&(l, r)| l < req && req < r) else {
        return req;
    };

    let direct_candidates = [l, r];
    let mut best_direct: Option<i64> = None;
    for cand in direct_candidates {
        if cand >= min_delta && cand <= max_delta {
            best_direct = Some(match best_direct {
                Some(prev) if (prev - req).abs() <= (cand - req).abs() => prev,
                _ => cand,
            });
        }
    }
    if let Some(best) = best_direct {
        return best;
    }

    let is_valid = |d: i64| -> bool {
        d >= min_delta && d <= max_delta && !merged.iter().any(|&(fl, fr)| fl < d && d < fr)
    };
    let mut best: Option<i64> = None;
    for cand in merged
        .iter()
        .flat_map(|&(fl, fr)| [fl, fr])
        .chain([min_delta, max_delta])
    {
        if is_valid(cand) {
            best = Some(match best {
                Some(prev) if (prev - req).abs() <= (cand - req).abs() => prev,
                _ => cand,
            });
        }
    }
    best.unwrap_or(0)
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
                SequenceStep::capture(
                    FrameType::Light,
                    30,
                    Duration::from_secs(120),
                    Some(800),
                    Duration::from_secs(2),
                ),
                SequenceStep::capture(
                    FrameType::Dark,
                    10,
                    Duration::from_secs(120),
                    Some(800),
                    Duration::ZERO,
                ),
            ],
        },
        SequenceTemplate {
            id: "quick-test".to_string(),
            name: "Quick Test".to_string(),
            builtin: true,
            steps: vec![SequenceStep::capture(
                FrameType::Light,
                5,
                Duration::from_secs(2),
                Some(800),
                Duration::ZERO,
            )],
        },
        SequenceTemplate {
            id: "calibration".to_string(),
            name: "Calibration (Dark + Bias)".to_string(),
            builtin: true,
            steps: vec![
                SequenceStep::capture(
                    FrameType::Dark,
                    15,
                    Duration::from_secs(60),
                    Some(800),
                    Duration::ZERO,
                ),
                SequenceStep::capture(
                    FrameType::Bias,
                    20,
                    Duration::from_millis(10),
                    Some(800),
                    Duration::ZERO,
                ),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureParams, StageKind};

    fn sample_step(enabled: bool, count: u32, exposure_secs: u64, delay_secs: u64) -> SequenceStep {
        SequenceStep {
            enabled,
            kind: StageKind::Capture,
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

    #[test]
    fn snap_group_delta_prevents_overlaps_and_preserves_spacing() {
        // Moving [100, 200] against stationary [300, 400]: forbidden open interval is (100, 300).
        let moving = [(100, 200)];
        let stationary = [(300, 400)];

        // Free space before obstacle
        assert_eq!(
            snap_group_delta_without_overlap(&moving, &stationary, 50, -500, 500),
            50
        );
        // Slightly inside left side (delta=150 -> [250, 350]): snaps flush before (delta=100 -> [200, 300])
        assert_eq!(
            snap_group_delta_without_overlap(&moving, &stationary, 150, -500, 500),
            100
        );
        // Past midpoint (delta=240 -> [340, 440]): snaps flush after (delta=300 -> [400, 500])
        assert_eq!(
            snap_group_delta_without_overlap(&moving, &stationary, 240, -500, 500),
            300
        );
        // When right snap exceeds max_delta=250, falls back to left snap (delta=100)
        assert_eq!(
            snap_group_delta_without_overlap(&moving, &stationary, 240, -500, 250),
            100
        );

        // Contiguous chain [300, 400] + [400, 500] merges into (100, 400): never snaps into seam
        let chain = [(300, 400), (400, 500)];
        assert_eq!(
            snap_group_delta_without_overlap(&moving, &chain, 260, -500, 500),
            400
        );

        // Multi-block group [100, 200] & [250, 350] against [400, 500]:
        // forbidden for block1 is (200, 400), for block2 is (50, 250) -> merged (50, 400)
        let group = [(100, 200), (250, 350)];
        assert_eq!(
            snap_group_delta_without_overlap(&group, &[(400, 500)], 150, -500, 500),
            50
        );
        assert_eq!(
            snap_group_delta_without_overlap(&group, &[(400, 500)], 280, -500, 500),
            400
        );
    }

    #[test]
    fn snap_group_delta_allows_exact_fit_between_surrounding_blocks() {
        let stationary = [(100, 200), (300, 400)];

        // Surrounded in [200, 300] with zero gap on both sides.
        let middle = [(200, 300)];
        assert_eq!(
            snap_group_delta_without_overlap(&middle, &stationary, 0, -500, 500),
            0
        );
        assert_eq!(
            snap_group_delta_without_overlap(&middle, &stationary, 40, -500, 500),
            0
        );
        assert_eq!(
            snap_group_delta_without_overlap(&middle, &stationary, -40, -500, 500),
            0
        );
        assert_eq!(
            snap_group_delta_without_overlap(&middle, &stationary, 160, -500, 500),
            200
        );

        // Moving block outside at [400, 500] dragging back into the exact-fit [200, 300] gap (delta -200).
        let outside = [(400, 500)];
        assert_eq!(
            snap_group_delta_without_overlap(&outside, &stationary, -200, -500, 500),
            -200
        );
        assert_eq!(
            snap_group_delta_without_overlap(&outside, &stationary, -170, -500, 500),
            -200
        );

        // A gap strictly narrower than the moving block (80s < 100s) still merges and rejects fitting inside.
        let narrower = [(100, 200), (280, 380)];
        assert_eq!(
            snap_group_delta_without_overlap(&outside, &narrower, -170, -500, 500),
            -20
        );
    }

    #[test]
    fn sequence_step_legacy_json_defaults_stage_kind_and_non_capture_stages() {
        let legacy_json = r#"{
            "frame_type": "Light",
            "count": 10,
            "params": { "exposure": { "secs": 60, "nanos": 0 }, "iso": 800 }
        }"#;
        let decoded: SequenceStep = serde_json::from_str(legacy_json).expect("legacy json parses");
        assert_eq!(decoded.kind, StageKind::Capture);
        assert!(decoded.enabled);
        assert_eq!(decoded.frame_count(), 10);
        assert_eq!(decoded.estimated_duration(), Duration::from_secs(600));
        assert_eq!(decoded.stage_label(), "Light");

        let af = SequenceStep::autofocus();
        assert_eq!(af.kind, StageKind::Autofocus);
        assert_eq!(af.frame_count(), 0);
        assert_eq!(af.estimated_duration(), Duration::ZERO);
        assert_eq!(af.stage_label(), "Autofocus");

        let ps = SequenceStep::plate_solve();
        assert_eq!(ps.kind, StageKind::PlateSolve);
        assert_eq!(ps.frame_count(), 0);
        assert_eq!(ps.estimated_duration(), Duration::ZERO);
        assert_eq!(ps.stage_label(), "Plate Solve");

        let wait = SequenceStep::wait(Duration::from_secs(45));
        assert_eq!(wait.kind, StageKind::Wait);
        assert_eq!(wait.frame_count(), 0);
        assert_eq!(wait.estimated_duration(), Duration::from_secs(45));
        assert_eq!(wait.stage_label(), "Wait");
    }
}
