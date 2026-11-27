use std::{
    collections::HashSet,
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use celestial::dso::{DsoCatalog, DsoCatalogHeader, DsoKind, DsoRecord, write_dso_catalog_to_zst};
use clap::Args;
use tracing::{info, warn};

use crate::fetch::build_http_client;

const OPENNGC_CSV_URL: &str =
    "https://raw.githubusercontent.com/mattiaverga/OpenNGC/master/database_files/NGC.csv";
const OPENNGC_ADDENDUM_URL: &str =
    "https://raw.githubusercontent.com/mattiaverga/OpenNGC/master/database_files/addendum.csv";

#[derive(Args, Debug)]
pub(crate) struct DsoArgs {
    /// Local OpenNGC NGC.csv file (fetches from OpenNGC GitHub if omitted)
    #[arg(long)]
    pub ngc_csv: Option<PathBuf>,

    /// Local OpenNGC addendum.csv file (fetches from OpenNGC GitHub if omitted)
    #[arg(long)]
    pub addendum_csv: Option<PathBuf>,

    /// Output zstd-compressed DSO catalog binary file
    #[arg(short, long, default_value = "assets/catalogs/dso.bin.zst")]
    pub output: PathBuf,

    /// Zstd compression level (1-22)
    #[arg(long, default_value_t = 12)]
    pub compression_level: i32,
}

async fn read_or_fetch_csv(
    local_path: Option<&Path>,
    url: &str,
    client: &reqwest::Client,
    label: &str,
) -> Result<String> {
    if let Some(path) = local_path {
        info!("Reading {label} from {}...", path.display());
        return std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {} from {}", label, path.display()));
    }

    info!("Downloading {label} from {url}...");
    let text = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to request {label} from {url}"))?
        .error_for_status()
        .with_context(|| format!("{label} HTTP request failed"))?
        .text()
        .await
        .with_context(|| format!("Failed to read {label} response body"))?;
    Ok(text)
}

fn parse_sexagesimal_ra_deg(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    let mut parts = trimmed.split(':');
    let h = parts.next()?.trim().parse::<f64>().ok()?;
    let m = parts.next()?.trim().parse::<f64>().ok()?;
    let s = parts.next()?.trim().parse::<f64>().ok()?;
    if parts.next().is_some()
        || !h.is_finite()
        || !m.is_finite()
        || !s.is_finite()
        || !(0.0..24.0).contains(&h)
        || !(0.0..60.0).contains(&m)
        || !(0.0..60.0).contains(&s)
    {
        return None;
    }
    Some(((h + m / 60.0 + s / 3600.0) * 15.0).rem_euclid(360.0))
}

fn parse_sexagesimal_dec_deg(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (sign, rest) = if let Some(r) = trimmed.strip_prefix('-') {
        (-1.0, r)
    } else if let Some(r) = trimmed.strip_prefix('+') {
        (1.0, r)
    } else {
        (1.0, trimmed)
    };
    let mut parts = rest.split(':');
    let d = parts.next()?.trim().parse::<f64>().ok()?;
    let m = parts.next()?.trim().parse::<f64>().ok()?;
    let s = parts.next()?.trim().parse::<f64>().ok()?;
    if parts.next().is_some()
        || !d.is_finite()
        || !m.is_finite()
        || !s.is_finite()
        || !(0.0..=90.0).contains(&d)
        || !(0.0..60.0).contains(&m)
        || !(0.0..60.0).contains(&s)
    {
        return None;
    }
    let deg = sign * (d + m / 60.0 + s / 3600.0);
    (-90.0..=90.0).contains(&deg).then_some(deg)
}

fn normalize_numeric_catalog_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let digit_end = trimmed
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(trimmed.len());
    if digit_end == 0 {
        return Some(trimmed.to_string());
    }
    let (digits, suffix) = trimmed.split_at(digit_end);
    let stripped_digits = digits.trim_start_matches('0');
    let num_part = if stripped_digits.is_empty() {
        "0"
    } else {
        stripped_digits
    };
    Some(format!("{num_part}{suffix}"))
}

