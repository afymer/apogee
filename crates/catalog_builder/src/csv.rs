use std::{
    collections::{HashMap, hash_map::Entry},
    fs::File,
    io::BufReader,
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result};
use celestial::catalog::{StarMetadata, StarRecord};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{info, warn};

use crate::BuiltStar;

pub(crate) fn parse_from_csv(path: &Path, max_mag: f32) -> Result<Vec<BuiltStar>> {
    info!("Reading Gaia CSV from {}...", path.display());
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    parse_csv_reader(reader, max_mag)
}

pub(crate) fn parse_csv_bytes(bytes: &[u8], max_mag: f32) -> Result<Vec<BuiltStar>> {
    parse_csv_reader(bytes, max_mag)
}

fn sanitize_tyc(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("TYC ")
        .or_else(|| trimmed.strip_prefix("tyc "))
        .unwrap_or(trimmed)
        .trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn parse_csv_reader<R: std::io::Read>(reader: R, max_mag: f32) -> Result<Vec<BuiltStar>> {
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    let headers = csv_reader.headers()?.clone();

    let find_col = |candidates: &[&str]| -> Option<usize> {
        candidates.iter().find_map(|&c| {
            headers
                .iter()
                .position(|h| h.trim().eq_ignore_ascii_case(c))
        })
    };

    let ra_idx =
        find_col(&["ra", "ra_deg", "raj2000"]).context("Missing \"ra\" column in CSV headers")?;
    let dec_idx = find_col(&["dec", "dec_deg", "dej2000"])
        .context("Missing \"dec\" column in CSV headers")?;
    let mag_idx = find_col(&["phot_g_mean_mag", "g_mag", "mag", "phot_g_mean"])
        .context("Missing \"phot_g_mean_mag\" column in CSV headers")?;
    let color_idx = find_col(&["bp_rp", "b_v", "color", "bp_rp_color"]);
    let source_id_idx = find_col(&["source_id", "gaia_source_id"]);
    let hip_idx = find_col(&["hip", "hip_id"]);
    let hip_dist_idx = find_col(&["hip_dist", "hip_angular_distance"]);
    let tyc_idx = find_col(&["tyc", "tyc_id"]);
    let tyc_dist_idx = find_col(&["tyc_dist", "tyc_angular_distance"]);

    let mut stars: Vec<BuiltStar> = Vec::new();
    let mut source_to_idx: HashMap<u64, usize> = HashMap::new();
    let mut best_hip_dist: Vec<f64> = Vec::new();
    let mut best_tyc_dist: Vec<f64> = Vec::new();

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{elapsed_precise}] Parsed {pos} stars...")?,
    );
    pb.enable_steady_tick(Duration::from_millis(150));

    for result in csv_reader.records() {
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                warn!("Skipping invalid CSV row: {}", e);
                continue;
            }
        };

        let ra_str = record.get(ra_idx).unwrap_or("").trim();
        let dec_str = record.get(dec_idx).unwrap_or("").trim();
        let mag_str = record.get(mag_idx).unwrap_or("").trim();

        let Ok(ra_deg) = ra_str.parse::<f64>() else {
            continue;
        };
        let Ok(dec_deg) = dec_str.parse::<f64>() else {
            continue;
        };
        let Ok(mag) = mag_str.parse::<f32>() else {
            continue;
        };

        if mag > max_mag || !mag.is_finite() || !ra_deg.is_finite() || !dec_deg.is_finite() {
            continue;
        }

        let hip = hip_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<u32>().ok());
        let hip_dist = hip_dist_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(f64::INFINITY);

        let tyc = tyc_idx
            .and_then(|idx| record.get(idx))
            .and_then(sanitize_tyc);
        let tyc_dist = tyc_dist_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(f64::INFINITY);

        let source_id = source_id_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<u64>().ok());

        if let Some(sid) = source_id {
            match source_to_idx.entry(sid) {
                Entry::Occupied(entry) => {
                    let existing_idx = *entry.get();
                    if hip.is_some()
                        && (stars[existing_idx].metadata.hip.is_none()
                            || hip_dist < best_hip_dist[existing_idx])
                    {
                        stars[existing_idx].metadata.hip = hip;
                        best_hip_dist[existing_idx] = hip_dist;
                    }
                    if tyc.is_some()
                        && (stars[existing_idx].metadata.tyc.is_none()
                            || tyc_dist < best_tyc_dist[existing_idx])
                    {
                        stars[existing_idx].metadata.tyc = tyc;
                        best_tyc_dist[existing_idx] = tyc_dist;
                    }
                    continue;
                }
                Entry::Vacant(entry) => {
                    entry.insert(stars.len());
                }
            }
        }

        let bp_rp: f32 = color_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.8); // 0.8 because...it looks good :)

        let star =
            StarRecord::from_icrs_spherical(ra_deg.to_radians(), dec_deg.to_radians(), mag, bp_rp);
        stars.push(BuiltStar {
            record: star,
            ra_deg: ra_deg.rem_euclid(360.0),
            dec_deg: dec_deg.clamp(-90.0, 90.0),
            g_mag: mag,
            metadata: StarMetadata {
                hip,
                tyc,
                ..Default::default()
            },
        });
        best_hip_dist.push(hip_dist);
        best_tyc_dist.push(tyc_dist);
        pb.inc(1);
    }

    pb.finish_with_message(format!("Parsed {} stars", stars.len()));
    Ok(stars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_deduplicates_by_source_id_keeping_closest_hip_and_tyc() {
        let data = b"\
source_id,ra,dec,phot_g_mean_mag,bp_rp,hip,hip_dist,tyc,tyc_dist
1001,10.127,56.537,1.94,1.14,3179,0.03,3663-2668-1,0.50
1001,10.127,56.537,1.94,1.14,,,3663-2668-2,0.02
1002,20.000,15.000,5.20,0.45,, ,TYC 1234-567-1,0.04
";
        let stars = parse_csv_bytes(data, 8.0).expect("parse csv");
        assert_eq!(stars.len(), 2);
        assert_eq!(stars[0].metadata.hip, Some(3179));
        assert_eq!(stars[0].metadata.tyc.as_deref(), Some("3663-2668-2"));
        assert_eq!(stars[1].metadata.hip, None);
        assert_eq!(stars[1].metadata.tyc.as_deref(), Some("1234-567-1"));
    }
}
