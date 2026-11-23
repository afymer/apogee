use serde::{Deserialize, Serialize};

/// Connection status of the telescope mount driver.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MountStatus {
    #[default]
    Disconnected,
    Connecting {
        port: String,
    },
    Ready,
    Error(String),
}

/// Hardware backend used to talk to the mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MountDriverKind {
    /// Sky-Watcher SynScan hand controller serial protocol (9600 8N1) for NEQ6-Pro / EQ mounts.
    #[default]
    SynScanSerial,
    /// Built-in kinematic German Equatorial Mount simulator for offline testing.
    Simulator,
}

impl MountDriverKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::SynScanSerial => "SynScan Hand Controller (Serial)",
            Self::Simulator => "NEQ6-Pro Simulator",
        }
    }
}

/// Physical side of the equatorial pier where the optical tube assembly currently sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PierSide {
    /// OTA is on the East side of the pier (typically looking West of the meridian).
    East,
    /// OTA is on the West side of the pier (typically looking East of the meridian).
    West,
    #[default]
    Unknown,
}

impl PierSide {
    pub fn label(self) -> &'static str {
        match self {
            Self::East => "East",
            Self::West => "West",
            Self::Unknown => "Unknown",
        }
    }
}

/// Sidereal/solar/lunar tracking rate mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TrackingRate {
    #[default]
    Off,
    Sidereal,
    Lunar,
    Solar,
    SiderealPec,
}

impl TrackingRate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Sidereal => "Sidereal",
            Self::Lunar => "Lunar",
            Self::Solar => "Solar",
            Self::SiderealPec => "Sidereal + PEC",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Off,
            Self::Sidereal,
            Self::Lunar,
            Self::Solar,
            Self::SiderealPec,
        ]
    }
}

/// Equatorial mount axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountAxis {
    Ra,
    Dec,
}

/// Cardinal manual-slew direction on the sky.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlewDirection {
    North,
    South,
    East,
    West,
}

impl SlewDirection {
    pub fn axis(self) -> MountAxis {
        match self {
            Self::North | Self::South => MountAxis::Dec,
            Self::East | Self::West => MountAxis::Ra,
        }
    }
}

/// Soft safety & meridian-flip limits enforced by the daemon and visualized in 3D.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MountLimits {
    /// Minimum allowed altitude in degrees `[0, 80]`.
    pub min_altitude_deg: f64,
    /// Maximum allowed altitude in degrees `[1, 90]` (zenith exclusion cone).
    pub max_altitude_deg: f64,
    /// Optional azimuth exclusion sector (e.g. house wall or tree).
    pub az_avoid_enabled: bool,
    /// Start azimuth of the exclusion sector in degrees `[0, 360)`.
    pub az_avoid_start_deg: f64,
    /// End azimuth of the exclusion sector in degrees `[0, 360)` (clockwise from start).
    pub az_avoid_end_deg: f64,
    /// Maximum degrees past the meridian before a flip/stop is required `[0, 45]`.
    pub meridian_limit_deg: f64,
    /// Whether an automatic meridian flip should be triggered when crossing the limit.
    pub auto_meridian_flip: bool,
}

impl Default for MountLimits {
    fn default() -> Self {
        Self {
            min_altitude_deg: 15.0,
            max_altitude_deg: 85.0,
            az_avoid_enabled: false,
            az_avoid_start_deg: 160.0,
            az_avoid_end_deg: 200.0,
            meridian_limit_deg: 10.0,
            auto_meridian_flip: true,
        }
    }
}

impl MountLimits {
    /// Clamps every field into a valid finite range.
    pub fn sanitize(&mut self) {
        if !self.min_altitude_deg.is_finite() {
            self.min_altitude_deg = 15.0;
        }
        if !self.max_altitude_deg.is_finite() {
            self.max_altitude_deg = 85.0;
        }
        self.min_altitude_deg = self.min_altitude_deg.clamp(0.0, 80.0);
        self.max_altitude_deg = self
            .max_altitude_deg
            .clamp((self.min_altitude_deg + 5.0).min(90.0), 90.0);

        if !self.az_avoid_start_deg.is_finite() {
            self.az_avoid_start_deg = 160.0;
        }
        if !self.az_avoid_end_deg.is_finite() {
            self.az_avoid_end_deg = 200.0;
        }
        self.az_avoid_start_deg = self.az_avoid_start_deg.rem_euclid(360.0);
        self.az_avoid_end_deg = self.az_avoid_end_deg.rem_euclid(360.0);

        if !self.meridian_limit_deg.is_finite() {
            self.meridian_limit_deg = 10.0;
        }
        self.meridian_limit_deg = self.meridian_limit_deg.clamp(0.0, 45.0);
    }

