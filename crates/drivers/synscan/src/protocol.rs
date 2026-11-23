//! Sky-Watcher SynScan Hand Controller Serial Protocol (NexStar-compatible, 9600 8N1).
//!
//! References: Sky-Watcher SynScan Serial Communication Protocol v3.3 / v4.x / v5.x.

use protocol::{MountAxis, PierSide, TrackingRate};

use crate::types::MountError;

const FULL_32: f64 = 4_294_967_296.0; // 2^32
const FULL_16: f64 = 65_536.0; // 2^16

/// Encodes a finite angle in degrees as an 8-digit uppercase hex 32-bit unsigned fraction of 360°.
pub fn encode_precise_angle(deg: f64) -> Result<String, MountError> {
    if !deg.is_finite() {
        return Err(MountError::Protocol("Angle must be finite".into()));
    }
    let wrapped = deg.rem_euclid(360.0);
    let raw = ((wrapped / 360.0) * FULL_32).round() as u64 % (1u64 << 32);
    Ok(format!("{raw:08X}"))
}

/// Decodes an 8-hex-digit (32-bit) or 4-hex-digit (16-bit) SynScan angle into degrees `[0, 360)`.
pub fn decode_precise_angle(hex: &str) -> Option<f64> {
    let trimmed = hex.trim();
    match trimmed.len() {
        8 => {
            let raw = u32::from_str_radix(trimmed, 16).ok()?;
            Some((f64::from(raw) / FULL_32) * 360.0)
        }
        4 => {
            let raw = u16::from_str_radix(trimmed, 16).ok()?;
            Some((f64::from(raw) / FULL_16) * 360.0)
        }
        _ => None,
    }
}

/// Decodes a `AAAAAAAA,BBBBBBBB#` response into two angles in `[0, 360)`.
pub fn decode_precise_pair(resp: &str) -> Option<(f64, f64)> {
    let body = resp.trim().trim_end_matches('#');
    let (first, second) = body.split_once(',')?;
    Some((decode_precise_angle(first)?, decode_precise_angle(second)?))
}

/// Encodes a `(longitude_or_az_deg, signed_lat_or_alt_deg)` pair for SynScan `r`/`s`/`b`.
fn encode_precise_signed_pair(first_deg: f64, second_deg: f64) -> Result<String, MountError> {
    if !first_deg.is_finite() || !second_deg.is_finite() {
        return Err(MountError::Protocol("Coordinates must be finite".into()));
    }
    let second_clamped = second_deg.clamp(-90.0, 90.0);
    Ok(format!(
        "{},{}",
        encode_precise_angle(first_deg)?,
        encode_precise_angle(second_clamped)?
    ))
}

/// Decodes a `(longitude_or_az_deg, signed_lat_or_alt_deg)` pair from `e`/`z` responses.
fn decode_precise_signed_pair(resp: &str) -> Option<(f64, f64)> {
    let (first, raw_second) = decode_precise_pair(resp)?;
    let second = if raw_second > 180.0 {
        raw_second - 360.0
    } else {
        raw_second
    };
    Some((first.rem_euclid(360.0), second.clamp(-90.0, 90.0)))
}

/// Encodes equatorial `(ra_deg, dec_deg)` for SynScan precise GoTo (`r`) or Sync (`s`).
pub fn encode_precise_radec_payload(ra_deg: f64, dec_deg: f64) -> Result<String, MountError> {
    encode_precise_signed_pair(ra_deg, dec_deg)
}

/// Decodes equatorial `(ra_deg, dec_deg)` from an `e` response (`ra ∈ [0, 360)`, `dec ∈ [-90, 90]`).
pub fn decode_precise_radec(resp: &str) -> Option<(f64, f64)> {
    decode_precise_signed_pair(resp)
}

/// Encodes horizontal `(az_deg, alt_deg)` for SynScan precise Az/Alt GoTo (`b`).
pub fn encode_precise_azalt_payload(az_deg: f64, alt_deg: f64) -> Result<String, MountError> {
    encode_precise_signed_pair(az_deg, alt_deg)
}

/// Decodes horizontal `(az_deg, alt_deg)` from a `z` response (`az ∈ [0, 360)`, `alt ∈ [-90, 90]`).
pub fn decode_precise_azalt(resp: &str) -> Option<(f64, f64)> {
    decode_precise_signed_pair(resp)
}

