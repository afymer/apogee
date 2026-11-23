use std::{collections::HashSet, io::Read};

use anyhow::{Context, Result};
use celestial::{
    catalog::{StarMetadata, StarRecord},
    coords::{Angle, canonical_spherical_to_cartesian},
};
use flate2::read::GzDecoder;
use glam::DVec3;
use tracing::info;

use crate::{BuiltStar, fetch::HipparcosStar};

const BSC5_URL: &str = "http://tdc-www.harvard.edu/catalogs/bsc5.dat.gz";
const IAU_CSN_TXT_URL: &str = "https://www.pas.rochester.edu/~emamajek/WGSN/IAU-CSN.txt";
const IAU_CSN_CSV_URL: &str =
    "https://raw.githubusercontent.com/cyschneck/iau-star-names/main/iau_proper_stars.csv";

/// Time baseline in Julian years from J2000.0 to Gaia DR3 epoch J2016.0.
const J2000_TO_GAIA_YEARS: f64 = 16.0;
const DEC_BANDS: usize = 180;

#[derive(Clone, Debug)]
pub(crate) struct Bsc5Entry {
    pub hr: u16,
    pub hd: Option<u32>,
    pub bayer_flamsteed: Option<String>,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub g_est: f32,
    pub bp_rp_est: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct IauNameEntry {
    pub proper_name: String,
    pub hr: Option<u16>,
    pub hd: Option<u32>,
    pub hip: Option<u32>,
    pub bayer_flamsteed: Option<String>,
    pub mag: Option<f32>,
    pub ra_deg: Option<f64>,
    pub dec_deg: Option<f64>,
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct EnrichmentStats {
    pub matched_bsc5: usize,
    pub supplemental_added: usize,
    pub matched_hipparcos: usize,
    pub matched_iau_names: usize,
}

pub(crate) async fn fetch_bsc5_entries(client: &reqwest::Client) -> Result<Vec<Bsc5Entry>> {
    info!(
        "Downloading Yale Bright Star Catalog (BSC5) from {}...",
        BSC5_URL
    );
    let bytes = client
        .get(BSC5_URL)
        .send()
        .await
        .context("Failed to request BSC5 catalog")?
        .error_for_status()
        .context("BSC5 HTTP request failed")?
        .bytes()
        .await
        .context("Failed to download BSC5 bytes")?;

    let mut decoder = GzDecoder::new(bytes.as_ref());
    let mut raw = Vec::new();
    decoder
        .read_to_end(&mut raw)
        .context("Failed to decompress BSC5 gzip stream")?;

    let text = String::from_utf8_lossy(&raw);
    let entries = parse_bsc5_text(&text);
    info!("Parsed {} stellar entries from BSC5", entries.len());
    Ok(entries)
}

pub(crate) async fn fetch_iau_entries(client: &reqwest::Client) -> Result<Vec<IauNameEntry>> {
    info!("Downloading IAU-CSN from {}...", IAU_CSN_TXT_URL);
    let txt_body = client
        .get(IAU_CSN_TXT_URL)
        .send()
        .await
        .context("Failed to request IAU-CSN.txt")?
        .error_for_status()
        .context("IAU-CSN.txt HTTP request failed")?
        .text()
        .await
        .context("Failed to read IAU-CSN.txt body")?;

    info!("Downloading updated IAU CSV from {}...", IAU_CSN_CSV_URL);
    let csv_body = client
        .get(IAU_CSN_CSV_URL)
        .send()
        .await
        .context("Failed to request iau_proper_stars.csv")?
        .error_for_status()
        .context("iau_proper_stars.csv HTTP request failed")?
        .text()
        .await
        .context("Failed to read iau_proper_stars.csv body")?;

    let mut entries = parse_iau_csn_txt(&txt_body);
    let csv_entries = parse_iau_csv(&csv_body)?;
    merge_iau_lists(&mut entries, csv_entries);
    info!("Merged {} unique IAU proper-name entries", entries.len());
    Ok(entries)
}

pub(crate) fn parse_bsc5_text(text: &str) -> Vec<Bsc5Entry> {
    let mut entries = Vec::with_capacity(9110);
    for line in text.lines() {
        if line.len() < 114 || !line.is_ascii() {
            continue;
        }
        let Ok(hr) = line[0..4].trim().parse::<u16>() else {
            continue;
        };
        let ra_h = line[75..77].trim();
        if ra_h.is_empty() {
            continue;
        }
        let (Ok(rah), Ok(ram), Ok(ras)) = (
            ra_h.parse::<f64>(),
            line[77..79].trim().parse::<f64>(),
            line[79..83].trim().parse::<f64>(),
        ) else {
            continue;
        };
        let (Ok(decd), Ok(decm), Ok(decs)) = (
            line[84..86].trim().parse::<f64>(),
            line[86..88].trim().parse::<f64>(),
            line[88..90].trim().parse::<f64>(),
        ) else {
            continue;
        };
        let Ok(vmag) = line[102..107].trim().parse::<f32>() else {
            continue;
        };
        if !vmag.is_finite() {
            continue;
        }

        let dec_sign = if &line[83..84] == "-" { -1.0 } else { 1.0 };
        let ra_j2000 = (rah + ram / 60.0 + ras / 3600.0) * 15.0;
        let dec_j2000 = dec_sign * (decd + decm / 60.0 + decs / 3600.0);
        if !ra_j2000.is_finite() || !dec_j2000.is_finite() {
            continue;
        }

        let pmra_arcsec_yr = line
            .get(148..154)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let pmde_arcsec_yr = line
            .get(154..160)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);

        let (ra_deg, dec_deg) = propagate_j2000_arcsec(
            ra_j2000,
            dec_j2000,
            pmra_arcsec_yr,
            pmde_arcsec_yr,
            J2000_TO_GAIA_YEARS,
        );

        let hd = line[25..31].trim().parse::<u32>().ok();
        let bv = line[109..114]
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite());
        let (g_est, bp_rp_est) = vmag_bv_to_gaia_photometry(vmag, bv);
        let bayer_flamsteed = format_bsc5_designation(&line[4..14]);

        entries.push(Bsc5Entry {
            hr,
            hd,
            bayer_flamsteed,
            ra_deg,
            dec_deg,
            g_est,
            bp_rp_est,
        });
    }
    entries
}