fn parse_constellation_code(raw: &str) -> [u8; 3] {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() == 3 && bytes.iter().all(u8::is_ascii_alphabetic) {
        [bytes[0], bytes[1], bytes[2]]
    } else {
        [0, 0, 0]
    }
}

fn parse_optional_f32(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn normalize_common_name(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() >= 4 && s[..4].eq_ignore_ascii_case("the ") {
        s = s[4..].trim_start();
    }
    if s.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii_lowercase() {
            out.push(first.to_ascii_uppercase());
        } else {
            out.push(first);
        }
        out.push_str(chars.as_str());
    }

    // Normalize common trailing nouns like "Bow-Tie nebula" -> "Bow-Tie Nebula".
    if out.ends_with(" nebula") {
        let len = out.len();
        out.replace_range(len - 6..len, "Nebula");
    } else if out.ends_with(" galaxy") {
        let len = out.len();
        out.replace_range(len - 6..len, "Galaxy");
    } else if out.ends_with(" cluster") {
        let len = out.len();
        out.replace_range(len - 7..len, "Cluster");
    }

    Some(out)
}

fn parse_common_names(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for part in raw.split(',') {
        if let Some(name) = normalize_common_name(part) {
            let key = name.to_lowercase();
            if seen.insert(key) {
                out.push(name);
            }
        }
    }
    out
}

fn prepend_curated_common_name(names: &mut Vec<String>, curated: &str) {
    let curated_lower = curated.to_lowercase();
    names.retain(|n| n.to_lowercase() != curated_lower);
    names.insert(0, curated.to_string());
}