/// Builds a SynScan `P` fixed-rate manual slew packet.
///
/// - `axis`: `MountAxis::Ra` (device 16) or `MountAxis::Dec` (device 17)
/// - `positive`: `true` for positive direction (cmd 36), `false` for negative (cmd 37)
/// - `rate`: `0` stops the axis; `1..=9` sets hand-controller slew speed
pub fn encode_fixed_rate_slew(axis: MountAxis, positive: bool, rate: u8) -> [u8; 8] {
    let dev_id = match axis {
        MountAxis::Ra => 16u8,
        MountAxis::Dec => 17u8,
    };
    let cmd_id = if positive { 36u8 } else { 37u8 };
    [b'P', 2, dev_id, cmd_id, rate.min(9), 0, 0, 0]
}

/// Builds a SynScan `T` set-tracking-mode command (`0`=Off, `2`=EQ, `3`=EQ+PEC).
///
/// Note: The SynScan hand controller serial protocol only exposes `Off`, `EQ` (sidereal),
/// and `EQ+PEC`; `Lunar` and `Solar` map to `EQ` mode on hardware.
pub fn encode_tracking_mode(rate: TrackingRate) -> [u8; 2] {
    let code = match rate {
        TrackingRate::Off => 0u8,
        TrackingRate::Sidereal | TrackingRate::Lunar | TrackingRate::Solar => 2u8,
        TrackingRate::SiderealPec => 3u8,
    };
    [b'T', code]
}

/// Parses a SynScan `t` get-tracking-mode response byte.
pub fn decode_tracking_mode(raw: &[u8]) -> Option<TrackingRate> {
    let body = raw.strip_suffix(b"#").unwrap_or(raw);
    if body.len() != 1 {
        return None;
    }
    match body[0] {
        0 | b'0' => Some(TrackingRate::Off),
        1 | b'1' | 2 | b'2' => Some(TrackingRate::Sidereal),
        3 | b'3' => Some(TrackingRate::SiderealPec),
        _ => None,
    }
}

/// Parses a SynScan `p` pier-side response (`'E'` or `'W'`).
pub fn decode_pier_side(raw: &[u8]) -> PierSide {
    let body = raw.strip_suffix(b"#").unwrap_or(raw);
    match body.first().copied() {
        Some(b'E' | b'e') => PierSide::East,
        Some(b'W' | b'w') => PierSide::West,
        _ => PierSide::Unknown,
    }
}

/// Builds a SynScan `W` set-location packet (`[b'W', lat_d, lat_m, lat_s, lat_sign, lon_d, lon_m, lon_s, lon_sign]`).
pub fn encode_set_location(latitude_deg: f64, longitude_deg: f64) -> Result<[u8; 9], MountError> {
    if !latitude_deg.is_finite() || !longitude_deg.is_finite() {
        return Err(MountError::Protocol(
            "Site coordinates must be finite".into(),
        ));
    }
    let (lat_d, lat_m, lat_s, lat_sign) = deg_to_dms_synscan(latitude_deg.clamp(-90.0, 90.0));
    let (lon_d, lon_m, lon_s, lon_sign) = deg_to_dms_synscan(longitude_deg.clamp(-180.0, 180.0));
    Ok([
        b'W', lat_d, lat_m, lat_s, lat_sign, lon_d, lon_m, lon_s, lon_sign,
    ])
}

fn deg_to_dms_synscan(val: f64) -> (u8, u8, u8, u8) {
    let sign = u8::from(val < 0.0); // 0 = N/E, 1 = S/W
    let total_secs = (val.abs() * 3600.0).round() as u32;
    let d = (total_secs / 3600).min(180) as u8;
    let m = ((total_secs % 3600) / 60) as u8;
    let s = (total_secs % 60) as u8;
    (d, m, s, sign)
}

/// Parses a SynScan `V` firmware version response (6 ASCII hex chars or 2 binary bytes).
pub fn parse_firmware_version(raw: &[u8]) -> Option<String> {
    let body = raw.strip_suffix(b"#").unwrap_or(raw);
    if body.len() == 6 && body.iter().all(u8::is_ascii_hexdigit) {
        let s = std::str::from_utf8(body).ok()?;
        let major = u8::from_str_radix(&s[0..2], 16).ok()?;
        let minor = u8::from_str_radix(&s[2..4], 16).ok()?;
        let patch = u8::from_str_radix(&s[4..6], 16).ok()?;
        return Some(format!("{major}.{minor:02}.{patch:02}"));
    }
    if body.len() == 2 {
        return Some(format!("{}.{:02}", body[0], body[1]));
    }
    None
}

