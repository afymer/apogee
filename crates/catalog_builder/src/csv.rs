use std::{fs::File, io::BufReader, path::Path, time::Duration};

use anyhow::{Context, Result};
use celestial::catalog::StarRecord;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{info, warn};

pub(crate) fn parse_from_csv(path: &Path, max_mag: f32) -> Result<Vec<StarRecord>> {
    info!("Reading Gaia CSV from {}...", path.display());
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    parse_csv_reader(reader, max_mag)
}

pub(crate) fn parse_csv_bytes(bytes: &[u8], max_mag: f32) -> Result<Vec<StarRecord>> {
    parse_csv_reader(bytes, max_mag)
}

fn parse_csv_reader<R: std::io::Read>(reader: R, max_mag: f32) -> Result<Vec<StarRecord>> {
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

    let mut records = Vec::new();
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

        let ra_deg: f64 = match ra_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let dec_deg: f64 = match dec_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mag: f32 = match mag_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        if mag > max_mag || !mag.is_finite() || !ra_deg.is_finite() || !dec_deg.is_finite() {
            continue;
        }

        let bp_rp: f32 = color_idx
            .and_then(|idx| record.get(idx))
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.8); // 0.8 because...it looks good :)

        let star =
            StarRecord::from_icrs_spherical(ra_deg.to_radians(), dec_deg.to_radians(), mag, bp_rp);
        records.push(star);
        pb.inc(1);
    }

    pb.finish_with_message(format!("Parsed {} stars", records.len()));
    Ok(records)
}
