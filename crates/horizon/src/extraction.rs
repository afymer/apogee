use std::io::Cursor;
use std::sync::Arc;

use celestial::{DEFAULT_HORIZON_SAMPLES, MAX_HORIZON_ALT_DEG, MIN_HORIZON_ALT_DEG};
use gpui::RenderImage;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, Frame, ImageDecoder, ImageReader, RgbaImage};

/// Working panorama width used for fast horizon extraction (`0.5°` azimuthal resolution).
pub const WORKING_WIDTH: u32 = 720;
/// Working panorama height used for fast horizon extraction (`0.5°` vertical resolution).
pub const WORKING_HEIGHT: u32 = 360;

/// Maximum managed asset width stored inside the app persistence folder.
pub const MAX_MANAGED_WIDTH: u32 = 4096;
/// Maximum managed asset height stored inside the app persistence folder.
pub const MAX_MANAGED_HEIGHT: u32 = 2048;

/// Maximum preview texture width decoded for the GPUI horizon editor canvas.
pub const EDITOR_PREVIEW_MAX_WIDTH: u32 = 2048;
/// Maximum preview texture height decoded for the GPUI horizon editor canvas.
pub const EDITOR_PREVIEW_MAX_HEIGHT: u32 = 1024;

/// Sidebar list thumbnail width.
pub const THUMBNAIL_WIDTH: u32 = 160;
/// Sidebar list thumbnail height.
pub const THUMBNAIL_HEIGHT: u32 = 80;

/// Parameters controlling automatic equirectangular horizon extraction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizonExtractionParams {
    /// Shifts pixel sky classification bias in `[-1.0, 1.0]` (`+` classifies brighter/hazy areas as sky).
    pub sky_bias: f32,
    /// Horizontal smoothness regularization weight in `[0.0, 1.0]`.
    pub smoothness: f32,
    /// Uniform altitude offset applied after extraction in degrees.
    pub altitude_margin_deg: f32,
}

impl Default for HorizonExtractionParams {
    fn default() -> Self {
        Self {
            sky_bias: 0.0,
            smoothness: 0.35,
            altitude_margin_deg: 0.0,
        }
    }
}

/// Compact `720×360` RGBA8 buffer cached in memory for instant interactive re-extraction.
#[derive(Clone, Debug)]
pub struct WorkingPanorama {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Validated, EXIF-oriented, and resized panorama ready for disk persistence and extraction.
#[derive(Clone, Debug)]
pub struct PreparedPanorama {
    pub managed_bytes: Vec<u8>,
    pub asset_ext: &'static str,
    pub dimensions: (u32, u32),
    pub working: WorkingPanorama,
    pub aspect_ratio_warning: Option<String>,
}

fn decode_oriented_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("Failed to read image header: {e}"))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Unsupported or corrupt image file: {e}"))?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| format!("Failed to decode image: {e}"))?;
    img.apply_orientation(orientation);

    if img.width() < 32 || img.height() < 16 {
        return Err(format!(
            "Image is too small ({}×{}). Minimum supported size is 32×16.",
            img.width(),
            img.height()
        ));
    }
    Ok(img)
}

fn fit_dimensions(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    let sx = f64::from(max_w) / f64::from(width.max(1));
    let sy = f64::from(max_h) / f64::from(height.max(1));
    let scale = sx.min(sy);
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
}

fn rgba_to_gpui_render_image(mut rgba: RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(rgba)]))
}

fn make_working_panorama(img: &DynamicImage) -> WorkingPanorama {
    let resized = img.resize_exact(WORKING_WIDTH, WORKING_HEIGHT, FilterType::Triangle);
    let rgba = resized.to_rgba8();
    WorkingPanorama {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }
}

