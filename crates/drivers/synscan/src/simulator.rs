//! Kinematic NEQ6-Pro German Equatorial Mount simulator.

use protocol::{
    MountAxis, MountDriverKind, MountLimits, MountTelemetry, PierSide, SlewDirection, TrackingRate,
};

use crate::coords::{
    DEFAULT_SITE_LAT_DEG, DEFAULT_SITE_LON_DEG, current_lst_deg, equatorial_to_horizontal,
    horizontal_to_equatorial, now_unix_ms, pole_horizontal, wrap_signed_180,
};
use crate::types::MountError;

/// Sidereal rate in degrees per second (`360° / 86164.0905s`).
const SIDEREAL_DEG_PER_SEC: f64 = 360.0 / 86_164.090_5;

/// SynScan hand-controller manual slew speeds (1..=9) in degrees per second.
pub fn slew_rate_deg_per_sec(rate: u8) -> f64 {
    match rate.clamp(1, 9) {
        1 => SIDEREAL_DEG_PER_SEC * 0.5,
        2 => SIDEREAL_DEG_PER_SEC * 1.0,
        3 => SIDEREAL_DEG_PER_SEC * 8.0,
        4 => SIDEREAL_DEG_PER_SEC * 16.0,
        5 => SIDEREAL_DEG_PER_SEC * 32.0,
        6 => SIDEREAL_DEG_PER_SEC * 64.0,
        7 => SIDEREAL_DEG_PER_SEC * 128.0,
        8 => 1.5,
        _ => 3.4, // NEQ6-Pro max slew ~3.4°/s (800x sidereal)
    }
}

/// GoTo slew speed (degrees/sec) scaled by the user's slew-rate slider `1..=9` in simulation.
pub fn goto_rate_deg_per_sec(rate: u8) -> f64 {
    match rate.clamp(1, 9) {
        1 => 0.35,
        2 => 0.75,
        3 => 1.4,
        4 => 2.4,
        5 => 3.8,
        6 => 5.5,
        7 => 8.0,
        8 => 11.5,
        _ => 16.0,
    }
}

pub struct SimulatedMount {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub pier_side: PierSide,
    pub tracking: TrackingRate,
    pub is_parked: bool,
    pub slew_rate: u8,
    pub target_ra_deg: Option<f64>,
    pub target_dec_deg: Option<f64>,
    pub target_pier_side: Option<PierSide>,
    pub parking_in_progress: bool,
    manual_flip_override: bool,
    pub manual_ra_vel_deg_s: f64,
    pub manual_dec_vel_deg_s: f64,
    pub site_lat_deg: f64,
    pub site_lon_deg: f64,
    pub park_az_deg: f64,
    pub park_alt_deg: f64,
    custom_park: bool,
    pub limits: MountLimits,
}

impl Default for SimulatedMount {
    fn default() -> Self {
        let lat = DEFAULT_SITE_LAT_DEG;
        let lon = DEFAULT_SITE_LON_DEG;
        let lst = current_lst_deg(lon);
        let (park_az, park_alt) = pole_horizontal(lat);
        let (ra, dec) = horizontal_to_equatorial(park_az, park_alt, lat, lst);
        Self {
            ra_deg: ra,
            dec_deg: dec,
            pier_side: PierSide::West,
            tracking: TrackingRate::Off,
            is_parked: true,
            slew_rate: 7,
            target_ra_deg: None,
            target_dec_deg: None,
            target_pier_side: None,
            parking_in_progress: false,
            manual_flip_override: false,
            manual_ra_vel_deg_s: 0.0,
            manual_dec_vel_deg_s: 0.0,
            site_lat_deg: lat,
            site_lon_deg: lon,
            park_az_deg: park_az,
            park_alt_deg: park_alt,
            custom_park: false,
            limits: MountLimits::default(),
        }
    }
}