/// Parses a SynScan `m` mount-model response (`[code, b'#']`) into a human-readable model name.
pub fn parse_mount_model(raw: &[u8]) -> Option<String> {
    let body = raw.strip_suffix(b"#").unwrap_or(raw);
    let code = *body.first()?;
    Some(mount_model_name(code))
}

/// Maps SynScan `m` mount-model response byte to a human-readable model name.
pub fn mount_model_name(code: u8) -> String {
    match code {
        0 => "EQ6 / NEQ6-Pro".to_string(),
        1 => "HEQ5".to_string(),
        2 => "EQ5".to_string(),
        3 => "EQ3".to_string(),
        4 => "EQ8".to_string(),
        5 => "AZ-EQ6".to_string(),
        6 => "AZ-EQ5".to_string(),
        128..=143 => "SynScan Alt-Az".to_string(),
        other => format!("SynScan Mount (0x{other:02X})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precise_angle_round_trips_and_rejects_non_finite() {
        for deg in [-15.0, 0.0, 45.0, 90.0, 180.0, 270.5, 359.9999999] {
            let encoded = encode_precise_angle(deg).expect("finite angle");
            assert_eq!(encoded.len(), 8);
            let decoded = decode_precise_angle(&encoded).expect("decode succeeds");
            let diff = ((decoded - deg + 180.0).rem_euclid(360.0) - 180.0).abs();
            assert!(diff < 1e-4, "deg={deg}, decoded={decoded}, diff={diff}");
        }
        // 16-bit decode branch
        assert!((decode_precise_angle("8000").unwrap() - 180.0).abs() < 1e-6);
        assert!(decode_precise_angle("ZZZZ").is_none());
        assert!(decode_precise_angle("123").is_none());
        // Non-finite rejection
        assert!(encode_precise_angle(f64::NAN).is_err());
        assert!(encode_precise_angle(f64::INFINITY).is_err());
    }

    #[test]
    fn precise_radec_handles_negative_dec_and_clamping() {
        let payload = encode_precise_radec_payload(123.456, -23.5).unwrap();
        let (ra, dec) = decode_precise_radec(&format!("{payload}#")).expect("decode radec");
        assert!((ra - 123.456).abs() < 1e-4);
        assert!((dec + 23.5).abs() < 1e-4);

        assert!(encode_precise_radec_payload(f64::NAN, 10.0).is_err());
        assert!(encode_precise_azalt_payload(10.0, f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn firmware_model_and_tracking_reject_malformed_responses() {
        assert_eq!(
            parse_firmware_version(b"042715#").as_deref(),
            Some("4.39.21")
        );
        assert_eq!(
            parse_firmware_version(&[4, 39, b'#']).as_deref(),
            Some("4.39")
        );
        assert_eq!(parse_firmware_version(b"ERR#"), None);
        assert_eq!(parse_firmware_version(b"#"), None);

        assert_eq!(parse_mount_model(&[0, b'#']).as_deref(), Some("EQ6 / NEQ6-Pro"));
        assert_eq!(parse_mount_model(b"#"), None);

        assert_eq!(decode_tracking_mode(&[2, b'#']), Some(TrackingRate::Sidereal));
        assert_eq!(decode_tracking_mode(&[3, b'#']), Some(TrackingRate::SiderealPec));
        assert_eq!(decode_tracking_mode(b"#"), None);
        assert_eq!(decode_tracking_mode(b"ERR#"), None);
    }

    #[test]
    fn set_location_encodes_dms_and_signs_completely() {
        // 48° 51' 24" N, 2° 21' 08" W
        let pkt = encode_set_location(48.8566, -2.3522).unwrap();
        assert_eq!(pkt, [b'W', 48, 51, 24, 0, 2, 21, 8, 1]);
        // South / East with rollover
        let pkt_se = encode_set_location(-33.9999, 151.0).unwrap();
        assert_eq!(pkt_se, [b'W', 34, 0, 0, 1, 151, 0, 0, 0]);
        assert!(encode_set_location(f64::NAN, 0.0).is_err());
    }
}