    /// Returns `true` if `az_deg` falls inside the configured clockwise azimuth exclusion sector.
    pub fn is_azimuth_excluded(&self, az_deg: f64) -> bool {
        if !self.az_avoid_enabled {
            return false;
        }
        let az = az_deg.rem_euclid(360.0);
        let start = self.az_avoid_start_deg.rem_euclid(360.0);
        let end = self.az_avoid_end_deg.rem_euclid(360.0);
        if (end - start).abs() < 1e-6 {
            return false;
        }
        if start < end {
            az >= start && az <= end
        } else {
            az >= start || az <= end
        }
    }

    /// Checks whether a horizontal `(az_deg, alt_deg)` pointing violates altitude or azimuth limits.
    pub fn check_horizontal(&self, az_deg: f64, alt_deg: f64) -> Result<(), String> {
        if !az_deg.is_finite() || !alt_deg.is_finite() {
            return Err("Target coordinates must be finite numbers".to_string());
        }
        if alt_deg < self.min_altitude_deg {
            return Err(format!(
                "Target altitude ({alt_deg:.1}°) is below minimum limit ({:.1}°)",
                self.min_altitude_deg
            ));
        }
        if alt_deg > self.max_altitude_deg {
            return Err(format!(
                "Target altitude ({alt_deg:.1}°) exceeds zenith limit ({:.1}°)",
                self.max_altitude_deg
            ));
        }
        if self.is_azimuth_excluded(az_deg) {
            return Err(format!(
                "Target azimuth ({az_deg:.1}°) is inside exclusion sector ({:.0}°–{:.0}°)",
                self.az_avoid_start_deg, self.az_avoid_end_deg
            ));
        }
        Ok(())
    }
}

/// Snapshot of mount pointing & kinematics broadcast by the daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MountTelemetry {
    /// Right Ascension in degrees `[0, 360)`.
    pub ra_deg: f64,
    /// Declination in degrees `[-90, 90]`.
    pub dec_deg: f64,
    /// Topocentric Azimuth in degrees `[0, 360)` (0 = N, 90 = E, 180 = S, 270 = W).
    pub az_deg: f64,
    /// Topocentric Altitude in degrees `[-90, 90]`.
    pub alt_deg: f64,
    /// Current physical pier side.
    pub pier_side: PierSide,
    /// Active tracking rate.
    pub tracking: TrackingRate,
    /// Whether the mount is currently slewing (GoTo or manual).
    pub is_slewing: bool,
    /// Whether the mount is parked.
    pub is_parked: bool,
    /// Current SynScan hand-controller slew speed index `1..=9`.
    pub slew_rate: u8,
    /// Active GoTo target RA/Dec in degrees, if slewing to a target.
    pub target_ra_deg: Option<f64>,
    pub target_dec_deg: Option<f64>,
    /// Active driver backend.
    pub driver: MountDriverKind,
    /// Serial port path (or `"simulator"`).
    pub port: Option<String>,
    /// Reported hand-controller firmware string (e.g. `"4.39.21"`).
    pub firmware_version: Option<String>,
    /// Reported mount model name (e.g. `"NEQ6-Pro / EQ6"`).
    pub mount_model: Option<String>,
    /// Active safety & meridian limits.
    pub limits: MountLimits,
    /// Configured park position in horizontal degrees.
    pub park_az_deg: f64,
    pub park_alt_deg: f64,
    /// Timestamp when this telemetry sample was captured (UTC unix milliseconds).
    pub updated_at_unix_ms: u64,
}

impl Default for MountTelemetry {
    fn default() -> Self {
        Self {
            ra_deg: 0.0,
            dec_deg: 90.0,
            az_deg: 0.0,
            alt_deg: 48.8,
            pier_side: PierSide::West,
            tracking: TrackingRate::Off,
            is_slewing: false,
            is_parked: true,
            slew_rate: 6,
            target_ra_deg: None,
            target_dec_deg: None,
            driver: MountDriverKind::SynScanSerial,
            port: None,
            firmware_version: None,
            mount_model: None,
            limits: MountLimits::default(),
            park_az_deg: 0.0,
            park_alt_deg: 48.8,
            updated_at_unix_ms: 0,
        }
    }
}

/// Minimal persisted pointing snapshot retained when the mount is disconnected.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MountPointing {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub pier_side: PierSide,
    pub updated_at_unix_ms: u64,
}

impl From<&MountTelemetry> for MountPointing {
    fn from(t: &MountTelemetry) -> Self {
        Self {
            ra_deg: t.ra_deg,
            dec_deg: t.dec_deg,
            az_deg: t.az_deg,
            alt_deg: t.alt_deg,
            pier_side: t.pier_side,
            updated_at_unix_ms: t.updated_at_unix_ms,
        }
    }
}