pub(crate) fn parse_openngc_csv(text: &str) -> Result<Vec<DsoRecord>> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());

    let headers = reader.headers()?.clone();
    let col = |name: &str| -> Result<usize> {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
            .with_context(|| format!("Missing \"{name}\" column in OpenNGC CSV"))
    };

    let name_idx = col("Name")?;
    let type_idx = col("Type")?;
    let ra_idx = col("RA")?;
    let dec_idx = col("Dec")?;
    let const_idx = col("Const")?;
    let maj_idx = col("MajAx")?;
    let min_idx = col("MinAx")?;
    let pa_idx = col("PosAng")?;
    let b_mag_idx = col("B-Mag")?;
    let v_mag_idx = col("V-Mag")?;
    let surf_br_idx = col("SurfBr")?;
    let hubble_idx = col("Hubble")?;
    let m_idx = col("M")?;
    let ngc_idx = col("NGC")?;
    let ic_idx = col("IC")?;
    let common_idx = col("Common names")?;

    let mut records = Vec::new();

    for row in reader.records() {
        let Ok(rec) = row else { continue };
        let name = rec.get(name_idx).unwrap_or("").trim();
        let type_code = rec.get(type_idx).unwrap_or("").trim();
        if type_code.eq_ignore_ascii_case("Dup") || type_code.eq_ignore_ascii_case("NonEx") {
            continue;
        }

        let m_raw = rec.get(m_idx).unwrap_or("").trim();
        let messier = (!m_raw.is_empty())
            .then(|| m_raw.parse::<u16>().ok())
            .flatten();

        if !name.starts_with("NGC") && messier.is_none() {
            continue;
        }

        let Some(ra_deg) = parse_sexagesimal_ra_deg(rec.get(ra_idx).unwrap_or("")) else {
            continue;
        };
        let Some(dec_deg) = parse_sexagesimal_dec_deg(rec.get(dec_idx).unwrap_or("")) else {
            continue;
        };

        let kind = match DsoKind::from_openngc_code(type_code) {
            Some(k) => k,
            None => {
                warn!(
                    "Unmapped OpenNGC type {:?} for {}; defaulting to Other",
                    type_code, name
                );
                DsoKind::Other
            }
        };

        let ngc = name
            .strip_prefix("NGC")
            .and_then(normalize_numeric_catalog_id);
        let mut ic = name
            .strip_prefix("IC")
            .and_then(normalize_numeric_catalog_id);

        let mut alt_ngc = Vec::new();
        let cross_ngc_raw = rec.get(ngc_idx).unwrap_or("");
        for item in cross_ngc_raw.split(',') {
            let trimmed = item.trim();
            let raw_id = trimmed.strip_prefix("NGC").unwrap_or(trimmed);
            if let Some(norm) = normalize_numeric_catalog_id(raw_id)
                && ngc.as_deref() != Some(norm.as_str())
                && !alt_ngc.contains(&norm)
            {
                alt_ngc.push(norm);
            }
        }

        if ic.is_none() {
            let cross_ic_raw = rec.get(ic_idx).unwrap_or("");
            for item in cross_ic_raw.split(',') {
                let trimmed = item.trim();
                let raw_id = trimmed.strip_prefix("IC").unwrap_or(trimmed);
                if let Some(norm) = normalize_numeric_catalog_id(raw_id) {
                    ic = Some(norm);
                    break;
                }
            }
        }

        let constellation = parse_constellation_code(rec.get(const_idx).unwrap_or(""));
        let common_names = parse_common_names(rec.get(common_idx).unwrap_or(""));
        let v_mag = parse_optional_f32(rec.get(v_mag_idx).unwrap_or(""));
        let b_mag = parse_optional_f32(rec.get(b_mag_idx).unwrap_or(""));
        let surface_brightness = parse_optional_f32(rec.get(surf_br_idx).unwrap_or(""));
        let major_arcmin = parse_optional_f32(rec.get(maj_idx).unwrap_or(""));
        let minor_arcmin = parse_optional_f32(rec.get(min_idx).unwrap_or(""));
        let position_angle_deg = parse_optional_f32(rec.get(pa_idx).unwrap_or(""));
        let hubble_raw = rec.get(hubble_idx).unwrap_or("").trim();
        let hubble_type = (!hubble_raw.is_empty()).then(|| hubble_raw.to_string());

        records.push(DsoRecord {
            ra_deg,
            dec_deg,
            kind,
            constellation,
            messier,
            ngc,
            ic,
            alt_ngc,
            common_names,
            v_mag,
            b_mag,
            surface_brightness,
            major_arcmin,
            minor_arcmin,
            position_angle_deg,
            hubble_type,
        });
    }

    Ok(records)
}

pub(crate) fn enrich_and_sort_dso_records(records: &mut [DsoRecord]) {
    const MESSIER_V_MAG_FILLS: [(u16, f32); 7] = [
        (22, 5.10),
        (35, 5.10),
        (62, 6.45),
        (69, 7.64),
        (70, 7.90),
        (71, 8.20),
        (107, 7.93),
    ];

    const CURATED_MESSIER_NAMES: [(u16, &str); 8] = [
        (11, "Wild Duck Cluster"),
        (17, "Omega Nebula"),
        (40, "Winnecke 4"),
        (74, "Phantom Galaxy"),
        (76, "Little Dumbbell Nebula"),
        (94, "Cat's Eye Galaxy"),
        (101, "Pinwheel Galaxy"),
        (102, "Spindle Galaxy"),
    ];

    const CURATED_NGC_NAMES: [(&str, &str); 9] = [
        ("253", "Sculptor Galaxy"),
        ("281", "Pacman Nebula"),
        ("891", "Silver Sliver Galaxy"),
        ("2237", "Rosette Nebula"),
        ("2239", "Rosette Cluster"),
        ("2359", "Thor's Helmet"),
        ("3628", "Hamburger Galaxy"),
        ("5866", "Spindle Galaxy"),
        ("6543", "Cat's Eye Nebula"),
    ];

    for rec in records.iter_mut() {
        // Assign M102 to NGC 5866 (Spindle Galaxy) so all 110 Messier objects M1..=M110 exist.
        if rec.messier.is_none() && rec.ngc.as_deref() == Some("5866") {
            rec.messier = Some(102);
        }

        if let Some(m) = rec.messier {
            if rec.v_mag.is_none()
                && let Some(&(_, mag)) = MESSIER_V_MAG_FILLS.iter().find(|&&(id, _)| id == m)
            {
                rec.v_mag = Some(mag);
            }
            if let Some(&(_, name)) = CURATED_MESSIER_NAMES.iter().find(|&&(id, _)| id == m) {
                prepend_curated_common_name(&mut rec.common_names, name);
            }
        }

        if let Some(ngc_id) = rec.ngc.as_deref()
            && let Some(&(_, name)) = CURATED_NGC_NAMES.iter().find(|&&(id, _)| id == ngc_id)
        {
            prepend_curated_common_name(&mut rec.common_names, name);
        }
    }

    records.sort_unstable_by(|a, b| {
        (!a.is_messier())
            .cmp(&(!b.is_messier()))
            .then_with(|| {
                a.effective_display_mag()
                    .total_cmp(&b.effective_display_mag())
            })
            .then_with(|| a.primary_designation().cmp(&b.primary_designation()))
    });
}

