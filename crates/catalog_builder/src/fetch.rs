use std::time::Duration;

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use crate::{BuiltStar, csv::parse_csv_bytes};

const GAIA_TAP_URL: &str = "https://gea.esac.esa.int/tap-server/tap/sync";
/// Time baseline in Julian years from Hipparcos epoch J1991.25 to Gaia DR3 epoch J2016.0.
const HIP_TO_GAIA_YEARS: f64 = 24.75;

#[derive(Clone, Debug)]
pub(crate) struct HipparcosStar {
    pub hip: u32,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub hp_mag: f32,
}

pub(crate) fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("apogee-catalog-builder/0.1")
        .timeout(Duration::from_secs(600))
        .build()
        .context("Failed to build HTTP client")
}

async fn run_gaia_tap_csv(
    client: &reqwest::Client,
    adql: &str,
    spinner_label: &str,
) -> Result<Vec<u8>> {
    info!("Querying ESA Gaia TAP archive at {}...", GAIA_TAP_URL);
    info!("ADQL: {}", adql);

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner().template("{spinner:.green} [{elapsed_precise}] {msg}")?,
    );
    spinner.set_message(spinner_label.to_string());
    spinner.enable_steady_tick(Duration::from_millis(120));

    let response = client
        .post(GAIA_TAP_URL)
        .form(&[
            ("REQUEST", "doQuery"),
            ("LANG", "ADQL"),
            ("FORMAT", "csv"),
            ("MAXREC", "2000000"),
            ("QUERY", adql),
        ])
        .send()
        .await
        .context("Failed to send TAP query request to ESA")?;

    if !response.status().is_success() {
        let status = response.status();
        let err_text = response.text().await.unwrap_or_default();
        bail!("Gaia TAP query failed (HTTP {}): {}", status, err_text);
    }

    spinner.set_message("Downloading TAP CSV response...");
    let bytes = response
        .bytes()
        .await
        .context("Failed to download TAP response body")?;
    spinner.finish_with_message(format!(
        "Downloaded {:.2} MB",
        bytes.len() as f64 / 1_048_576.0
    ));

    Ok(bytes.to_vec())
}

pub(crate) async fn fetch_from_gaia_tap(
    client: &reqwest::Client,
    max_mag: f32,
) -> Result<Vec<BuiltStar>> {
    let adql = format!(
        "SELECT g.source_id, g.ra, g.dec, g.phot_g_mean_mag, g.bp_rp, \
                h.original_ext_source_id AS hip, h.angular_distance AS hip_dist, \
                t.original_ext_source_id AS tyc, t.angular_distance AS tyc_dist \
         FROM gaiadr3.gaia_source AS g \
         LEFT JOIN gaiadr3.hipparcos2_best_neighbour AS h ON g.source_id = h.source_id \
         LEFT JOIN gaiadr3.tycho2tdsc_merge_best_neighbour AS t ON g.source_id = t.source_id \
         WHERE g.phot_g_mean_mag <= {:.2}",
        max_mag
    );

    let bytes = run_gaia_tap_csv(client, &adql, "Executing Gaia DR3 TAP query...").await?;
    parse_csv_bytes(&bytes, max_mag)
}

pub(crate) async fn fetch_bright_hipparcos(
    client: &reqwest::Client,
    max_hp_mag: f32,
) -> Result<Vec<HipparcosStar>> {
    let adql = format!(
        "SELECT hip, ra, dec, pm_ra, pm_de, hp_mag \
         FROM public.hipparcos_newreduction \
         WHERE hp_mag <= {:.2}",
        max_hp_mag
    );

    let bytes = run_gaia_tap_csv(
        client,
        &adql,
        "Executing Hipparcos bright-star TAP query...",
    )
    .await?;

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(bytes.as_slice());

    let headers = reader.headers()?.clone();
    let col = |name: &str| -> Result<usize> {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
            .with_context(|| format!("Missing \"{name}\" column in Hipparcos TAP response"))
    };

    let hip_idx = col("hip")?;
    let ra_idx = col("ra")?;
    let dec_idx = col("dec")?;
    let pm_ra_idx = col("pm_ra")?;
    let pm_de_idx = col("pm_de")?;
    let hp_mag_idx = col("hp_mag")?;

    let mut out = Vec::new();
    for rec in reader.records() {
        let Ok(rec) = rec else { continue };
        let Some(hip) = rec.get(hip_idx).and_then(|s| s.trim().parse::<u32>().ok()) else {
            continue;
        };
        let Some(ra0) = rec.get(ra_idx).and_then(|s| s.trim().parse::<f64>().ok()) else {
            continue;
        };
        let Some(dec0) = rec.get(dec_idx).and_then(|s| s.trim().parse::<f64>().ok()) else {
            continue;
        };
        let Some(hp_mag) = rec
            .get(hp_mag_idx)
            .and_then(|s| s.trim().parse::<f32>().ok())
        else {
            continue;
        };
        if !ra0.is_finite() || !dec0.is_finite() || !hp_mag.is_finite() {
            continue;
        }

        let pm_ra = rec
            .get(pm_ra_idx)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let pm_de = rec
            .get(pm_de_idx)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);

        let (ra_deg, dec_deg) = propagate_icrs_mas(ra0, dec0, pm_ra, pm_de, HIP_TO_GAIA_YEARS);
        out.push(HipparcosStar {
            hip,
            ra_deg,
            dec_deg,
            hp_mag,
        });
    }

    info!("Fetched {} bright Hipparcos stars", out.len());
    Ok(out)
}

/// Propagates ICRS coordinates `(ra_deg, dec_deg)` by `dt_years` using `pm_ra = μ_α*cos(δ)` and `pm_de = μ_δ` in mas/yr.
pub(crate) fn propagate_icrs_mas(
    ra_deg: f64,
    dec_deg: f64,
    pm_ra_mas_yr: f64,
    pm_de_mas_yr: f64,
    dt_years: f64,
) -> (f64, f64) {
    let cos_dec = dec_deg.to_radians().cos().abs().max(0.005);
    let prop_dec = (dec_deg + pm_de_mas_yr * dt_years / 3_600_000.0).clamp(-90.0, 90.0);
    let prop_ra =
        (ra_deg + (pm_ra_mas_yr * dt_years / 3_600_000.0) / cos_dec).rem_euclid(360.0);
    (prop_ra, prop_dec)
}
