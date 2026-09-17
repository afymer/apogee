mod csv;
mod fetch;

use std::{fs::File, io::BufWriter, path::PathBuf};

use anyhow::{Context, Result, bail};
use celestial::catalog::{StarCatalogHeader, write_stars_to_zst};
use clap::Parser;
use tracing::info;

use crate::{csv::parse_from_csv, fetch::fetch_from_gaia_tap};

#[derive(Parser, Debug)]
#[command(author, version, about = "Apogee catalog builder for Gaia DR3")]
struct Args {
    /// Input CSV file containing Gaia sources (columns: ra, dec, phot_g_mean_mag, bp_rp)
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Output zstd-compressed catalog binary file
    #[arg(short, long, default_value = "assets/catalogs/stars.bin.zst")]
    output: PathBuf,

    /// Maximum limiting magnitude to include (G band)
    #[arg(long, default_value_t = 8.0)]
    max_mag: f32,

    /// Zstd compression level (1-22)
    #[arg(long, default_value_t = 9)]
    compression_level: i32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let records = if let Some(ref path) = args.input {
        parse_from_csv(path, args.max_mag)?
    } else {
        fetch_from_gaia_tap(args.max_mag).await?
    };

    if records.is_empty() {
        bail!("No star records found for magnitude < {}", args.max_mag);
    }

    let mut sorted_records = records;
    sorted_records.sort_unstable_by_key(|s| s.mag_scaled);

    let min_mag = sorted_records.first().map(|s| s.mag()).unwrap_or(0.0);
    let max_mag = sorted_records.last().map(|s| s.mag()).unwrap_or(0.0);

    let header = StarCatalogHeader::new(sorted_records.len() as u32, min_mag, max_mag);

    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory {}", parent.display())
            })?;
        }
    }

    info!("Writing compressed catalog to {}...", args.output.display());
    let out_file = File::create(&args.output)
        .with_context(|| format!("Failed to create output file {}", args.output.display()))?;
    let writer = BufWriter::new(out_file);

    write_stars_to_zst(writer, &header, &sorted_records, args.compression_level)
        .with_context(|| "Failed to compress and write star catalog")?;

    info!("Success");

    Ok(())
}
