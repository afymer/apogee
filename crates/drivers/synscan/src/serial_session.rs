//! SynScan hand-controller serial session state and protocol orchestration.

use std::sync::{Arc, Mutex};

use protocol::{
    MountAxis, MountDriverKind, MountLimits, MountTelemetry, PierSide, SlewDirection, TrackingRate,
};

use crate::coords::{
    current_lst_deg, equatorial_to_horizontal, now_unix_ms, pole_horizontal,
};
use crate::protocol as synscan_proto;
use crate::serial::SerialTransport;
use crate::types::MountError;

/// Poll tracking rate & pier side every N telemetry ticks (~1 Hz at 150ms poll interval)
/// to avoid saturating the 9600-baud hand controller link.
const SLOW_POLL_STRIDE: u32 = 6;

pub(crate) struct SerialSession {
    port_path: String,
    transport: Arc<Mutex<Box<dyn SerialTransport>>>,
    firmware_version: Option<String>,
    mount_model: Option<String>,
    pub slew_rate: u8,
    pub is_parked: bool,
    parking_in_progress: bool,
    pub park_az_deg: f64,
    pub park_alt_deg: f64,
    custom_park: bool,
    target_ra_deg: Option<f64>,
    target_dec_deg: Option<f64>,
    site_lat_deg: f64,
    site_lon_deg: f64,
    pub limits: MountLimits,
    poll_counter: u32,
    slow_poll_dirty: bool,
    cached_tracking: TrackingRate,
    cached_pier_side: PierSide,
}

pub(crate) struct HandshakeInfo {
    pub firmware_version: Option<String>,
    pub mount_model: Option<String>,
}

/// Runs the SynScan `Kx` echo handshake and queries firmware version (`V`) and mount model (`m`).
pub(crate) fn handshake_transport(
    transport: &mut dyn SerialTransport,
) -> Result<HandshakeInfo, MountError> {
    let echo = transport.exchange(b"Kx")?;
    if echo != b"x#" {
        return Err(MountError::Protocol(format!(
            "Unexpected SynScan echo response: {echo:?}"
        )));
    }
    let firmware_version = transport
        .exchange(b"V")
        .ok()
        .and_then(|raw| synscan_proto::parse_firmware_version(&raw));
    let mount_model = transport
        .exchange(b"m")
        .ok()
        .and_then(|raw| synscan_proto::parse_mount_model(&raw));
    Ok(HandshakeInfo {
        firmware_version,
        mount_model,
    })
}

impl SerialSession {
    pub fn new(
        port_path: String,
        transport: Box<dyn SerialTransport>,
        info: HandshakeInfo,
        site_lat_deg: f64,
        site_lon_deg: f64,
        limits: MountLimits,
    ) -> Self {
        let (park_az_deg, park_alt_deg) = pole_horizontal(site_lat_deg);
        Self {
            port_path,
            transport: Arc::new(Mutex::new(transport)),
            firmware_version: info.firmware_version,
            mount_model: info.mount_model,
            slew_rate: 6,
            is_parked: false,
            parking_in_progress: false,
            park_az_deg,
            park_alt_deg,
            custom_park: false,
            target_ra_deg: None,
            target_dec_deg: None,
            site_lat_deg,
            site_lon_deg,
            limits,
            poll_counter: 0,
            slow_poll_dirty: true,
            cached_tracking: TrackingRate::Off,
            cached_pier_side: PierSide::Unknown,
        }
    }

