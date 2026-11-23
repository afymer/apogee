use protocol::MountPointing;
use serde::{Deserialize, Serialize};

use crate::store::Persistable;

pub const MOUNT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    MOUNT_SCHEMA_VERSION
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_serial_port() -> String {
    "/dev/ttyUSB0".to_string()
}

/// Persisted hardware & mount preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HardwareConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub daemon_url: Option<String>,
    #[serde(default)]
    pub use_simulator: bool,
    #[serde(default = "default_serial_port")]
    pub mount_port: String,
    #[serde(default = "default_baud_rate")]
    pub mount_baud_rate: u32,
    #[serde(default)]
    pub last_pointing: Option<MountPointing>,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            schema_version: MOUNT_SCHEMA_VERSION,
            daemon_url: None,
            use_simulator: false,
            mount_port: default_serial_port(),
            mount_baud_rate: default_baud_rate(),
            last_pointing: None,
        }
    }
}

impl Persistable for HardwareConfig {
    const DOCUMENT_KEY: &'static str = "hardware";
    const SCHEMA_VERSION: u32 = MOUNT_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn normalize_after_load(&mut self) {
        self.schema_version = MOUNT_SCHEMA_VERSION;
        if let Some(url) = &mut self.daemon_url {
            let trimmed = url.trim();
            if trimmed.is_empty() {
                self.daemon_url = None;
            } else if trimmed.len() != url.len() {
                *url = trimmed.to_string();
            }
        }
        if self.mount_baud_rate == 0 {
            self.mount_baud_rate = default_baud_rate();
        }
        if self.mount_port.trim().is_empty() {
            self.mount_port = default_serial_port();
        }
        if let Some(pointing) = &mut self.last_pointing {
            if !pointing.ra_deg.is_finite()
                || !pointing.dec_deg.is_finite()
                || !pointing.az_deg.is_finite()
                || !pointing.alt_deg.is_finite()
            {
                self.last_pointing = None;
            } else {
                pointing.ra_deg = pointing.ra_deg.rem_euclid(360.0);
                pointing.dec_deg = pointing.dec_deg.clamp(-90.0, 90.0);
                pointing.az_deg = pointing.az_deg.rem_euclid(360.0);
                pointing.alt_deg = pointing.alt_deg.clamp(-90.0, 90.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::PierSide;

    use super::*;

    #[test]
    fn normalize_clears_empty_url_and_invalid_pointing() {
        let mut cfg = HardwareConfig {
            schema_version: 99,
            daemon_url: Some("   ".to_string()),
            use_simulator: true,
            mount_port: " ".to_string(),
            mount_baud_rate: 0,
            last_pointing: Some(MountPointing {
                ra_deg: f64::NAN,
                dec_deg: 45.0,
                az_deg: 180.0,
                alt_deg: 30.0,
                pier_side: PierSide::East,
                updated_at_unix_ms: 1000,
            }),
        };
        cfg.normalize_after_load();
        assert_eq!(cfg.schema_version, MOUNT_SCHEMA_VERSION);
        assert_eq!(cfg.daemon_url, None);
        assert_eq!(cfg.mount_port, "/dev/ttyUSB0");
        assert_eq!(cfg.mount_baud_rate, 9600);
        assert_eq!(cfg.last_pointing, None);
    }

    #[test]
    fn normalize_wraps_and_clamps_valid_pointing() {
        let mut cfg = HardwareConfig {
            daemon_url: Some("  ws://localhost:8080/ws  ".to_string()),
            last_pointing: Some(MountPointing {
                ra_deg: -10.0,
                dec_deg: 120.0,
                az_deg: 370.0,
                alt_deg: -95.0,
                pier_side: PierSide::West,
                updated_at_unix_ms: 42,
            }),
            ..HardwareConfig::default()
        };
        cfg.normalize_after_load();
        assert_eq!(cfg.daemon_url.as_deref(), Some("ws://localhost:8080/ws"));
        let p = cfg.last_pointing.expect("valid pointing kept");
        assert!((p.ra_deg - 350.0).abs() < 1e-9);
        assert_eq!(p.dec_deg, 90.0);
        assert!((p.az_deg - 10.0).abs() < 1e-9);
        assert_eq!(p.alt_deg, -90.0);
        assert_eq!(p.pier_side, PierSide::West);
    }
}