/// Decodes a user-selected 360° image file, applies EXIF orientation, caps dimensions to
/// `4096×2048`, encodes it for managed asset storage, and builds a `720×360` working buffer.
pub fn decode_and_prepare_panorama(bytes: &[u8]) -> Result<PreparedPanorama, String> {
    let img = decode_oriented_image(bytes)?;
    let orig_w = img.width();
    let orig_h = img.height();

    let aspect = f64::from(orig_w) / f64::from(orig_h.max(1));
    let aspect_ratio_warning = if (aspect - 2.0).abs() > 0.25 {
        Some(format!(
            "Imported image aspect ratio is {aspect:.2}:1 ({orig_w}×{orig_h}). Equirectangular 360° panoramas are normally 2:1."
        ))
    } else {
        None
    };

    let (target_w, target_h) =
        fit_dimensions(orig_w, orig_h, MAX_MANAGED_WIDTH, MAX_MANAGED_HEIGHT);
    let managed = if target_w == orig_w && target_h == orig_h {
        img
    } else {
        img.resize_exact(target_w, target_h, FilterType::Triangle)
    };

    let working = make_working_panorama(&managed);
    let managed_rgba = managed.to_rgba8();
    let has_alpha = managed.color().has_alpha() && managed_rgba.pixels().any(|p| p[3] < 250);

    let mut encoded = Vec::new();
    let asset_ext = if has_alpha {
        DynamicImage::ImageRgba8(managed_rgba)
            .write_with_encoder(PngEncoder::new(&mut encoded))
            .map_err(|e| format!("Failed to encode managed PNG panorama: {e}"))?;
        "png"
    } else {
        let rgb = DynamicImage::ImageRgba8(managed_rgba).to_rgb8();
        DynamicImage::ImageRgb8(rgb)
            .write_with_encoder(JpegEncoder::new_with_quality(&mut encoded, 90))
            .map_err(|e| format!("Failed to encode managed JPEG panorama: {e}"))?;
        "jpg"
    };

    Ok(PreparedPanorama {
        managed_bytes: encoded,
        asset_ext,
        dimensions: (target_w, target_h),
        working,
        aspect_ratio_warning,
    })
}

/// Decodes persisted panorama bytes into a GPUI BGRA [`RenderImage`] capped at `max_w × max_h`
/// alongside a `720×360` [`WorkingPanorama`] for instant re-extraction.
pub fn decode_for_gpui_preview(
    bytes: &[u8],
    max_w: u32,
    max_h: u32,
) -> Result<(Arc<RenderImage>, WorkingPanorama), String> {
    let img = decode_oriented_image(bytes)?;
    let working = make_working_panorama(&img);
    let (pw, ph) = fit_dimensions(img.width(), img.height(), max_w.max(32), max_h.max(16));
    let preview = if pw == img.width() && ph == img.height() {
        img.to_rgba8()
    } else {
        img.resize_exact(pw, ph, FilterType::Triangle).to_rgba8()
    };
    Ok((rgba_to_gpui_render_image(preview), working))
}

/// Decodes persisted panorama bytes into a small `160×80` BGRA [`RenderImage`] thumbnail.
pub fn decode_thumbnail_render_image(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let img = decode_oriented_image(bytes)?;
    let thumb = img
        .resize_exact(THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT, FilterType::Triangle)
        .to_rgba8();
    Ok(rgba_to_gpui_render_image(thumb))
}

/// Decodes persisted panorama bytes into RGBA8 dimensions + pixel payload for Bevy GPU upload.
pub fn decode_rgba_for_planetarium(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let img = decode_oriented_image(bytes)?;
    let (tw, th) = fit_dimensions(
        img.width(),
        img.height(),
        MAX_MANAGED_WIDTH,
        MAX_MANAGED_HEIGHT,
    );
    let rgba = if tw == img.width() && th == img.height() {
        img.to_rgba8()
    } else {
        img.resize_exact(tw, th, FilterType::Triangle).to_rgba8()
    };
    let dims = rgba.dimensions();
    Ok((dims.0, dims.1, rgba.into_raw()))
}