pub(crate) fn propagate_j2000_arcsec(
    ra_deg: f64,
    dec_deg: f64,
    pm_ra_arcsec_yr: f64,
    pm_de_arcsec_yr: f64,
    dt_years: f64,
) -> (f64, f64) {
    let cos_dec = dec_deg.to_radians().cos().abs().max(0.005);
    let prop_dec = (dec_deg + pm_de_arcsec_yr * dt_years / 3600.0).clamp(-90.0, 90.0);
    let prop_ra = (ra_deg + (pm_ra_arcsec_yr * dt_years / 3600.0) / cos_dec).rem_euclid(360.0);
    (prop_ra, prop_dec)
}

/// Converts Johnson `V` magnitude and `B - V` color index to empirical Gaia `(G, BP - RP)` estimates.
pub(crate) fn vmag_bv_to_gaia_photometry(vmag: f32, bv: Option<f32>) -> (f32, f32) {
    // Anchors: (B-V, median BP-RP, median G-V) calibrated against Gaia DR3 stars in stars.bin.zst.
    const ANCHORS: [(f32, f32, f32); 13] = [
        (-0.30, -0.35, -0.04),
        (-0.15, -0.18, -0.03),
        (-0.01, 0.01, -0.02),
        (0.19, 0.28, -0.04),
        (0.40, 0.56, -0.10),
        (0.58, 0.77, -0.14),
        (0.82, 1.04, -0.22),
        (1.00, 1.16, -0.27),
        (1.18, 1.33, -0.34),
        (1.40, 1.57, -0.47),
        (1.59, 1.98, -0.73),
        (1.79, 2.24, -0.88),
        (2.10, 2.75, -1.15),
    ];

    let x = bv
        .unwrap_or(0.60)
        .clamp(ANCHORS[0].0, ANCHORS[ANCHORS.len() - 1].0);
    let (bp_rp, g_minus_v) = if x <= ANCHORS[0].0 {
        (ANCHORS[0].1, ANCHORS[0].2)
    } else if x >= ANCHORS[ANCHORS.len() - 1].0 {
        let last = ANCHORS[ANCHORS.len() - 1];
        (last.1, last.2)
    } else {
        let mut res = (ANCHORS[0].1, ANCHORS[0].2);
        for w in ANCHORS.windows(2) {
            let (x0, b0, d0) = w[0];
            let (x1, b1, d1) = w[1];
            if x <= x1 {
                let u = (x - x0) / (x1 - x0);
                res = (b0 + (b1 - b0) * u, d0 + (d1 - d0) * u);
                break;
            }
        }
        res
    };

    (vmag + g_minus_v, bp_rp)
}