    async fn exchange(&self, cmd: Vec<u8>) -> Result<Vec<u8>, MountError> {
        let transport = self.transport.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = transport
                .lock()
                .map_err(|_| MountError::Io("Serial mutex poisoned".into()))?;
            guard.exchange(&cmd)
        })
        .await
        .map_err(|e| MountError::Io(format!("Serial task join error: {e}")))?
    }

    pub async fn set_site(&mut self, lat_deg: f64, lon_deg: f64) -> Result<(), MountError> {
        let pkt = synscan_proto::encode_set_location(lat_deg, lon_deg)?;
        self.site_lat_deg = lat_deg.clamp(-90.0, 90.0);
        self.site_lon_deg = lon_deg.clamp(-180.0, 180.0);
        if !self.custom_park {
            let (az, alt) = pole_horizontal(self.site_lat_deg);
            self.park_az_deg = az;
            self.park_alt_deg = alt;
        }
        self.exchange(pkt.to_vec()).await?;
        Ok(())
    }

    pub async fn slew_equatorial(&mut self, ra_deg: f64, dec_deg: f64) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        let payload = synscan_proto::encode_precise_radec_payload(ra_deg, dec_deg)?;
        let lst = current_lst_deg(self.site_lon_deg);
        let (az, alt) = equatorial_to_horizontal(ra_deg, dec_deg, self.site_lat_deg, lst);
        self.limits
            .check_horizontal(az, alt)
            .map_err(MountError::LimitViolation)?;

        self.exchange(format!("r{payload}").into_bytes()).await?;
        self.parking_in_progress = false;
        self.target_ra_deg = Some(ra_deg.rem_euclid(360.0));
        self.target_dec_deg = Some(dec_deg.clamp(-90.0, 90.0));
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub async fn slew_horizontal(&mut self, az_deg: f64, alt_deg: f64) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        self.limits
            .check_horizontal(az_deg, alt_deg)
            .map_err(MountError::LimitViolation)?;
        let payload = synscan_proto::encode_precise_azalt_payload(az_deg, alt_deg)?;
        self.exchange(format!("b{payload}").into_bytes()).await?;
        self.parking_in_progress = false;
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub async fn sync_equatorial(&mut self, ra_deg: f64, dec_deg: f64) -> Result<(), MountError> {
        let payload = synscan_proto::encode_precise_radec_payload(ra_deg, dec_deg)?;
        self.exchange(format!("s{payload}").into_bytes()).await?;
        Ok(())
    }

    pub async fn start_manual_slew(
        &mut self,
        direction: SlewDirection,
        rate: u8,
    ) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        self.slew_rate = rate.clamp(1, 9);
        let positive = matches!(direction, SlewDirection::North | SlewDirection::East);
        let pkt = synscan_proto::encode_fixed_rate_slew(direction.axis(), positive, self.slew_rate);
        self.exchange(pkt.to_vec()).await?;
        self.parking_in_progress = false;
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        Ok(())
    }

    pub async fn stop_manual_slew(&mut self, axis: MountAxis) -> Result<(), MountError> {
        let pkt = synscan_proto::encode_fixed_rate_slew(axis, true, 0);
        self.exchange(pkt.to_vec()).await?;
        Ok(())
    }

    pub async fn stop_all(&mut self) -> Result<(), MountError> {
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.parking_in_progress = false;

        let mut first_err: Option<MountError> = None;
        for cmd in [
            b"M".to_vec(),
            synscan_proto::encode_fixed_rate_slew(MountAxis::Ra, true, 0).to_vec(),
            synscan_proto::encode_fixed_rate_slew(MountAxis::Dec, true, 0).to_vec(),
        ] {
            if let Err(err) = self.exchange(cmd).await {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
        if let Some(err) = first_err {
            return Err(err);
        }
        Ok(())
    }

    pub async fn set_tracking(&mut self, rate: TrackingRate) -> Result<(), MountError> {
        if self.is_parked && rate != TrackingRate::Off {
            return Err(MountError::Parked);
        }
        let pkt = synscan_proto::encode_tracking_mode(rate);
        self.exchange(pkt.to_vec()).await?;
        self.cached_tracking = rate;
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub async fn park(&mut self) -> Result<(), MountError> {
        let payload = synscan_proto::encode_precise_azalt_payload(self.park_az_deg, self.park_alt_deg)?;
        self.exchange(format!("b{payload}").into_bytes()).await?;
        self.parking_in_progress = true;
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub fn unpark(&mut self) {
        self.is_parked = false;
        self.parking_in_progress = false;
    }

    pub fn set_park_here(&mut self, current_telemetry: Option<&MountTelemetry>) -> Result<(), MountError> {
        let t = current_telemetry.ok_or_else(|| {
            MountError::Protocol("No telemetry available yet to record park position".into())
        })?;
        self.park_az_deg = t.az_deg;
        self.park_alt_deg = t.alt_deg;
        self.custom_park = true;
        Ok(())
    }

    pub async fn go_home(&mut self) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        let (home_az, home_alt) = pole_horizontal(self.site_lat_deg);
        let payload = synscan_proto::encode_precise_azalt_payload(home_az, home_alt)?;
        self.exchange(format!("b{payload}").into_bytes()).await?;
        self.parking_in_progress = false;
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub async fn meridian_flip(
        &mut self,
        current_telemetry: Option<&MountTelemetry>,
    ) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        let t = current_telemetry.ok_or_else(|| {
            MountError::Protocol("No telemetry available yet for meridian flip".into())
        })?;
        let payload = synscan_proto::encode_precise_radec_payload(t.ra_deg, t.dec_deg)?;
        self.exchange(format!("r{payload}").into_bytes()).await?;
        self.slow_poll_dirty = true;
        Ok(())
    }

    pub async fn poll(&mut self) -> Result<MountTelemetry, MountError> {
        let transport = self.transport.clone();
        let site_lat = self.site_lat_deg;
        let site_lon = self.site_lon_deg;
        let do_slow_poll =
            self.slow_poll_dirty || self.poll_counter.is_multiple_of(SLOW_POLL_STRIDE);
        let prev_tracking = self.cached_tracking;
        let prev_pier_side = self.cached_pier_side;

        let sample = tokio::task::spawn_blocking(move || -> Result<SerialPollSample, MountError> {
            let mut guard = transport
                .lock()
                .map_err(|_| MountError::Io("Serial mutex poisoned".into()))?;
            let eq_raw = guard.exchange(b"e")?;
            let eq_str = String::from_utf8_lossy(&eq_raw);
            let (ra_deg, dec_deg) = synscan_proto::decode_precise_radec(&eq_str)
                .ok_or_else(|| MountError::Protocol(format!("Invalid 'e' response: {eq_str}")))?;

            let lst = current_lst_deg(site_lon);
            let (az_deg, alt_deg) = equatorial_to_horizontal(ra_deg, dec_deg, site_lat, lst);

            let is_slewing = guard
                .exchange(b"L")
                .is_ok_and(|raw| raw.first() == Some(&b'1'));

            let (tracking, pier_side) = if do_slow_poll {
                let tracking = guard
                    .exchange(b"t")
                    .ok()
                    .and_then(|raw| synscan_proto::decode_tracking_mode(&raw))
                    .unwrap_or(prev_tracking);
                let pier_side = guard
                    .exchange(b"p")
                    .ok()
                    .map(|raw| synscan_proto::decode_pier_side(&raw))
                    .unwrap_or(prev_pier_side);
                (tracking, pier_side)
            } else {
                (prev_tracking, prev_pier_side)
            };

            Ok(SerialPollSample {
                ra_deg,
                dec_deg,
                az_deg,
                alt_deg,
                is_slewing,
                tracking,
                pier_side,
            })
        })
        .await
        .map_err(|e| MountError::Io(format!("Serial poll task panicked: {e}")))??;

        self.poll_counter = self.poll_counter.wrapping_add(1);
        self.slow_poll_dirty = false;
        self.cached_tracking = sample.tracking;
        self.cached_pier_side = sample.pier_side;

        if !sample.is_slewing {
            self.target_ra_deg = None;
            self.target_dec_deg = None;
            if self.parking_in_progress {
                // Park slew finished: disable tracking and mark parked.
                let off = synscan_proto::encode_tracking_mode(TrackingRate::Off);
                let _ = self.exchange(off.to_vec()).await;
                self.parking_in_progress = false;
                self.is_parked = true;
                self.cached_tracking = TrackingRate::Off;
            }
        }

        Ok(MountTelemetry {
            ra_deg: sample.ra_deg,
            dec_deg: sample.dec_deg,
            az_deg: sample.az_deg,
            alt_deg: sample.alt_deg,
            pier_side: self.cached_pier_side,
            tracking: self.cached_tracking,
            is_slewing: sample.is_slewing,
            is_parked: self.is_parked,
            slew_rate: self.slew_rate,
            target_ra_deg: self.target_ra_deg,
            target_dec_deg: self.target_dec_deg,
            driver: MountDriverKind::SynScanSerial,
            port: Some(self.port_path.clone()),
            firmware_version: self.firmware_version.clone(),
            mount_model: self.mount_model.clone(),
            limits: self.limits,
            park_az_deg: self.park_az_deg,
            park_alt_deg: self.park_alt_deg,
            updated_at_unix_ms: now_unix_ms(),
        })
    }
}

struct SerialPollSample {
    ra_deg: f64,
    dec_deg: f64,
    az_deg: f64,
    alt_deg: f64,
    is_slewing: bool,
    tracking: TrackingRate,
    pier_side: PierSide,
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct MockTransport {
        responses: VecDeque<Result<Vec<u8>, MountError>>,
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Result<Vec<u8>, MountError>>) -> (Self, Arc<Mutex<Vec<Vec<u8>>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: responses.into(),
                    sent: sent.clone(),
                },
                sent,
            )
        }
    }

    impl SerialTransport for MockTransport {
        fn exchange(&mut self, cmd: &[u8]) -> Result<Vec<u8>, MountError> {
            self.sent.lock().unwrap().push(cmd.to_vec());
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err(MountError::Io("no mock response".into())))
        }
    }

    #[test]
    fn handshake_parses_v4_firmware_and_ignores_empty_model() {
        let (mut mock, _) = MockTransport::new(vec![
            Ok(b"x#".to_vec()),
            Ok(b"042715#".to_vec()),
            Ok(b"#".to_vec()),
        ]);
        let info = handshake_transport(&mut mock).expect("handshake succeeds");
        assert_eq!(info.firmware_version.as_deref(), Some("4.39.21"));
        assert_eq!(info.mount_model, None);
    }

    #[tokio::test]
    async fn stop_all_surfaces_serial_errors() {
        let (mock, sent) = MockTransport::new(vec![
            Err(MountError::Io("disconnected".into())),
            Ok(b"#".to_vec()),
            Ok(b"#".to_vec()),
        ]);
        let mut sess = SerialSession::new(
            "/dev/ttyUSB0".into(),
            Box::new(mock),
            HandshakeInfo {
                firmware_version: None,
                mount_model: None,
            },
            48.85,
            2.35,
            MountLimits::default(),
        );
        assert!(sess.stop_all().await.is_err());
        assert_eq!(sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn park_waits_for_slew_completion_before_marking_parked() {
        let (mock, sent) = MockTransport::new(vec![
            // park `b...`
            Ok(b"#".to_vec()),
            // poll 1: still slewing (`e`, `L`, `t`, `p`)
            Ok(b"00000000,20000000#".to_vec()),
            Ok(b"1#".to_vec()),
            Ok(vec![2, b'#']),
            Ok(b"W#".to_vec()),
            // poll 2: slew finished (`e`, `L`), followed by automatic `T0`
            Ok(b"00000000,20000000#".to_vec()),
            Ok(b"0#".to_vec()),
            Ok(b"#".to_vec()),
        ]);
        let mut sess = SerialSession::new(
            "/dev/ttyUSB0".into(),
            Box::new(mock),
            HandshakeInfo {
                firmware_version: None,
                mount_model: None,
            },
            48.85,
            2.35,
            MountLimits::default(),
        );

        sess.park().await.expect("park starts");
        assert!(!sess.is_parked);

        let t1 = sess.poll().await.expect("poll 1");
        assert!(t1.is_slewing);
        assert!(!t1.is_parked);

        let t2 = sess.poll().await.expect("poll 2");
        assert!(!t2.is_slewing);
        assert!(t2.is_parked);
        assert_eq!(t2.tracking, TrackingRate::Off);
        assert_eq!(sent.lock().unwrap().last().map(Vec::as_slice), Some(&[b'T', 0][..]));
    }
}
