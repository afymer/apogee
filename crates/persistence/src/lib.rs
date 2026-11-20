pub mod backend;
pub mod error;
pub mod observer;
pub mod store;

pub use backend::{DocumentBackend, JsonFileBackend, MemoryBackend, default_config_dir, validate_key};
pub use error::PersistenceError;
pub use observer::{
    CoordinateAxis, GeoLocation, MergeOutcome, OBSERVER_SCHEMA_VERSION, ObservatoryIcon,
    ObservatoryProfile, ObserverCatalog, ProfileSource, TimezoneConfig, TimezoneMode,
    estimate_longitude_from_utc_offset_minutes, estimate_utc_offset_minutes_from_longitude,
    parse_coordinate, parse_elevation_m, parse_utc_offset_minutes,
};
pub use store::{DocumentStore, Persistable};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_test_dir() -> PathBuf {
        let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("apogee_persistence_test_{}_{}", std::process::id(), seq))
    }

    #[test]
    fn json_file_backend_atomic_round_trip() {
        let dir = temp_test_dir();
        let store = DocumentStore::open_dir(dir.clone());

        let mut catalog = ObserverCatalog::default();
        catalog.create_profile(
            "Pic du Midi",
            ObservatoryIcon::Mountain,
            GeoLocation::new(42.9369, 0.1411, 2877.0),
        );
        store.save(&catalog).expect("save succeeds");

        let loaded: ObserverCatalog = store.load().expect("load succeeds");
        assert_eq!(loaded, catalog);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_default_handles_missing_and_corrupt_files() {
        let dir = temp_test_dir();
        let store = DocumentStore::open_dir(dir.clone());

        let missing_loaded: ObserverCatalog = store.load_or_default();
        assert_eq!(missing_loaded, ObserverCatalog::default());

        let backend = JsonFileBackend::new(dir.clone());
        backend
            .write_raw(ObserverCatalog::DOCUMENT_KEY, b"{not valid json")
            .expect("raw write succeeds");

        let corrupt_loaded: ObserverCatalog = store.load_or_default();
        assert_eq!(corrupt_loaded, ObserverCatalog::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reject_future_schema_version() {
        let store = DocumentStore::in_memory();
        let future = ObserverCatalog {
            schema_version: OBSERVER_SCHEMA_VERSION + 10,
            ..ObserverCatalog::default()
        };
        store.save(&future).expect("save succeeds");

        let err = store.load::<ObserverCatalog>().unwrap_err();
        assert!(matches!(
            err,
            PersistenceError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn geolocation_sanitization_and_dms_carry() {
        let dirty = GeoLocation::new(f64::NAN, 220.0, -900.0);
        assert_eq!(dirty.latitude_deg, 0.0);
        assert_eq!(dirty.longitude_deg, 180.0);
        assert_eq!(dirty.elevation_m, GeoLocation::MIN_ELEV_M);

        assert_eq!(GeoLocation::format_dms(48.999999, 'N', 'S'), "49°00'00\" N");
        assert_eq!(GeoLocation::format_dms(-0.0, 'E', 'W'), "0°00'00\" E");
        assert_eq!(GeoLocation::format_dms(-2.3364, 'E', 'W'), "2°20'11\" W");
    }

    #[test]
    fn parse_coordinate_decimal_and_dms_variants() {
        let lat = parse_coordinate("48.8364", CoordinateAxis::Latitude).unwrap();
        assert!((lat - 48.8364).abs() < 1e-9);

        let south = parse_coordinate("S 33.8688", CoordinateAxis::Latitude).unwrap();
        assert!((south + 33.8688).abs() < 1e-9);

        let dms = parse_coordinate("48° 50' 11\" N", CoordinateAxis::Latitude).unwrap();
        assert!((dms - (48.0 + 50.0 / 60.0 + 11.0 / 3600.0)).abs() < 1e-9);

        let compact_dms = parse_coordinate("48d50m11s S", CoordinateAxis::Latitude).unwrap();
        assert!((compact_dms + (48.0 + 50.0 / 60.0 + 11.0 / 3600.0)).abs() < 1e-9);

        let west_lon = parse_coordinate("122° 25' 10\" W", CoordinateAxis::Longitude).unwrap();
        assert!((west_lon + (122.0 + 25.0 / 60.0 + 10.0 / 3600.0)).abs() < 1e-9);

        // Incompatible hemispheres and out-of-range inputs must fail.
        assert!(parse_coordinate("48.0 E", CoordinateAxis::Latitude).is_none());
        assert!(parse_coordinate("2.0 N", CoordinateAxis::Longitude).is_none());
        assert!(parse_coordinate("91.0", CoordinateAxis::Latitude).is_none());
        assert!(parse_coordinate("-181.0", CoordinateAxis::Longitude).is_none());
        assert!(parse_coordinate("48 65 00", CoordinateAxis::Latitude).is_none());
        assert!(parse_coordinate("-48.0 S", CoordinateAxis::Latitude).is_none());
    }

    #[test]
    fn parse_elevation_supports_units_and_bounds() {
        assert!((parse_elevation_m("67").unwrap() - 67.0).abs() < 1e-9);
        assert!((parse_elevation_m("1450 m").unwrap() - 1450.0).abs() < 1e-9);
        assert!((parse_elevation_m("1000 ft").unwrap() - 304.8).abs() < 1e-9);
        assert!((parse_elevation_m("2.5km").unwrap() - 2500.0).abs() < 1e-9);
        assert!((parse_elevation_m("-420").unwrap() + 420.0).abs() < 1e-9);
        assert!(parse_elevation_m("9500").is_none());
        assert!(parse_elevation_m("-600").is_none());
    }

    #[test]
    fn catalog_crud_and_last_profile_guard() {
        let mut catalog = ObserverCatalog::default();
        assert_eq!(catalog.profiles.len(), 2);

        let p1 = catalog.create_profile(
            "Backyard",
            ObservatoryIcon::Rooftop,
            GeoLocation::new(45.0, 5.0, 200.0),
        );
        assert_eq!(p1.id, "backyard-2");
        assert_eq!(catalog.active_profile_id, "backyard-2");

        catalog
            .update_profile("backyard-2", "City Terrace", ObservatoryIcon::Balcony)
            .expect("update succeeds");
        assert_eq!(catalog.active_profile().name, "City Terrace");
        assert_eq!(catalog.active_profile().icon, ObservatoryIcon::Balcony);
        assert_eq!(catalog.active_profile().revision, 2);

        catalog.delete_profile("backyard-2").expect("delete succeeds");
        assert_eq!(catalog.active_profile_id, "backyard");

        catalog
            .delete_profile("nearby-field")
            .expect("second delete succeeds");
        assert!(matches!(
            catalog.delete_profile("backyard"),
            Err(PersistenceError::CannotDeleteLastProfile)
        ));
    }

    #[test]
    fn remote_profile_merge_respects_revisions() {
        let mut catalog = ObserverCatalog::default();
        let remote_new = ObservatoryProfile {
            id: "rpi-dome".to_string(),
            name: "Backyard Pi".to_string(),
            icon: ObservatoryIcon::Remote,
            location: GeoLocation::new(48.8, 2.3, 70.0),
            timezone: TimezoneConfig::default(),
            source: ProfileSource::Remote {
                endpoint: "ws://192.168.1.42:4000/ws".to_string(),
                last_synced_unix_ms: Some(1_000),
            },
            revision: 3,
            updated_at_unix_ms: 1_000,
        };

        assert_eq!(
            catalog.upsert_remote_profile(remote_new.clone()),
            MergeOutcome::Inserted
        );
        assert_eq!(
            catalog.upsert_remote_profile(remote_new.clone()),
            MergeOutcome::Unchanged
        );

        let stale_remote = ObservatoryProfile {
            revision: 2,
            ..remote_new.clone()
        };
        assert_eq!(
            catalog.upsert_remote_profile(stale_remote),
            MergeOutcome::LocalNewer
        );

        let newer_remote = ObservatoryProfile {
            revision: 4,
            location: GeoLocation::new(48.9, 2.4, 75.0),
            ..remote_new
        };
        assert_eq!(
            catalog.upsert_remote_profile(newer_remote),
            MergeOutcome::Updated
        );
        assert!((catalog.profile("rpi-dome").unwrap().location.latitude_deg - 48.9).abs() < 1e-9);
    }

    #[test]
    fn parse_utc_offset_variants_and_timezone_backcompat() {
        assert_eq!(parse_utc_offset_minutes("auto"), Some(None));
        assert_eq!(parse_utc_offset_minutes("site"), Some(None));
        assert_eq!(parse_utc_offset_minutes("local"), Some(None));
        assert_eq!(parse_utc_offset_minutes("UTC"), Some(Some(0)));
        assert_eq!(parse_utc_offset_minutes("+1"), Some(Some(60)));
        assert_eq!(parse_utc_offset_minutes("UTC-5"), Some(Some(-300)));
        assert_eq!(parse_utc_offset_minutes("UTC+5:30"), Some(Some(330)));
        assert_eq!(parse_utc_offset_minutes("-3.5"), Some(Some(-210)));
        assert_eq!(parse_utc_offset_minutes("+14:00"), Some(Some(840)));
        assert_eq!(parse_utc_offset_minutes("-12"), Some(Some(-720)));
        assert!(parse_utc_offset_minutes("+15").is_none());
        assert!(parse_utc_offset_minutes("UTC+1:65").is_none());
        assert!(parse_utc_offset_minutes("bogus").is_none());

        let legacy_json = br#"{
          "schema_version": 1,
          "active_profile_id": "backyard",
          "profiles": [
            {
              "id": "backyard",
              "name": "Backyard",
              "icon": "dome",
              "location": { "latitude_deg": 48.0, "longitude_deg": 2.0, "elevation_m": 60.0 }
            }
          ]
        }"#;
        let parsed: ObserverCatalog = serde_json::from_slice(legacy_json).expect("legacy json loads");
        assert_eq!(parsed.active_profile().timezone, TimezoneConfig::default());
    }

    #[test]
    fn longitude_and_utc_offset_estimation_roundtrip() {
        assert_eq!(estimate_utc_offset_minutes_from_longitude(0.0), 0);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(15.0), 60);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(139.7), 540);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(-74.0), -300);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(-180.0), -720);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(180.0), 720);
        assert_eq!(estimate_utc_offset_minutes_from_longitude(f64::NAN), 0);

        assert!((estimate_longitude_from_utc_offset_minutes(60) - 15.0).abs() < 1e-9);
        assert!((estimate_longitude_from_utc_offset_minutes(-300) + 75.0).abs() < 1e-9);
        assert!((estimate_longitude_from_utc_offset_minutes(540) - 135.0).abs() < 1e-9);
        assert!((estimate_longitude_from_utc_offset_minutes(330) - 82.5).abs() < 1e-9);
        assert!((estimate_longitude_from_utc_offset_minutes(840) - 180.0).abs() < 1e-9);
    }

    #[test]
    fn catalog_default_for_utc_and_load_or_else() {
        let seeded = ObserverCatalog::default_for_utc_offset_minutes(-300);
        assert!((seeded.active_profile().location.longitude_deg + 75.0).abs() < 1e-9);
        assert_eq!(seeded.active_profile().timezone.mode, TimezoneMode::SiteLocal);

        let store = DocumentStore::in_memory();
        let fallback_loaded: ObserverCatalog =
            store.load_or_else(|| ObserverCatalog::default_for_utc_offset_minutes(540));
        assert!((fallback_loaded.active_profile().location.longitude_deg - 135.0).abs() < 1e-9);

        store.save(&seeded).expect("save succeeds");
        let existing_loaded: ObserverCatalog =
            store.load_or_else(|| ObserverCatalog::default_for_utc_offset_minutes(540));
        assert_eq!(existing_loaded, seeded);
    }
}
