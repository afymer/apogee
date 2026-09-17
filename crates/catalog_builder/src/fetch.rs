use std::time::Duration;

use anyhow::{Context, Result, bail};
use celestial::catalog::StarRecord;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use crate::csv::parse_csv_bytes;

pub(crate) async fn fetch_from_gaia_tap(max_mag: f32) -> Result<Vec<StarRecord>> {
    let tap_url = "https://gea.esac.esa.int/tap-server/tap/sync";
    let adql = format!(
        "SELECT ra, dec, phot_g_mean_mag, bp_rp \
         FROM gaiadr3.gaia_source \
         WHERE phot_g_mean_mag <= {:.2}",
        max_mag
    );

    info!("Querying ESA Gaia TAP archive at {}...", tap_url);
    info!("ADQL: {}", adql);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner().template("{spinner:.green} [{elapsed_precise}] {msg}")?,
    );
    spinner.set_message("Executing TAP query on ESA servers...");
    spinner.enable_steady_tick(Duration::from_millis(120));

    let response = client
        .post(tap_url)
        .form(&[
            ("REQUEST", "doQuery"),
            ("LANG", "ADQL"),
            ("FORMAT", "csv"),
            ("MAXREC", "2000000"),
            ("QUERY", &adql),
        ])
        .send()
        .await
        .context("Failed to send TAP query request to ESA")?;

    if !response.status().is_success() {
        let status = response.status();
        let err_text = response.text().await.unwrap_or_default();
        bail!("Gaia TAP query failed (HTTP {}): {}", status, err_text);
    }

    spinner.set_message("Downloading and parsing Gaia CSV stream...");

    let bytes = response
        .bytes()
        .await
        .context("Failed to download TAP response body")?;
    spinner.finish_with_message(format!(
        "Downloaded {:.2} MB",
        bytes.len() as f64 / 1_048_576.0
    ));

    parse_csv_bytes(&bytes, max_mag)
}