/// Commands sent from the viewer UI to the daemon's mount controller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum MountCommand {
    /// Discover available serial ports on the daemon host (e.g. `/dev/ttyUSB0`).
    ListPorts,
    /// Connect to the mount using the selected driver and serial parameters.
    Connect {
        driver: MountDriverKind,
        port: String,
        baud_rate: u32,
    },
    /// Disconnect cleanly from the mount.
    Disconnect,
    /// GoTo equatorial J2000/of-date coordinates in degrees.
    SlewToEquatorial { ra_deg: f64, dec_deg: f64 },
    /// GoTo topocentric horizontal coordinates in degrees.
    SlewToHorizontal { az_deg: f64, alt_deg: f64 },
    /// Sync the mount's sky model to the given equatorial coordinates.
    SyncEquatorial { ra_deg: f64, dec_deg: f64 },
    /// Start continuous manual slewing in a cardinal direction at SynScan rate `1..=9`.
    StartManualSlew { direction: SlewDirection, rate: u8 },
    /// Stop manual slewing on a single axis.
    StopManualSlew { axis: MountAxis },
    /// Emergency stop: abort any GoTo and halt both axes immediately.
    StopAll,
    /// Change tracking rate.
    SetTracking(TrackingRate),
    /// Change default hand-controller manual slew rate `1..=9`.
    SetSlewRate(u8),
    /// Slew to the configured park position and disable tracking.
    Park,
    /// Unpark the mount so slewing and tracking are allowed.
    Unpark,
    /// Store the current horizontal position as the new park position.
    SetParkHere,
    /// Slew to the counterweight-down celestial pole home position.
    GoHome,
    /// Perform a meridian flip to the opposite pier side at the current target.
    MeridianFlip,
    /// Update safety and meridian-flip limits.
    SetLimits(MountLimits),
    /// Push observer site coordinates to the mount / simulator.
    SetSite {
        latitude_deg: f64,
        longitude_deg: f64,
        elevation_m: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_limits_sanitize_clamps_and_recovers_non_finite() {
        let mut limits = MountLimits {
            min_altitude_deg: f64::NAN,
            max_altitude_deg: f64::INFINITY,
            az_avoid_enabled: true,
            az_avoid_start_deg: -30.0,
            az_avoid_end_deg: 400.0,
            meridian_limit_deg: 90.0,
            auto_meridian_flip: true,
        };
        limits.sanitize();
        assert_eq!(limits.min_altitude_deg, 15.0);
        assert_eq!(limits.max_altitude_deg, 85.0);
        assert!((limits.az_avoid_start_deg - 330.0).abs() < 1e-9);
        assert!((limits.az_avoid_end_deg - 40.0).abs() < 1e-9);
        assert_eq!(limits.meridian_limit_deg, 45.0);

        let mut inverted = MountLimits {
            min_altitude_deg: 88.0,
            max_altitude_deg: 20.0,
            ..MountLimits::default()
        };
        inverted.sanitize();
        assert_eq!(inverted.min_altitude_deg, 80.0);
        assert_eq!(inverted.max_altitude_deg, 85.0);
    }

    #[test]
    fn mount_limits_azimuth_exclusion_handles_wrap_and_nan() {
        let limits = MountLimits {
            az_avoid_enabled: true,
            az_avoid_start_deg: 330.0,
            az_avoid_end_deg: 30.0,
            ..MountLimits::default()
        };
        assert!(limits.is_azimuth_excluded(350.0));
        assert!(limits.is_azimuth_excluded(0.0));
        assert!(limits.is_azimuth_excluded(15.0));
        assert!(!limits.is_azimuth_excluded(180.0));

        assert!(limits.check_horizontal(180.0, 45.0).is_ok());
        assert!(limits.check_horizontal(f64::NAN, 45.0).is_err());
        assert!(limits.check_horizontal(180.0, f64::NAN).is_err());
        assert!(limits.check_horizontal(180.0, f64::INFINITY).is_err());
        assert!(limits.check_horizontal(10.0, 45.0).is_err());
        assert!(limits.check_horizontal(180.0, 5.0).is_err());
        assert!(limits.check_horizontal(180.0, 89.0).is_err());
    }

    #[test]
    fn mount_command_json_round_trip() {
        let cmd = MountCommand::Connect {
            driver: MountDriverKind::SynScanSerial,
            port: "/dev/ttyUSB0".to_string(),
            baud_rate: 9600,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let decoded: MountCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, cmd);
    }
}