fn greek_code_to_unicode(code: &str) -> Option<char> {
    match code {
        "Alp" => Some('α'),
        "Bet" => Some('β'),
        "Gam" => Some('γ'),
        "Del" => Some('δ'),
        "Eps" => Some('ε'),
        "Zet" => Some('ζ'),
        "Eta" => Some('η'),
        "The" => Some('θ'),
        "Iot" => Some('ι'),
        "Kap" => Some('κ'),
        "Lam" => Some('λ'),
        "Mu" => Some('μ'),
        "Nu" => Some('ν'),
        "Xi" => Some('ξ'),
        "Omi" => Some('ο'),
        "Pi" => Some('π'),
        "Rho" => Some('ρ'),
        "Sig" => Some('σ'),
        "Tau" => Some('τ'),
        "Ups" => Some('υ'),
        "Phi" => Some('φ'),
        "Chi" => Some('χ'),
        "Psi" => Some('ψ'),
        "Ome" => Some('ω'),
        _ => None,
    }
}

fn digit_to_superscript(ch: char) -> Option<char> {
    match ch {
        '1' => Some('¹'),
        '2' => Some('²'),
        '3' => Some('³'),
        '4' => Some('⁴'),
        '5' => Some('⁵'),
        '6' => Some('⁶'),
        '7' => Some('⁷'),
        '8' => Some('⁸'),
        '9' => Some('⁹'),
        _ => None,
    }
}

/// Formats a 10-character BSC5 Bayer/Flamsteed column (`[4..14]`) into clean Unicode (e.g. `"α CMa"`, `"61 Cyg"`).
pub(crate) fn format_bsc5_designation(raw: &str) -> Option<String> {
    if raw.len() < 10 {
        return None;
    }
    let flam = raw[0..3].trim();
    let bayer = raw[3..7].trim();
    let con = raw[7..10].trim();
    if con.is_empty() || !con.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    if !bayer.is_empty() {
        let trimmed = bayer.trim();
        let last = trimmed.chars().last();
        let (prefix_raw, sup) = match last.and_then(digit_to_superscript) {
            Some(sup) => (&trimmed[..trimmed.len() - 1], Some(sup)),
            None => (trimmed, None),
        };
        if let Some(greek) = greek_code_to_unicode(prefix_raw.trim()) {
            return match sup {
                Some(sup) => Some(format!("{greek}{sup} {con}")),
                None => Some(format!("{greek} {con}")),
            };
        }
    }

    if !flam.is_empty() && flam.parse::<u16>().is_ok() {
        return Some(format!("{flam} {con}"));
    }

    None
}

/// Replaces ASCII component digits immediately following a Greek letter with Unicode superscripts (e.g. `"θ1 Eri"` -> `"θ¹ Eri"`).
pub(crate) fn normalize_bayer_superscripts(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_is_greek = false;
    for ch in input.chars() {
        if prev_is_greek {
            if let Some(sup) = digit_to_superscript(ch) {
                out.push(sup);
                prev_is_greek = false;
                continue;
            }
        }
        out.push(ch);
        prev_is_greek = ('α'..='ω').contains(&ch);
    }
    out
}