impl SimulatedMount {
    pub fn set_site(&mut self, lat_deg: f64, lon_deg: f64) {
        if !lat_deg.is_finite() || !lon_deg.is_finite() {
            return;
        }
        self.site_lat_deg = lat_deg.clamp(-90.0, 90.0);
        self.site_lon_deg = lon_deg.clamp(-180.0, 180.0);
        if !self.custom_park {
            let (az, alt) = pole_horizontal(self.site_lat_deg);
            self.park_az_deg = az;
            self.park_alt_deg = alt;
        }
        if self.is_parked {
            let lst = current_lst_deg(self.site_lon_deg);
            let (ra, dec) = horizontal_to_equatorial(
                self.park_az_deg,
                self.park_alt_deg,
                self.site_lat_deg,
                lst,
            );
            self.ra_deg = ra;
            self.dec_deg = dec;
        }
    }

    pub fn is_slewing(&self) -> bool {
        self.target_ra_deg.is_some()
            || self.parking_in_progress
            || self.manual_ra_vel_deg_s.abs() > 1e-9
            || self.manual_dec_vel_deg_s.abs() > 1e-9
    }

    pub fn goto_equatorial(&mut self, ra_deg: f64, dec_deg: f64) -> Result<(), MountError> {
        let lst = current_lst_deg(self.site_lon_deg);
        self.goto_equatorial_at(ra_deg, dec_deg, lst)
    }

    pub fn goto_equatorial_at(
        &mut self,
        ra_deg: f64,
        dec_deg: f64,
        lst_deg: f64,
    ) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        if !ra_deg.is_finite() || !dec_deg.is_finite() {
            return Err(MountError::Protocol(
                "Target coordinates must be finite".into(),
            ));
        }
        let ra = ra_deg.rem_euclid(360.0);
        let dec = dec_deg.clamp(-90.0, 90.0);
        let (az, alt) = equatorial_to_horizontal(ra, dec, self.site_lat_deg, lst_deg);
        self.limits
            .check_horizontal(az, alt)
            .map_err(MountError::LimitViolation)?;