pub(crate) async fn run_dso_builder(args: DsoArgs) -> Result<()> {
    let client = build_http_client()?;
    let (ngc_csv, addendum_csv) = tokio::try_join!(
        read_or_fetch_csv(
            args.ngc_csv.as_deref(),
            OPENNGC_CSV_URL,
            &client,
            "OpenNGC NGC.csv"
        ),
        read_or_fetch_csv(
            args.addendum_csv.as_deref(),
            OPENNGC_ADDENDUM_URL,
            &client,
            "OpenNGC addendum.csv"
        ),
    )?;

    let mut records = parse_openngc_csv(&ngc_csv)?;
    let mut addendum_records = parse_openngc_csv(&addendum_csv)?;
    records.append(&mut addendum_records);

    if records.is_empty() {
        bail!("No DSO records parsed from OpenNGC CSV inputs");
    }

    enrich_and_sort_dso_records(&mut records);

    let messier_count = records.iter().filter(|r| r.is_messier()).count() as u32;
    let ngc_count = records.iter().filter(|r| r.is_ngc()).count() as u32;
    let named_count = records
        .iter()
        .filter(|r| !r.common_names.is_empty())
        .count();

    let catalog = DsoCatalog {
        header: DsoCatalogHeader::new(records.len() as u32, messier_count, ngc_count),
        records,
    };

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory {}", parent.display()))?;
    }

    info!(
        "Writing v{} DSO catalog ({} objects: {} Messier, {} NGC, {} with common names) to {}...",
        catalog.header.version,
        catalog.header.object_count,
        catalog.header.messier_count,
        catalog.header.ngc_count,
        named_count,
        args.output.display()
    );

    let out_file = File::create(&args.output)
        .with_context(|| format!("Failed to create output file {}", args.output.display()))?;
    let writer = BufWriter::new(out_file);
    write_dso_catalog_to_zst(writer, &catalog, args.compression_level)
        .with_context(|| "Failed to compress and write DSO catalog")?;

    info!("Success");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_HEADER: &str = "Name;Type;RA;Dec;Const;MajAx;MinAx;PosAng;B-Mag;V-Mag;J-Mag;H-Mag;K-Mag;SurfBr;Hubble;Pax;Pm-RA;Pm-Dec;RadVel;Redshift;Cstar U-Mag;Cstar B-Mag;Cstar V-Mag;M;NGC;IC;Cstar Names;Identifiers;Common names;NED notes;OpenNGC notes;Sources";

    #[test]
    fn parses_openngc_csv_normalizes_ids_and_cross_refs() {
        let csv_data = format!(
            "{SAMPLE_HEADER}\n\
             NGC0224;G;00:42:44.35;+41:16:08.6;And;177.83;69.66;35;4.29;3.44;;;;23.58;Sb;;;;;;;;;031;;;;;Andromeda Galaxy;;;\n\
             NGC2239;OCl;06:31:55.00;+04:56:30.0;Mon;16.00;;;;4.80;;;;;;;;;;;;;;;2244;;;;;;;\n\
             NGC2244;Dup;06:31:55.00;+04:56:30.0;Mon;;;;;;;;;;;;;;;;;;;;2239;;;;;;;\n\
             NGC5866;G;15:06:29.50;+55:45:47.6;Dra;6.31;2.72;126;10.74;9.89;;;;;;;;;;;;;;;\n\
             NGC6618;Neb;18:20:47.00;-16:10:18.0;Sgr;12.60;;;;7.00;;;;;;;;;;;;;;017;;;;;Checkmark Nebula,omega Nebula;;;\n\
             IC4715;*Ass;18:16:56.12;-18:30:52.4;Sgr;120.00;60.00;90;;4.50;;;;;;;;;;;;;;024;;;;;Small Sgr Star Cloud;;;\n\
             IC0002;G;00:11:00.88;-12:49:22.3;Cet;0.98;0.32;142;15.46;;;;;;;;;;;;;;;;;;;;;;;\n"
        );

        let mut records = parse_openngc_csv(&csv_data).expect("parse sample OpenNGC CSV");
        // NGC2244 (Dup) and IC0002 (IC without M) must be skipped.
        assert_eq!(records.len(), 5);

        enrich_and_sort_dso_records(&mut records);

        // M31 sorted near top (brightest Messier).
        let m31 = records
            .iter()
            .find(|r| r.messier == Some(31))
            .expect("M31 present");
        assert_eq!(m31.ngc.as_deref(), Some("224"));
        assert_eq!(m31.constellation_str(), Some("And"));
        assert!((m31.ra_deg - 10.684_79).abs() < 1e-4);
        assert!((m31.dec_deg - 41.269_06).abs() < 1e-4);
        assert_eq!(m31.common_names, vec!["Andromeda Galaxy"]);

        // M17 gets Omega Nebula prepended without dropping Checkmark Nebula.
        let m17 = records
            .iter()
            .find(|r| r.messier == Some(17))
            .expect("M17 present");
        assert_eq!(m17.common_names[0], "Omega Nebula");
        assert!(m17.common_names.iter().any(|n| n == "Checkmark Nebula"));

        // M102 assigned to NGC 5866 with Spindle Galaxy.
        let m102 = records
            .iter()
            .find(|r| r.messier == Some(102))
            .expect("M102 present");
        assert_eq!(m102.ngc.as_deref(), Some("5866"));
        assert_eq!(m102.common_names, vec!["Spindle Galaxy"]);

        // M24 keeps IC 4715.
        let m24 = records
            .iter()
            .find(|r| r.messier == Some(24))
            .expect("M24 present");
        assert_eq!(m24.ic.as_deref(), Some("4715"));
        assert!(m24.ngc.is_none());

        // NGC 2239 preserves NGC 2244 as alt_ngc and Rosette Cluster name.
        let ngc2239 = records
            .iter()
            .find(|r| r.ngc.as_deref() == Some("2239"))
            .expect("NGC 2239 present");
        assert_eq!(ngc2239.alt_ngc, vec!["2244".to_string()]);
        assert_eq!(ngc2239.common_names, vec!["Rosette Cluster"]);
    }

    #[test]
    fn normalizes_common_names_and_leading_zeros() {
        assert_eq!(
            normalize_numeric_catalog_id("0061A").as_deref(),
            Some("61A")
        );
        assert_eq!(
            normalize_numeric_catalog_id("0190 NED01").as_deref(),
            Some("190 NED01")
        );
        assert_eq!(
            parse_common_names("the Witch Head Nebula,Bow-Tie nebula,omega Nebula"),
            vec![
                "Witch Head Nebula".to_string(),
                "Bow-Tie Nebula".to_string(),
                "Omega Nebula".to_string(),
            ]
        );
    }
}