fn slice_chars(chars: &[char], start: usize, end: usize) -> String {
    if start >= chars.len() {
        return String::new();
    }
    let end = end.min(chars.len());
    chars[start..end]
        .iter()
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_designation_ids(desig: &str) -> (Option<u16>, Option<u32>) {
    let trimmed = desig.trim();
    let hr = trimmed
        .strip_prefix("HR ")
        .and_then(|s| s.trim().parse::<u16>().ok());
    let hd = trimmed
        .strip_prefix("HD ")
        .and_then(|s| s.trim().parse::<u32>().ok());
    (hr, hd)
}

pub(crate) fn parse_iau_csn_txt(text: &str) -> Vec<IauNameEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('$') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 72 {
            continue;
        }

        let name_ascii = slice_chars(&chars, 0, 18);
        let name_utf8 = slice_chars(&chars, 18, 36);
        let proper_name = if name_utf8.is_empty() {
            name_ascii
        } else {
            name_utf8
        };
        if proper_name.is_empty() {
            continue;
        }

        let desig = slice_chars(&chars, 36, 49);
        let (hr, hd_from_desig) = parse_designation_ids(&desig);

        let bayer_u = slice_chars(&chars, 55, 61);
        let con = slice_chars(&chars, 61, 65);
        let bayer_flamsteed =
            if !bayer_u.is_empty() && bayer_u != "_" && !con.is_empty() && con != "_" {
                Some(normalize_bayer_superscripts(&format!("{bayer_u} {con}")))
            } else {
                None
            };

        let tail: String = chars[70..].iter().collect();
        let tokens: Vec<&str> = tail.split_whitespace().collect();
        if tokens.len() < 7 {
            continue;
        }

        let mag = (tokens[1] != "_")
            .then(|| tokens[1].parse::<f32>().ok())
            .flatten()
            .filter(|v| v.is_finite());
        let hip = (tokens[3] != "_")
            .then(|| tokens[3].parse::<u32>().ok())
            .flatten();
        let hd = hd_from_desig.or_else(|| {
            (tokens[4] != "_")
                .then(|| tokens[4].parse::<u32>().ok())
                .flatten()
        });
        let ra_deg = (tokens[5] != "_")
            .then(|| tokens[5].parse::<f64>().ok())
            .flatten()
            .filter(|v| v.is_finite());
        let dec_deg = (tokens[6] != "_")
            .then(|| tokens[6].parse::<f64>().ok())
            .flatten()
            .filter(|v| v.is_finite());

        entries.push(IauNameEntry {
            proper_name,
            hr,
            hd,
            hip,
            bayer_flamsteed,
            mag,
            ra_deg,
            dec_deg,
        });
    }
    entries
}

fn parse_iau_csv(text: &str) -> Result<Vec<IauNameEntry>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader.headers()?.clone();
    let find_col = |name: &str| -> Option<usize> {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };

    let name_idx =
        find_col("Proper Names").context("Missing \"Proper Names\" column in IAU CSV")?;
    let desig_idx = find_col("Designation");
    let hip_idx = find_col("HIP");
    let bayer_idx = find_col("Bayer ID");

    let mut out = Vec::new();
    for rec in reader.records() {
        let Ok(rec) = rec else { continue };
        let proper_name = rec.get(name_idx).unwrap_or("").trim().to_string();
        if proper_name.is_empty() {
            continue;
        }
        let desig = desig_idx.and_then(|i| rec.get(i)).unwrap_or("").trim();
        let (hr, hd) = parse_designation_ids(desig);
        let hip = hip_idx
            .and_then(|i| rec.get(i))
            .and_then(|s| s.trim().parse::<u32>().ok());
        let bayer_flamsteed = bayer_idx
            .and_then(|i| rec.get(i))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(normalize_bayer_superscripts);

        out.push(IauNameEntry {
            proper_name,
            hr,
            hd,
            hip,
            bayer_flamsteed,
            mag: None,
            ra_deg: None,
            dec_deg: None,
        });
    }
    Ok(out)
}