#[derive(Clone, Copy)]
struct SectorStats {
    sky_clear: [f32; 3],
    sky_bright: [f32; 3],
    sky_lum_ref: f32,
    gnd_dark: [f32; 3],
    gnd_mid: [f32; 3],
    gnd_lum_dark: f32,
    gnd_lum_mid: f32,
}

#[inline]
fn rgb_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
fn color_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    (dr * dr + dg * dg + db * db).sqrt()
}

#[inline]
fn lerp_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn percentile_by_luma(samples: &mut [([f32; 3], f32)], pct: f32) -> ([f32; 3], f32) {
    if samples.is_empty() {
        return ([0.0, 0.0, 0.0], 0.0);
    }
    samples.sort_by(|a, b| a.1.total_cmp(&b.1));
    let idx = ((samples.len() - 1) as f32 * pct.clamp(0.0, 1.0)).round() as usize;
    samples[idx]
}

/// Extracts a 360° horizon profile (in degrees, `DEFAULT_HORIZON_SAMPLES` bins)
/// from a `WorkingPanorama` using sector-adaptive color/luminance scoring and
/// exact circular Viterbi dynamic programming.
pub fn extract_horizon_from_working(
    working: &WorkingPanorama,
    params: HorizonExtractionParams,
) -> Vec<f32> {
    let w = working.width as usize;
    let h = working.height as usize;
    if w < 4 || h < 8 || working.rgba.len() < w * h * 4 {
        return vec![0.0; DEFAULT_HORIZON_SAMPLES];
    }

    let pixel_count = w * h;
    let mut rgb_buf = vec![[0.0_f32; 3]; pixel_count];
    let mut lum_buf = vec![0.0_f32; pixel_count];
    let mut alpha_buf = vec![255_u8; pixel_count];

    let upper_rows = h / 2;
    let mut upper_transparent = 0usize;
    let upper_total = upper_rows * w;

    for row in 0..h {
        let row_off = row * w;
        for col in 0..w {
            let idx = row_off + col;
            let b_off = idx * 4;
            let red = f32::from(working.rgba[b_off]) / 255.0;
            let green = f32::from(working.rgba[b_off + 1]) / 255.0;
            let blue = f32::from(working.rgba[b_off + 2]) / 255.0;
            let alpha = working.rgba[b_off + 3];
            let rgb = [red, green, blue];
            rgb_buf[idx] = rgb;
            lum_buf[idx] = rgb_luminance(rgb);
            alpha_buf[idx] = alpha;
            if row < upper_rows && alpha < 128 {
                upper_transparent += 1;
            }
        }
    }

    let is_alpha_masked = upper_total > 0 && upper_transparent * 10 > upper_total;
    let mut sky_score = vec![0.0_f32; pixel_count];

    if is_alpha_masked {
        for idx in 0..pixel_count {
            let base = 1.0 - f32::from(alpha_buf[idx]) / 255.0;
            sky_score[idx] = (base + 0.25 * params.sky_bias).clamp(0.0, 1.0);
        }
    } else {
        const NUM_SECTORS: usize = 24;
        let sector_w = (w as f32) / (NUM_SECTORS as f32);
        let half_span = (w / NUM_SECTORS).max(8) as isize;
        let zenith_y_start = 2usize.min(h / 7);
        let zenith_y_end = (h / 7).max(zenith_y_start + 1);
        let bright_y_end = (h / 4).max(zenith_y_start + 1);
        let gnd_y_start = (h * 62 / 100).min(h.saturating_sub(2));
        let gnd_y_end = (h * 88 / 100).max(gnd_y_start + 1).min(h);

        let mut sectors = Vec::with_capacity(NUM_SECTORS);
        let mut sky_upper = Vec::with_capacity(256);
        let mut sky_wide = Vec::with_capacity(512);
        let mut gnd_samples = Vec::with_capacity(512);

        for s in 0..NUM_SECTORS {
            let center_x = (((s as f32) + 0.5) * sector_w).round() as isize;
            sky_upper.clear();
            sky_wide.clear();
            gnd_samples.clear();

            for dx in (-half_span..=half_span).step_by(2) {
                let x = (center_x + dx).rem_euclid(w as isize) as usize;
                for y in (zenith_y_start..zenith_y_end).step_by(2) {
                    let idx = y * w + x;
                    sky_upper.push((rgb_buf[idx], lum_buf[idx]));
                }
                for y in (zenith_y_start..bright_y_end).step_by(2) {
                    let idx = y * w + x;
                    sky_wide.push((rgb_buf[idx], lum_buf[idx]));
                }
                for y in (gnd_y_start..gnd_y_end).step_by(2) {
                    let idx = y * w + x;
                    gnd_samples.push((rgb_buf[idx], lum_buf[idx]));
                }
            }

            let (sky_clear, _) = percentile_by_luma(&mut sky_upper, 0.50);
            let (sky_bright, _) = percentile_by_luma(&mut sky_wide, 0.85);
            let (_, sky_lum_ref) = percentile_by_luma(&mut sky_wide, 0.60);
            let (gnd_dark, gnd_lum_dark) = percentile_by_luma(&mut gnd_samples, 0.25);
            let (gnd_mid, gnd_lum_mid) = percentile_by_luma(&mut gnd_samples, 0.65);

            sectors.push(SectorStats {
                sky_clear,
                sky_bright,
                sky_lum_ref,
                gnd_dark,
                gnd_mid,
                gnd_lum_dark,
                gnd_lum_mid,
            });
        }

        for col in 0..w {
            let sf = (col as f32 + 0.5) / sector_w - 0.5;
            let s0_i = sf.floor() as isize;
            let blend = sf - s0_i as f32;
            let s0 = s0_i.rem_euclid(NUM_SECTORS as isize) as usize;
            let s1 = (s0 + 1) % NUM_SECTORS;

            let sec_a = sectors[s0];
            let sec_b = sectors[s1];
            let sky_clear = lerp_rgb(sec_a.sky_clear, sec_b.sky_clear, blend);
            let sky_bright = lerp_rgb(sec_a.sky_bright, sec_b.sky_bright, blend);
            let sky_lum_ref = sec_a.sky_lum_ref + (sec_b.sky_lum_ref - sec_a.sky_lum_ref) * blend;
            let gnd_dark = lerp_rgb(sec_a.gnd_dark, sec_b.gnd_dark, blend);
            let gnd_mid = lerp_rgb(sec_a.gnd_mid, sec_b.gnd_mid, blend);
            let gnd_lum_dark =
                sec_a.gnd_lum_dark + (sec_b.gnd_lum_dark - sec_a.gnd_lum_dark) * blend;
            let gnd_lum_mid = sec_a.gnd_lum_mid + (sec_b.gnd_lum_mid - sec_a.gnd_lum_mid) * blend;

            let sky_brighter_than_ground = sky_lum_ref > gnd_lum_dark + 0.05;
            let lum_mid = f32::midpoint(gnd_lum_mid, sky_lum_ref);
            let lum_span = (sky_lum_ref - gnd_lum_dark).max(0.08);

            for row in 0..h {
                let idx = row * w + col;
                let rgb = rgb_buf[idx];
                let lum = lum_buf[idx];

                let d_sky = color_distance(rgb, sky_clear).min(color_distance(rgb, sky_bright));
                let d_gnd = color_distance(rgb, gnd_dark).min(color_distance(rgb, gnd_mid));
                let p_color = d_gnd / (d_sky + d_gnd + 0.05);

                let p_luma = ((lum - lum_mid) / lum_span + 0.5).clamp(0.0, 1.0);
                let blue_excess =
                    ((rgb[2] - f32::midpoint(rgb[0], rgb[1])).clamp(0.0, 0.35)) / 0.35;

                let base = if sky_brighter_than_ground {
                    0.62 * p_color + 0.25 * p_luma + 0.13 * blue_excess
                } else {
                    0.82 * p_color + 0.18 * blue_excess
                };

                sky_score[idx] = (base + 0.35 * params.sky_bias).clamp(0.0, 1.0);
            }
        }
    }

    let y_min = (h * 4 / 100).max(2);
    let y_max = (h * 64 / 100).min(h - 3).max(y_min + 1);
    let m = y_max - y_min + 1;

    // Precompute column data costs `E_data(x, y_h)` in O(W * H).
    let mut e_data = vec![0.0_f32; w * m];
    let mut prefix = vec![0.0_f32; h + 1];

    for x in 0..w {
        prefix[0] = 0.0;
        for y in 0..h {
            prefix[y + 1] = prefix[y] + sky_score[y * w + x];
        }
        let p_bottom = prefix[y_max + 1];

        for u in 0..m {
            let y_h = y_min + u;
            let err_sky = (y_h as f32) - prefix[y_h];
            let err_gnd = p_bottom - prefix[y_h];

            let edge_bonus = if is_alpha_masked {
                let a_above = f32::from(alpha_buf[(y_h - 1) * w + x]) / 255.0;
                let a_below = f32::from(alpha_buf[y_h * w + x]) / 255.0;
                2.5 * (a_below - a_above).max(0.0)
            } else {
                let idx_above = (y_h - 2) * w + x;
                let idx_below = (y_h + 1) * w + x;
                let l_drop = (lum_buf[idx_above] - lum_buf[idx_below]).max(0.0);
                let c_jump = color_distance(rgb_buf[idx_above], rgb_buf[idx_below]);
                2.5 * l_drop + 1.5 * c_jump
            };

            e_data[x * m + u] = err_sky + err_gnd - edge_bonus;
        }
    }

    // Solve exact circular Viterbi DP over 2*W columns with truncated L1 pairwise penalty.
    let smoothness = params.smoothness.clamp(0.0, 1.0);
    let lambda = 0.15 + smoothness * 2.2;
    let cap = 16.0_f32;
    let total_steps = 2 * w;

    let mut dp_prev = vec![0.0_f32; m];
    let mut dp_curr = vec![0.0_f32; m];
    let mut fwd_val = vec![0.0_f32; m];
    let mut fwd_idx = vec![0_u16; m];
    let mut backptr = vec![0_u16; total_steps * m];

    dp_prev[..m].copy_from_slice(&e_data[..m]);

    for step in 1..total_steps {
        let col = step % w;
        let col_offset = col * m;
        let bp_offset = step * m;

        // Global minimum for jump transitions (|u - v| > cap).
        let mut global_min_dp = f32::INFINITY;
        let mut global_arg = 0_u16;
        for (v, &val) in dp_prev.iter().enumerate() {
            if val < global_min_dp {
                global_min_dp = val;
                global_arg = v as u16;
            }
        }
        let global_jump_cost = global_min_dp + lambda * cap;

        // Forward envelope pass.
        let mut best_val = f32::INFINITY;
        let mut best_idx = 0_u16;
        for u in 0..m {
            best_val += lambda;
            if dp_prev[u] < best_val {
                best_val = dp_prev[u];
                best_idx = u as u16;
            }
            fwd_val[u] = best_val;
            fwd_idx[u] = best_idx;
        }

        // Backward envelope pass + combine with data term.
        let mut bwd_best = f32::INFINITY;
        let mut bwd_idx = (m - 1) as u16;
        for u in (0..m).rev() {
            bwd_best += lambda;
            if dp_prev[u] < bwd_best {
                bwd_best = dp_prev[u];
                bwd_idx = u as u16;
            }

            let (mut trans_cost, mut prev_u) = if fwd_val[u] <= bwd_best {
                (fwd_val[u], fwd_idx[u])
            } else {
                (bwd_best, bwd_idx)
            };

            if global_jump_cost < trans_cost {
                trans_cost = global_jump_cost;
                prev_u = global_arg;
            }

            dp_curr[u] = e_data[col_offset + u] + trans_cost;
            backptr[bp_offset + u] = prev_u;
        }

        std::mem::swap(&mut dp_prev, &mut dp_curr);
    }

    let mut cur_u = 0_u16;
    let mut best_final = f32::INFINITY;
    for (u, &val) in dp_prev.iter().enumerate() {
        if val < best_final {
            best_final = val;
            cur_u = u as u16;
        }
    }

    let mut path_u = vec![0_u16; total_steps];
    for step in (0..total_steps).rev() {
        path_u[step] = cur_u;
        cur_u = backptr[step * m + usize::from(cur_u)];
    }

    let mut raw_alt_deg = Vec::with_capacity(w);
    for x in 0..w {
        let y_h = y_min + usize::from(path_u[w + x]);
        let alt = 90.0 - (y_h as f32 / h as f32) * 180.0 + params.altitude_margin_deg;
        raw_alt_deg.push(alt.clamp(MIN_HORIZON_ALT_DEG, MAX_HORIZON_ALT_DEG));
    }

    if raw_alt_deg.len() == DEFAULT_HORIZON_SAMPLES {
        raw_alt_deg
    } else {
        (0..DEFAULT_HORIZON_SAMPLES)
            .map(|i| {
                let pos = (i as f32 / DEFAULT_HORIZON_SAMPLES as f32) * raw_alt_deg.len() as f32;
                let i0 = (pos.floor() as usize) % raw_alt_deg.len();
                let i1 = (i0 + 1) % raw_alt_deg.len();
                let frac = pos - pos.floor();
                raw_alt_deg[i0] + (raw_alt_deg[i1] - raw_alt_deg[i0]) * frac
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_synthetic_working(horizon_fn: impl Fn(usize, usize) -> f32) -> WorkingPanorama {
        let w = WORKING_WIDTH as usize;
        let h = WORKING_HEIGHT as usize;
        let mut rgba = vec![0_u8; w * h * 4];
        for x in 0..w {
            let h_alt = horizon_fn(x, w);
            for y in 0..h {
                let alt = 90.0 - ((y as f32 + 0.5) / h as f32) * 180.0;
                let is_sky = alt >= h_alt;
                let off = (y * w + x) * 4;
                if is_sky {
                    rgba[off] = 115;
                    rgba[off + 1] = 165;
                    rgba[off + 2] = 240;
                    rgba[off + 3] = 255;
                } else {
                    rgba[off] = 38;
                    rgba[off + 1] = 56;
                    rgba[off + 2] = 30;
                    rgba[off + 3] = 255;
                }
            }
        }
        WorkingPanorama {
            width: WORKING_WIDTH,
            height: WORKING_HEIGHT,
            rgba,
        }
    }

    #[test]
    fn extracts_flat_horizon_accurately() {
        let working = make_synthetic_working(|_, _| 10.0);
        let samples = extract_horizon_from_working(&working, HorizonExtractionParams::default());
        assert_eq!(samples.len(), DEFAULT_HORIZON_SAMPLES);
        for (idx, &alt) in samples.iter().enumerate() {
            assert!(
                (alt - 10.0).abs() <= 1.5,
                "bin {idx}: expected ~10.0 deg, got {alt}"
            );
        }
    }

    #[test]
    fn extracts_stepped_building_silhouette() {
        let working = make_synthetic_working(|x, w| {
            let az = (x as f32 / w as f32) * 360.0;
            if (90.0..=180.0).contains(&az) {
                35.0
            } else {
                2.0
            }
        });
        let samples = extract_horizon_from_working(&working, HorizonExtractionParams::default());
        assert_eq!(samples.len(), DEFAULT_HORIZON_SAMPLES);

        // Mid-building at 135 deg -> bin 270
        assert!(
            (samples[270] - 35.0).abs() <= 2.0,
            "building bin got {}",
            samples[270]
        );
        // Open sky at 270 deg -> bin 540
        assert!(
            (samples[540] - 2.0).abs() <= 2.0,
            "open bin got {}",
            samples[540]
        );
    }

    #[test]
    fn extracts_alpha_masked_panorama_and_applies_margin() {
        let w = WORKING_WIDTH as usize;
        let h = WORKING_HEIGHT as usize;
        let mut rgba = vec![0_u8; w * h * 4];
        for x in 0..w {
            for y in 0..h {
                let alt = 90.0 - ((y as f32 + 0.5) / h as f32) * 180.0;
                let off = (y * w + x) * 4;
                rgba[off] = 40;
                rgba[off + 1] = 45;
                rgba[off + 2] = 50;
                rgba[off + 3] = if alt >= 15.0 { 0 } else { 255 };
            }
        }
        let working = WorkingPanorama {
            width: WORKING_WIDTH,
            height: WORKING_HEIGHT,
            rgba,
        };
        let base = extract_horizon_from_working(&working, HorizonExtractionParams::default());
        assert!((base[0] - 15.0).abs() <= 1.5, "got {}", base[0]);

        let shifted = extract_horizon_from_working(
            &working,
            HorizonExtractionParams {
                altitude_margin_deg: 5.0,
                ..Default::default()
            },
        );
        assert!((shifted[0] - (base[0] + 5.0)).abs() < 1e-4);
    }

    #[test]
    fn sky_bias_shifts_ambiguous_gradient_horizon_monotonically() {
        let width = WORKING_WIDTH as usize;
        let height = WORKING_HEIGHT as usize;
        let mut rgba = vec![0_u8; width * height * 4];
        for y in 0..height {
            // Smooth twilight/haze vertical gradient between y=60 and y=240
            let blend = ((y as f32 - 60.0) / 180.0).clamp(0.0, 1.0);
            let red = (180.0 * (1.0 - blend) + 30.0 * blend) as u8;
            let green = (200.0 * (1.0 - blend) + 40.0 * blend) as u8;
            let blue = (230.0 * (1.0 - blend) + 35.0 * blend) as u8;
            for x in 0..width {
                let off = (y * width + x) * 4;
                rgba[off] = red;
                rgba[off + 1] = green;
                rgba[off + 2] = blue;
                rgba[off + 3] = 255;
            }
        }
        let working = WorkingPanorama {
            width: WORKING_WIDTH,
            height: WORKING_HEIGHT,
            rgba,
        };

        let conservative = extract_horizon_from_working(
            &working,
            HorizonExtractionParams {
                sky_bias: -0.8,
                ..Default::default()
            },
        );
        let permissive = extract_horizon_from_working(
            &working,
            HorizonExtractionParams {
                sky_bias: 0.8,
                ..Default::default()
            },
        );
        let avg_cons = conservative.iter().sum::<f32>() / conservative.len() as f32;
        let avg_perm = permissive.iter().sum::<f32>() / permissive.len() as f32;
        assert!(
            avg_cons > avg_perm,
            "expected negative sky_bias to yield higher horizon ({avg_cons}) than positive ({avg_perm})"
        );
    }

    #[test]
    fn prepare_panorama_encodes_and_warns_on_non_equirectangular_aspect() {
        let wide = RgbaImage::from_pixel(64, 16, image::Rgba([80, 120, 200, 255]));
        let mut png_bytes = Vec::new();
        DynamicImage::ImageRgba8(wide)
            .write_with_encoder(PngEncoder::new(&mut png_bytes))
            .unwrap();

        let prepared = decode_and_prepare_panorama(&png_bytes).expect("prepare succeeds");
        assert_eq!(prepared.asset_ext, "jpg");
        assert_eq!(prepared.dimensions, (64, 16));
        assert!(prepared.aspect_ratio_warning.is_some());

        let (render_img, working) =
            decode_for_gpui_preview(&prepared.managed_bytes, 1024, 512).expect("preview succeeds");
        assert_eq!(working.width, WORKING_WIDTH);
        assert_eq!(working.height, WORKING_HEIGHT);
        assert_eq!(render_img.size(0).width.0, 64);
    }
}