        self.begin_goto(ra, dec, lst_deg);
        Ok(())
    }

    pub fn goto_horizontal(&mut self, az_deg: f64, alt_deg: f64) -> Result<(), MountError> {
        let lst = current_lst_deg(self.site_lon_deg);
        self.goto_horizontal_at(az_deg, alt_deg, lst)
    }

    pub fn goto_horizontal_at(
        &mut self,
        az_deg: f64,
        alt_deg: f64,
        lst_deg: f64,
    ) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        self.limits
            .check_horizontal(az_deg, alt_deg)
            .map_err(MountError::LimitViolation)?;
        let (ra, dec) = horizontal_to_equatorial(az_deg, alt_deg, self.site_lat_deg, lst_deg);
        self.begin_goto(ra, dec, lst_deg);
        Ok(())
    }

    fn begin_goto(&mut self, ra: f64, dec: f64, lst_deg: f64) {
        let ha = wrap_signed_180(lst_deg - ra);
        // Standard GEM convention: targets East of meridian (HA < 0) -> PierSide::West;
        // targets West of meridian (HA >= 0) -> PierSide::East.
        self.target_pier_side = Some(if ha >= 0.0 {
            PierSide::East
        } else {
            PierSide::West
        });
        self.target_ra_deg = Some(ra);
        self.target_dec_deg = Some(dec);
        self.parking_in_progress = false;
        self.manual_flip_override = false;
        self.manual_ra_vel_deg_s = 0.0;
        self.manual_dec_vel_deg_s = 0.0;
        if self.tracking == TrackingRate::Off {
            self.tracking = TrackingRate::Sidereal;
        }
    }

    pub fn sync_equatorial(&mut self, ra_deg: f64, dec_deg: f64) -> Result<(), MountError> {
        if !ra_deg.is_finite() || !dec_deg.is_finite() {
            return Err(MountError::Protocol(
                "Sync coordinates must be finite".into(),
            ));
        }
        self.ra_deg = ra_deg.rem_euclid(360.0);
        self.dec_deg = dec_deg.clamp(-90.0, 90.0);
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.target_pier_side = None;
        Ok(())
    }

    pub fn start_manual_slew(
        &mut self,
        direction: SlewDirection,
        rate: u8,
    ) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        self.slew_rate = rate.clamp(1, 9);
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.target_pier_side = None;
        self.parking_in_progress = false;

        let speed = slew_rate_deg_per_sec(self.slew_rate);
        match direction {
            SlewDirection::North => self.manual_dec_vel_deg_s = speed,
            SlewDirection::South => self.manual_dec_vel_deg_s = -speed,
            SlewDirection::East => self.manual_ra_vel_deg_s = speed,
            SlewDirection::West => self.manual_ra_vel_deg_s = -speed,
        }
        Ok(())
    }

    pub fn stop_manual_slew(&mut self, axis: MountAxis) {
        match axis {
            MountAxis::Ra => self.manual_ra_vel_deg_s = 0.0,
            MountAxis::Dec => self.manual_dec_vel_deg_s = 0.0,
        }
    }

    pub fn stop_all(&mut self) {
        self.target_ra_deg = None;
        self.target_dec_deg = None;
        self.target_pier_side = None;
        self.parking_in_progress = false;
        self.manual_ra_vel_deg_s = 0.0;
        self.manual_dec_vel_deg_s = 0.0;
    }

    pub fn set_tracking(&mut self, rate: TrackingRate) -> Result<(), MountError> {
        if self.is_parked && rate != TrackingRate::Off {
            return Err(MountError::Parked);
        }
        self.tracking = rate;
        Ok(())
    }

    pub fn park(&mut self) {
        let lst = current_lst_deg(self.site_lon_deg);
        self.park_at(lst);
    }

    pub fn park_at(&mut self, lst_deg: f64) {
        let (ra, dec) = horizontal_to_equatorial(
            self.park_az_deg,
            self.park_alt_deg,
            self.site_lat_deg,
            lst_deg,
        );
        self.target_ra_deg = Some(ra);
        self.target_dec_deg = Some(dec);
        self.target_pier_side = Some(PierSide::West);
        self.parking_in_progress = true;
        self.manual_flip_override = false;
        self.manual_ra_vel_deg_s = 0.0;
        self.manual_dec_vel_deg_s = 0.0;
        self.tracking = TrackingRate::Off;
    }

    pub fn unpark(&mut self) {
        self.is_parked = false;
        self.parking_in_progress = false;
    }

    pub fn set_park_here(&mut self) {
        let lst = current_lst_deg(self.site_lon_deg);
        let (az, alt) = equatorial_to_horizontal(self.ra_deg, self.dec_deg, self.site_lat_deg, lst);
        self.park_az_deg = az;
        self.park_alt_deg = alt;
        self.custom_park = true;
    }

    pub fn go_home(&mut self) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        let (home_az, home_alt) = pole_horizontal(self.site_lat_deg);
        let lst = current_lst_deg(self.site_lon_deg);
        let (ra, dec) = horizontal_to_equatorial(home_az, home_alt, self.site_lat_deg, lst);
        self.target_ra_deg = Some(ra);
        self.target_dec_deg = Some(dec);
        self.target_pier_side = Some(PierSide::West);
        self.parking_in_progress = false;
        self.manual_flip_override = false;
        self.manual_ra_vel_deg_s = 0.0;
        self.manual_dec_vel_deg_s = 0.0;
        Ok(())
    }

    pub fn meridian_flip(&mut self) -> Result<(), MountError> {
        if self.is_parked {
            return Err(MountError::Parked);
        }
        let next_side = match self.pier_side {
            PierSide::East => PierSide::West,
            PierSide::West | PierSide::Unknown => PierSide::East,
        };
        self.stop_all();
        self.pier_side = next_side;
        self.manual_flip_override = true;
        Ok(())
    }

    pub fn step(&mut self, dt_secs: f64) {
        let lst = current_lst_deg(self.site_lon_deg);
        self.step_at(dt_secs, lst);
    }

    pub fn step_at(&mut self, dt_secs: f64, lst_deg: f64) {
        let dt = dt_secs.clamp(0.0, 1.0);
        if dt <= 0.0 {
            return;
        }

        // If parked and stationary, the mount is fixed in horizontal coordinates, so RA drifts with LST.
        if self.is_parked && !self.is_slewing() {
            let (ra, dec) = horizontal_to_equatorial(
                self.park_az_deg,
                self.park_alt_deg,
                self.site_lat_deg,
                lst_deg,
            );
            self.ra_deg = ra;
            self.dec_deg = dec;
            return;
        }

        // Keep horizontal park target fresh during park slew so arrival doesn't jump.
        if self.parking_in_progress {
            let (ra, dec) = horizontal_to_equatorial(
                self.park_az_deg,
                self.park_alt_deg,
                self.site_lat_deg,
                lst_deg,
            );
            self.target_ra_deg = Some(ra);
            self.target_dec_deg = Some(dec);
        }

        // GoTo slew interpolation — scaled by the selected slew rate
        if let (Some(tgt_ra), Some(tgt_dec)) = (self.target_ra_deg, self.target_dec_deg) {
            let max_step = goto_rate_deg_per_sec(self.slew_rate) * dt;
            let dra = wrap_signed_180(tgt_ra - self.ra_deg);
            let ddec = tgt_dec - self.dec_deg;
            let dist = dra.hypot(ddec);
            if dist <= max_step {
                self.ra_deg = tgt_ra;
                self.dec_deg = tgt_dec;
                if let Some(side) = self.target_pier_side.take() {
                    self.pier_side = side;
                }
                self.target_ra_deg = None;
                self.target_dec_deg = None;
                if self.parking_in_progress {
                    self.parking_in_progress = false;
                    self.is_parked = true;
                    self.tracking = TrackingRate::Off;
                }
            } else {
                let frac = max_step / dist;
                self.ra_deg = (self.ra_deg + dra * frac).rem_euclid(360.0);
                self.dec_deg = (self.dec_deg + ddec * frac).clamp(-90.0, 90.0);
            }
            return;
        }

        // Manual slewing (respect safety limits if moving into a forbidden zone)
        if self.manual_ra_vel_deg_s.abs() > 1e-9 || self.manual_dec_vel_deg_s.abs() > 1e-9 {
            let next_ra = (self.ra_deg + self.manual_ra_vel_deg_s * dt).rem_euclid(360.0);
            let next_dec = (self.dec_deg + self.manual_dec_vel_deg_s * dt).clamp(-90.0, 90.0);
            let (cur_az, cur_alt) =
                equatorial_to_horizontal(self.ra_deg, self.dec_deg, self.site_lat_deg, lst_deg);
            let (next_az, next_alt) =
                equatorial_to_horizontal(next_ra, next_dec, self.site_lat_deg, lst_deg);
            // Allow moving out of a limit zone if already inside one, otherwise block entering it.
            let already_outside = self.limits.check_horizontal(cur_az, cur_alt).is_err();
            if already_outside || self.limits.check_horizontal(next_az, next_alt).is_ok() {
                self.ra_deg = next_ra;
                self.dec_deg = next_dec;
            } else {
                self.manual_ra_vel_deg_s = 0.0;
                self.manual_dec_vel_deg_s = 0.0;
            }
            return;
        }

        // Tracking / non-tracking drift
        match self.tracking {
            TrackingRate::Off => {
                self.ra_deg = (self.ra_deg + SIDEREAL_DEG_PER_SEC * dt).rem_euclid(360.0);
            }
            TrackingRate::Sidereal | TrackingRate::SiderealPec => {
                let ha = wrap_signed_180(lst_deg - self.ra_deg);
                if ha <= 0.0 {
                    self.manual_flip_override = false;
                }
                // Auto meridian flip when tracking West of meridian past configured limit
                if self.limits.auto_meridian_flip
                    && !self.manual_flip_override
                    && self.pier_side == PierSide::West
                    && ha > self.limits.meridian_limit_deg
                {
                    self.pier_side = PierSide::East;
                }
            }
            TrackingRate::Lunar => {
                // Moon drifts ~0.549°/hr East in RA relative to stars
                self.ra_deg = (self.ra_deg + (0.549 / 3600.0) * dt).rem_euclid(360.0);
            }
            TrackingRate::Solar => {
                // Sun drifts ~0.041°/hr East in RA relative to stars
                self.ra_deg = (self.ra_deg + (0.041 / 3600.0) * dt).rem_euclid(360.0);
            }
        }
    }

    pub fn telemetry(&self) -> MountTelemetry {
        let lst = current_lst_deg(self.site_lon_deg);
        self.telemetry_at(lst)
    }

    pub fn telemetry_at(&self, lst_deg: f64) -> MountTelemetry {
        let (az_deg, alt_deg) =
            equatorial_to_horizontal(self.ra_deg, self.dec_deg, self.site_lat_deg, lst_deg);
        MountTelemetry {
            ra_deg: self.ra_deg,
            dec_deg: self.dec_deg,
            az_deg,
            alt_deg,
            pier_side: self.pier_side,
            tracking: self.tracking,
            is_slewing: self.is_slewing(),
            is_parked: self.is_parked,
            slew_rate: self.slew_rate,
            target_ra_deg: self.target_ra_deg,
            target_dec_deg: self.target_dec_deg,
            driver: MountDriverKind::Simulator,
            port: Some("simulator".to_string()),
            firmware_version: Some("4.39.21 (Sim)".to_string()),
            mount_model: Some("NEQ6-Pro (Simulated)".to_string()),
            limits: self.limits,
            park_az_deg: self.park_az_deg,
            park_alt_deg: self.park_alt_deg,
            updated_at_unix_ms: now_unix_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_unpark_goto_pier_side_and_park_deterministic() {
        let lst = 140.0;
        let mut sim = SimulatedMount::default();
        assert!(sim.is_parked);
        assert!(matches!(
            sim.goto_equatorial_at(100.0, 45.0, lst),
            Err(MountError::Parked)
        ));
        assert!(matches!(
            sim.set_tracking(TrackingRate::Sidereal),
            Err(MountError::Parked)
        ));

        sim.unpark();
        // Target slightly West of meridian (HA = +5°, inside 10° meridian limit) -> PierSide::East
        sim.goto_equatorial_at(lst - 5.0, 40.0, lst)
            .expect("west target allowed");
        assert!(sim.is_slewing());
        for _ in 0..100 {
            sim.step_at(0.2, lst);
        }
        assert!(!sim.is_slewing());
        assert_eq!(sim.pier_side, PierSide::East);
        assert!(wrap_signed_180(sim.ra_deg - (lst - 5.0)).abs() < 1e-4);

        // Manual meridian flip swaps pier side immediately and isn't reverted by auto-flip
        sim.meridian_flip().expect("meridian flip");
        assert_eq!(sim.pier_side, PierSide::West);
        sim.step_at(0.2, lst + 20.0);
        assert_eq!(sim.pier_side, PierSide::West);

        // Tracking from East of meridian across the meridian limit triggers auto meridian flip
        sim.goto_equatorial_at(lst + 10.0, 40.0, lst)
            .expect("east target allowed");
        for _ in 0..100 {
            sim.step_at(0.2, lst);
        }
        assert_eq!(sim.pier_side, PierSide::West);
        sim.step_at(0.2, lst + 25.0);
        assert_eq!(sim.pier_side, PierSide::East);

        // Park slew finishes parked with tracking off
        sim.park_at(lst);
        for _ in 0..100 {
            sim.step_at(0.2, lst);
        }
        assert!(sim.is_parked);
        assert_eq!(sim.tracking, TrackingRate::Off);
    }

    #[test]
    fn simulator_enforces_limits_and_rejects_nan() {
        let lst = 140.0;
        let mut sim = SimulatedMount::default();
        sim.unpark();
        // Below min altitude (15°)
        assert!(matches!(
            sim.goto_horizontal_at(180.0, 5.0, lst),
            Err(MountError::LimitViolation(_))
        ));
        // Boundary altitude succeeds without round-trip failure
        sim.goto_horizontal_at(180.0, sim.limits.min_altitude_deg, lst)
            .expect("boundary altitude allowed");
        // NaN rejected
        assert!(sim.goto_equatorial_at(f64::NAN, 30.0, lst).is_err());
        assert!(sim.sync_equatorial(f64::NAN, 30.0).is_err());
    }

    #[test]
    fn simulator_set_site_updates_default_park_even_when_unparked() {
        let mut sim = SimulatedMount::default();
        sim.unpark();
        sim.set_site(-33.8688, 151.2093);
        assert_eq!(sim.park_az_deg, 180.0);
        assert!((sim.park_alt_deg - 33.8688).abs() < 1e-4);
    }
}