fn merge_iau_lists(base: &mut Vec<IauNameEntry>, extra: Vec<IauNameEntry>) {
    for item in extra {
        let existing_pos = base.iter().position(|e| {
            e.proper_name.eq_ignore_ascii_case(&item.proper_name)
                || (e.hr.is_some() && e.hr == item.hr)
        });
        if let Some(pos) = existing_pos {
            let target = &mut base[pos];
            if target.hip.is_none() {
                target.hip = item.hip;
            }
            if target.hd.is_none() {
                target.hd = item.hd;
            }
            if target.hr.is_none() {
                target.hr = item.hr;
            }
            if target.bayer_flamsteed.is_none() {
                target.bayer_flamsteed = item.bayer_flamsteed;
            }
        } else {
            base.push(item);
        }
    }
}

#[inline]
fn icrs_unit_dir(ra_deg: f64, dec_deg: f64) -> DVec3 {
    canonical_spherical_to_cartesian(Angle::from_degrees(ra_deg), Angle::from_degrees(dec_deg))
        .normalize_or_zero()
}

#[inline]
fn angular_sep_arcsec(a: DVec3, b: DVec3) -> f64 {
    let half_chord = (0.5 * (a - b).length()).clamp(0.0, 1.0);
    (2.0 * half_chord.asin()).to_degrees() * 3600.0
}

#[derive(Clone, Copy)]
struct MatchQuery {
    ra_deg: f64,
    dec_deg: f64,
    target_mag: Option<f32>,
    max_sep_arcsec: f64,
    max_dmag: f32,
}

struct DecBandIndex {
    dirs: Vec<DVec3>,
    bands: Vec<Vec<usize>>,
}

impl DecBandIndex {
    fn build(stars: &[BuiltStar]) -> Self {
        let mut dirs = Vec::with_capacity(stars.len());
        let mut bands = vec![Vec::new(); DEC_BANDS];
        for (idx, star) in stars.iter().enumerate() {
            let dir = icrs_unit_dir(star.ra_deg, star.dec_deg);
            let bin = Self::z_bin(dir.z);
            bands[bin].push(idx);
            dirs.push(dir);
        }
        Self { dirs, bands }
    }

    #[inline]
    fn z_bin(z: f64) -> usize {
        ((z + 1.0) * (DEC_BANDS as f64 * 0.5))
            .floor()
            .clamp(0.0, (DEC_BANDS - 1) as f64) as usize
    }

    fn find_best_match(
        &self,
        stars: &[BuiltStar],
        query: MatchQuery,
        mut filter: impl FnMut(&BuiltStar) -> bool,
    ) -> Option<usize> {
        let target_dir = icrs_unit_dir(query.ra_deg, query.dec_deg);
        let center_bin = Self::z_bin(target_dir.z);
        let start_bin = center_bin.saturating_sub(1);
        let end_bin = (center_bin + 1).min(DEC_BANDS - 1);

        let mut best: Option<(f64, usize)> = None;
        for bin in start_bin..=end_bin {
            for &idx in &self.bands[bin] {
                let star = &stars[idx];
                if !filter(star) {
                    continue;
                }
                let sep = angular_sep_arcsec(self.dirs[idx], target_dir);
                if sep > query.max_sep_arcsec {
                    continue;
                }
                let dmag = query.target_mag.map_or(0.0, |m| (star.g_mag - m).abs());
                let mag_ok = dmag <= query.max_dmag || (sep < 3.0 && dmag <= query.max_dmag + 1.0);
                if !mag_ok {
                    continue;
                }
                let score = sep + 3.0 * f64::from(dmag);
                if best.is_none_or(|(best_score, _)| score < best_score) {
                    best = Some((score, idx));
                }
            }
        }
        best.map(|(_, idx)| idx)
    }
}

