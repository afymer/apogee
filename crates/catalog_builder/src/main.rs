mod csv;
mod dso;
mod enrich;
mod fetch;

use std::{fs::File, io::BufWriter, path::PathBuf};

use anyhow::{Context, Result, bail};
use celestial::catalog::{StarCatalogHeader, StarMetadata, StarRecord, write_star_catalog_to_zst};
use clap::{Args, Parser, Subcommand};
use tracing::info;

use crate::{
    csv::parse_from_csv,
    dso::{DsoArgs, run_dso_builder},
    enrich::{enrich_catalog, fetch_bsc5_entries, fetch_iau_entries},
    fetch::{build_http_client, fetch_bright_hipparcos, fetch_from_gaia_tap},
};

#[derive(Clone, Debug)]
pub(crate) struct BuiltStar {
    pub record: StarRecord,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub g_mag: f32,
    pub metadata: StarMetadata,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Apogee astronomical catalog builder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Build the Gaia DR3 / Hipparcos / BSC5 / IAU star catalog (`stars.bin.zst`)
    Stars(StarsArgs),
    /// Build the OpenNGC Messier + NGC deep-sky catalog (`dso.bin.zst`)
    Dso(DsoArgs),
}

#[derive(Args, Debug)]
struct StarsArgs {
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

    /// Skip astronomical name enrichment (IAU-CSN, BSC5 Bayer/Flamsteed, bright Hipparcos fallback)
    #[arg(long, default_value_t = false)]
    no_enrich: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Stars(args) => run_stars_builder(args).await,
        Command::Dso(args) => run_dso_builder(args).await,
    }
}

async fn run_stars_builder(args: StarsArgs) -> Result<()> {
    let client = build_http_client()?;

    let mut stars = if let Some(ref path) = args.input {
        parse_from_csv(path, args.max_mag)?
    } else {
        fetch_from_gaia_tap(&client, args.max_mag).await?
    };

    if stars.is_empty() {
        bail!("No star records found for magnitude <= {}", args.max_mag);
    }

    let enrich_stats = if args.no_enrich {
        None
    } else {
        let (bright_hipparcos, bsc5, iau_entries) = tokio::try_join!(
            fetch_bright_hipparcos(&client, 6.8),
            fetch_bsc5_entries(&client),
            fetch_iau_entries(&client),
        )?;
        Some(enrich_catalog(
            &mut stars,
            &bright_hipparcos,
            &bsc5,
            &iau_entries,
            args.max_mag,
        ))
    };

    stars.sort_unstable_by_key(|s| (s.record.mag_scaled, s.ra_deg.to_bits()));

    let min_mag = stars.first().map(|s| s.record.mag()).unwrap_or(0.0);
    let max_mag = stars.last().map(|s| s.record.mag()).unwrap_or(0.0);

    let mut records = Vec::with_capacity(stars.len());
    let mut metadata = Vec::new();
    let mut proper_count = 0usize;
    let mut bayer_count = 0usize;
    let mut hip_count = 0usize;
    let mut hd_count = 0usize;
    let mut hr_count = 0usize;
    let mut tyc_count = 0usize;

    for (idx, star) in stars.into_iter().enumerate() {
        records.push(star.record);
        if !star.metadata.is_empty() {
            if star.metadata.proper_name.is_some() {
                proper_count += 1;
            }
            if star.metadata.bayer_flamsteed.is_some() {
                bayer_count += 1;
            }
            if star.metadata.hip.is_some() {
                hip_count += 1;
            }
            if star.metadata.hd.is_some() {
                hd_count += 1;
            }
            if star.metadata.hr.is_some() {
                hr_count += 1;
            }
            if star.metadata.tyc.is_some() {
                tyc_count += 1;
            }
            metadata.push((idx as u32, star.metadata));
        }
    }

    let header = StarCatalogHeader::new(records.len() as u32, min_mag, max_mag);

    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory {}", parent.display())
            })?;
        }
    }

    info!(
        "Writing v{} catalog ({} stars, {:.2}..{:.2} mag, {} named/identified) to {}...",
        header.version,
        records.len(),
        min_mag,
        max_mag,
        metadata.len(),
        args.output.display()
    );
    if let Some(stats) = enrich_stats {
        info!(
            "Enrichment stats: +{} supplemental bright stars, {} BSC5 matches, {} Hipparcos fills, {} IAU names",
            stats.supplemental_added,
            stats.matched_bsc5,
            stats.matched_hipparcos,
            stats.matched_iau_names,
        );
    }
    info!(
        "Metadata breakdown: {} proper names, {} Bayer/Flamsteed, {} HIP, {} HD, {} HR, {} TYC-only",
        proper_count, bayer_count, hip_count, hd_count, hr_count, tyc_count
    );

    let out_file = File::create(&args.output)
        .with_context(|| format!("Failed to create output file {}", args.output.display()))?;
    let writer = BufWriter::new(out_file);

    write_star_catalog_to_zst(writer, &header, &records, &metadata, args.compression_level)
        .with_context(|| "Failed to compress and write star catalog")?;

    info!("Success");

    Ok(())
}
