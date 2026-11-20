use crate::observer::estimate_utc_offset_minutes_from_longitude;

const GRID_SCALE: usize = 6;
const ROW_LEN: usize = 360 * GRID_SCALE;
const N_ROWS: usize = 180 * GRID_SCALE + 1;
const ROW_TABLE_BYTES: usize = (N_ROWS + 1) * 2;

const GRID_BYTES: &[u8] = include_bytes!("tz_offset_grid.bin");

const STANDARD_OFFSET_MINUTES: [i16; 38] = [
    -720, -660, -600, -570, -540, -480, -420, -360, -300, -240, -210, -180, -120, -60, 0, 60, 120,
    180, 210, 240, 270, 300, 330, 345, 360, 390, 420, 480, 525, 540, 570, 600, 630, 660, 720, 765,
    780, 840,
];

#[inline]
fn read_u16_le(bytes: &[u8], index: usize) -> Option<u16> {
    let offset = index.checked_mul(2)?;
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Looks up the standard (non-DST) civil UTC offset in minutes from geodetic coordinates
/// using an embedded 1/6-degree (~10 arcmin) run-length encoded global timezone grid.
///
/// Falls back to [`estimate_utc_offset_minutes_from_longitude`] if either coordinate is
/// non-finite.
pub fn lookup_standard_utc_offset_minutes(latitude_deg: f64, longitude_deg: f64) -> i32 {
    if !latitude_deg.is_finite() || !longitude_deg.is_finite() {
        return estimate_utc_offset_minutes_from_longitude(longitude_deg);
    }

    let lat = latitude_deg.clamp(-90.0, 90.0);
    let lon = longitude_deg.clamp(-180.0, 180.0);

    let max_lat_idx = 180 * GRID_SCALE;
    let r_lat = ((lat + 90.0) * (GRID_SCALE as f64)).round() as usize;
    let row = max_lat_idx.saturating_sub(r_lat.min(max_lat_idx));
    let col = (((lon + 180.0) * (GRID_SCALE as f64)).round() as usize).min(ROW_LEN - 1);

    lookup_grid_cell(row, col)
        .unwrap_or_else(|| estimate_utc_offset_minutes_from_longitude(lon))
}

fn lookup_grid_cell(row: usize, col: usize) -> Option<i32> {
    let runs_bytes = GRID_BYTES.get(ROW_TABLE_BYTES..)?;
    let start = usize::from(read_u16_le(GRID_BYTES, row)?);
    let end = usize::from(read_u16_le(GRID_BYTES, row + 1)?);

    let mut cursor = 0_usize;
    for run_idx in start..end {
        let word = read_u16_le(runs_bytes, run_idx)?;
        let run_len = usize::from(word >> 6) + 1;
        cursor += run_len;
        if col < cursor {
            let offset_idx = usize::from(word & 0x3F);
            return STANDARD_OFFSET_MINUTES
                .get(offset_idx)
                .copied()
                .map(i32::from);
        }
    }
    None
}