fn find_brightest_matching_star(
    stars: &[BuiltStar],
    predicate: impl Fn(&BuiltStar) -> bool,
) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (idx, star) in stars.iter().enumerate() {
        if predicate(star) && best.is_none_or(|(best_mag, _)| star.g_mag < best_mag) {
            best = Some((star.g_mag, idx));
        }
    }
    best.map(|(_, idx)| idx)
}

fn apply_iau_metadata(star: &mut BuiltStar, iau: &IauNameEntry) {
    star.metadata.proper_name = Some(iau.proper_name.clone());
    if star.metadata.hip.is_none() {
        star.metadata.hip = iau.hip;
    }
    if star.metadata.hd.is_none() {
        star.metadata.hd = iau.hd;
    }
    if star.metadata.hr.is_none() {
        star.metadata.hr = iau.hr;
    }
    if star.metadata.bayer_flamsteed.is_none() {
        star.metadata
            .bayer_flamsteed
            .clone_from(&iau.bayer_flamsteed);
    }
}

pub(crate) fn enrich_catalog(
    stars: &mut Vec<BuiltStar>,
    bright_hipparcos: &[HipparcosStar],
    bsc5: &[Bsc5Entry],
    iau_entries: &[IauNameEntry],
    max_mag: f32,
) -> EnrichmentStats {
    let mut stats = EnrichmentStats::default();

    // 1. Sort BSC5 brightest-first so primary components claim unresolved Gaia stars before fainter companions.
    let mut sorted_bsc5: Vec<&Bsc5Entry> = bsc5.iter().collect();
    sorted_bsc5.sort_by(|a, b| a.g_est.total_cmp(&b.g_est).then_with(|| a.hr.cmp(&b.hr)));

    let gaia_index = DecBandIndex::build(stars);
    let mut supplemental = Vec::new();
    let mut added_hrs = HashSet::new();

    for bsc in sorted_bsc5 {
        let query = MatchQuery {
            ra_deg: bsc.ra_deg,
            dec_deg: bsc.dec_deg,
            target_mag: Some(bsc.g_est),
            max_sep_arcsec: 20.0,
            max_dmag: 2.5,
        };
        let unclaimed_match = gaia_index.find_best_match(stars, query, |s| s.metadata.hr.is_none());

        if let Some(idx) = unclaimed_match {
            let star = &mut stars[idx];
            star.metadata.hr = Some(bsc.hr);
            if star.metadata.hd.is_none() {
                star.metadata.hd = bsc.hd;
            }
            if star.metadata.bayer_flamsteed.is_none() {
                star.metadata
                    .bayer_flamsteed
                    .clone_from(&bsc.bayer_flamsteed);
            }
            stats.matched_bsc5 += 1;
            continue;
        }

        let already_claimed = gaia_index.find_best_match(stars, query, |s| s.metadata.hr.is_some());

        if already_claimed.is_some() {
            // An already-attached brighter BSC5 entry shares this unresolved Gaia star.
            continue;
        }

        if bsc.g_est <= max_mag && added_hrs.insert(bsc.hr) {
            let record = StarRecord::from_icrs_spherical(
                bsc.ra_deg.to_radians(),
                bsc.dec_deg.to_radians(),
                bsc.g_est,
                bsc.bp_rp_est,
            );
            supplemental.push(BuiltStar {
                record,
                ra_deg: bsc.ra_deg,
                dec_deg: bsc.dec_deg,
                g_mag: bsc.g_est,
                metadata: StarMetadata {
                    proper_name: None,
                    bayer_flamsteed: bsc.bayer_flamsteed.clone(),
                    hip: None,
                    hd: bsc.hd,
                    hr: Some(bsc.hr),
                    tyc: None,
                },
            });
            stats.supplemental_added += 1;
        }
    }

    stars.extend(supplemental);

    // 2. Match Hipparcos IDs (J1991.25 propagated to J2016.0) onto stars missing `hip`.
    let full_index = DecBandIndex::build(stars);
    for hip_star in bright_hipparcos {
        let matched = full_index.find_best_match(
            stars,
            MatchQuery {
                ra_deg: hip_star.ra_deg,
                dec_deg: hip_star.dec_deg,
                target_mag: Some(hip_star.hp_mag),
                max_sep_arcsec: 12.0,
                max_dmag: 2.5,
            },
            |s| s.metadata.hip.is_none(),
        );
        if let Some(idx) = matched {
            stars[idx].metadata.hip = Some(hip_star.hip);
            stats.matched_hipparcos += 1;
        }
    }

    // 3. Match IAU proper names: HR first (handles shared-HIP binaries), then HD, then HIP, then sky pos.
    let mut iau_matched = vec![false; iau_entries.len()];

    for (i, iau) in iau_entries.iter().enumerate() {
        let Some(hr) = iau.hr else { continue };
        if let Some(idx) = find_brightest_matching_star(stars, |s| {
            s.metadata.proper_name.is_none() && s.metadata.hr == Some(hr)
        }) {
            apply_iau_metadata(&mut stars[idx], iau);
            iau_matched[i] = true;
            stats.matched_iau_names += 1;
        }
    }

    for (i, iau) in iau_entries.iter().enumerate() {
        if iau_matched[i] {
            continue;
        }
        let Some(hd) = iau.hd else { continue };
        if let Some(idx) = find_brightest_matching_star(stars, |s| {
            s.metadata.proper_name.is_none() && s.metadata.hd == Some(hd)
        }) {
            apply_iau_metadata(&mut stars[idx], iau);
            iau_matched[i] = true;
            stats.matched_iau_names += 1;
        }
    }

    for (i, iau) in iau_entries.iter().enumerate() {
        if iau_matched[i] {
            continue;
        }
        let Some(hip) = iau.hip else { continue };
        if let Some(idx) = find_brightest_matching_star(stars, |s| {
            s.metadata.proper_name.is_none() && s.metadata.hip == Some(hip)
        }) {
            apply_iau_metadata(&mut stars[idx], iau);
            iau_matched[i] = true;
            stats.matched_iau_names += 1;
        }
    }

    for (i, iau) in iau_entries.iter().enumerate() {
        if iau_matched[i] {
            continue;
        }
        let (Some(ra), Some(dec)) = (iau.ra_deg, iau.dec_deg) else {
            continue;
        };
        if let Some(idx) = full_index.find_best_match(
            stars,
            MatchQuery {
                ra_deg: ra,
                dec_deg: dec,
                target_mag: iau.mag,
                max_sep_arcsec: 15.0,
                max_dmag: 2.5,
            },
            |s| s.metadata.proper_name.is_none(),
        ) {
            apply_iau_metadata(&mut stars[idx], iau);
            iau_matched[i] = true;
            stats.matched_iau_names += 1;
        }
    }

    // 4. Prune redundant Tycho-2 IDs whenever a higher-priority name/designation is present.
    for star in stars.iter_mut() {
        let has_higher_id = star.metadata.proper_name.is_some()
            || star.metadata.bayer_flamsteed.is_some()
            || star.metadata.hip.is_some()
            || star.metadata.hd.is_some()
            || star.metadata.hr.is_some();
        if has_higher_id {
            star.metadata.tyc = None;
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsc5_bayer_and_flamsteed_formatting() {
        assert_eq!(
            format_bsc5_designation("  9Alp CMa").as_deref(),
            Some("α CMa")
        );
        assert_eq!(
            format_bsc5_designation("   Gam Cas").as_deref(),
            Some("γ Cas")
        );
        assert_eq!(
            format_bsc5_designation(" 34Mu 1Sco").as_deref(),
            Some("μ¹ Sco")
        );
        assert_eq!(
            format_bsc5_designation("  1Pi 3Ori").as_deref(),
            Some("π³ Ori")
        );
        assert_eq!(
            format_bsc5_designation(" 61    Cyg").as_deref(),
            Some("61 Cyg")
        );
        assert_eq!(format_bsc5_designation("          "), None);
    }

    #[test]
    fn bayer_superscript_normalization_leaves_flamsteed_alone() {
        assert_eq!(normalize_bayer_superscripts("θ1 Eri"), "θ¹ Eri");
        assert_eq!(normalize_bayer_superscripts("α2 CVn"), "α² CVn");
        assert_eq!(normalize_bayer_superscripts("39 Ari"), "39 Ari");
    }

    #[test]
    fn bv_to_gaia_calibration_matches_sirius_and_red_giants() {
        let (g_sirius, bprp_sirius) = vmag_bv_to_gaia_photometry(-1.46, Some(0.00));
        assert!((g_sirius - (-1.48)).abs() < 0.05);
        assert!(bprp_sirius.abs() < 0.05);

        let (g_red, bprp_red) = vmag_bv_to_gaia_photometry(0.50, Some(1.85));
        assert!(g_red < 0.0);
        assert!(bprp_red > 2.0);
    }

    #[test]
    fn proper_motion_propagation_shifts_coords_accurately() {
        let (ra, dec) = propagate_j2000_arcsec(0.0, 0.0, 3.6, -3.6, 10.0);
        assert!((ra - 0.01).abs() < 1e-6);
        assert!((dec - (-0.01)).abs() < 1e-6);
    }

    #[test]
    fn iau_matching_separates_shared_hip_binaries_by_hr() {
        let make_star = |ra: f64, dec: f64, mag: f32| BuiltStar {
            record: StarRecord::from_icrs_spherical(ra.to_radians(), dec.to_radians(), mag, 0.9),
            ra_deg: ra,
            dec_deg: dec,
            g_mag: mag,
            metadata: StarMetadata::default(),
        };

        // Simulates ε Boo A (Izar, HR 5506) and B (Pulcherrima, HR 5505) 3" apart sharing HIP 72105.
        let mut stars = vec![
            make_star(221.24676, 27.07421, 2.18),
            make_star(221.24760, 27.07480, 4.94),
        ];
        let bsc5 = vec![
            Bsc5Entry {
                hr: 5506,
                hd: Some(129988),
                bayer_flamsteed: Some("ε Boo".to_string()),
                ra_deg: 221.24676,
                dec_deg: 27.07421,
                g_est: 2.18,
                bp_rp_est: 0.9,
            },
            Bsc5Entry {
                hr: 5505,
                hd: Some(129989),
                bayer_flamsteed: Some("ε Boo".to_string()),
                ra_deg: 221.24760,
                dec_deg: 27.07480,
                g_est: 4.94,
                bp_rp_est: 0.5,
            },
        ];
        let hipparcos = vec![HipparcosStar {
            hip: 72105,
            ra_deg: 221.24676,
            dec_deg: 27.07421,
            hp_mag: 2.5,
        }];
        let iau = vec![
            IauNameEntry {
                proper_name: "Izar".to_string(),
                hr: Some(5506),
                hd: Some(129988),
                hip: Some(72105),
                bayer_flamsteed: Some("ε Boo".to_string()),
                mag: Some(2.35),
                ra_deg: Some(221.24676),
                dec_deg: Some(27.07421),
            },
            IauNameEntry {
                proper_name: "Pulcherrima".to_string(),
                hr: Some(5505),
                hd: Some(129989),
                hip: Some(72105),
                bayer_flamsteed: Some("ε Boo".to_string()),
                mag: Some(5.12),
                ra_deg: Some(221.24760),
                dec_deg: Some(27.07480),
            },
        ];

        enrich_catalog(&mut stars, &hipparcos, &bsc5, &iau, 8.0);
        assert_eq!(stars[0].metadata.proper_name.as_deref(), Some("Izar"));
        assert_eq!(stars[0].metadata.hr, Some(5506));
        assert_eq!(
            stars[1].metadata.proper_name.as_deref(),
            Some("Pulcherrima")
        );
        assert_eq!(stars[1].metadata.hr, Some(5505));
    }
}
