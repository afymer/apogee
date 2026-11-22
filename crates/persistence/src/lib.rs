pub mod backend;
pub mod error;
pub mod horizon;
pub mod observer;
pub mod settings;
pub mod store;
pub mod tz_offset;

pub use backend::{
    DocumentBackend, JsonFileBackend, MemoryBackend, default_config_dir, validate_asset_name,
    validate_key,
};
pub use error::PersistenceError;
pub use horizon::{
    DEFAULT_HORIZON_SAMPLE_COUNT, HORIZON_ASSET_NAMESPACE, HORIZON_SCHEMA_VERSION,
    HorizonBackground, HorizonCatalog,
};
pub use observer::{
    CoordinateAxis, GeoLocation, MergeOutcome, OBSERVER_SCHEMA_VERSION, ObservatoryIcon,
    ObservatoryProfile, ObserverCatalog, ProfileSource, TimezoneConfig, TimezoneMode,
    estimate_longitude_from_utc_offset_minutes, estimate_utc_offset_minutes_from_longitude,
    parse_coordinate, parse_elevation_m, parse_utc_offset_minutes,
};
pub use settings::{
    AppSettings, AtmosphereSettings, DEFAULT_ATMOSPHERE_STRENGTH, DEFAULT_GRID_OPACITY,
    DEFAULT_GROUND_OPACITY, DEFAULT_HIPS_SURVEY_OPACITY, DEFAULT_HORIZON_OPACITY,
    DEFAULT_STARS_OPACITY, DebugSettings, GridLayerSettings, GroundSettings, HorizonLayerSettings,
    KeymapSettings, PlanetariumSettings, SETTINGS_SCHEMA_VERSION, StarsSettings, SurveySettings,
};
pub use store::{DocumentStore, Persistable};
pub use tz_offset::lookup_standard_utc_offset_minutes;

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
            horizon_id: None,
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

    #[test]
    fn coordinate_standard_utc_offset_lookup() {
        let southern_france_lat = parse_coordinate("43°28'54\" N", CoordinateAxis::Latitude).unwrap();
        let southern_france_lon = parse_coordinate("1°21'07\" E", CoordinateAxis::Longitude).unwrap();
        assert_eq!(
            lookup_standard_utc_offset_minutes(southern_france_lat, southern_france_lon),
            60
        );
        assert_eq!(
            GeoLocation::new(southern_france_lat, southern_france_lon, 150.0)
                .standard_utc_offset_minutes(),
            60
        );

        let cases = [
            // France & Spain (UTC+1 despite near-0° or negative longitudes)
            (48.8364, 2.3364, 60),    // Paris
            (42.9369, 0.1411, 60),    // Pic du Midi
            (48.3904, -4.4861, 60),   // Brest
            (40.4168, -3.7038, 60),   // Madrid
            (42.2406, -8.7207, 60),   // Vigo
            // UK & Portugal (UTC+0)
            (51.5074, -0.1278, 0),    // London
            (51.1279, 1.3134, 0),     // Dover
            (38.7223, -9.1393, 0),    // Lisbon
            // Non-15°-aligned and fractional-hour zones
            (43.8256, 87.6168, 480),  // Urumqi (UTC+8)
            (28.6139, 77.2090, 330),  // New Delhi (UTC+5:30)
            (27.7172, 85.3240, 345),  // Kathmandu (UTC+5:45)
            (35.6892, 51.3890, 210),  // Tehran (UTC+3:30)
            (-31.6767, 128.8886, 525), // Eucla (UTC+8:45)
            (-34.9285, 138.6007, 570), // Adelaide (UTC+9:30)
            (-33.8688, 151.2093, 600), // Sydney (UTC+10)
            (47.5615, -52.7126, -210), // St. John's (UTC-3:30)
            (40.7128, -74.0060, -300), // New York (UTC-5)
            (-32.8895, -68.8458, -180), // Mendoza (UTC-3)
            (35.6762, 139.6503, 540),  // Tokyo (UTC+9)
            // Boundaries & non-finite fallback
            (0.0, -180.0, -720),
            (0.0, 180.0, 720),
            (f64::NAN, 15.0, 60),
            (43.5, f64::NAN, 0),
        ];

        for (lat, lon, expected) in cases {
            assert_eq!(
                lookup_standard_utc_offset_minutes(lat, lon),
                expected,
                "lat={lat}, lon={lon}"
            );
        }
    }

    #[test]
    fn app_settings_default_and_round_trip() {
        let defaults = AppSettings::default();
        assert_eq!(defaults.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(!defaults.debug.show_hips_grid);

        let dir = temp_test_dir();
        let store = DocumentStore::open_dir(dir.clone());

        assert_eq!(store.load_or_default::<AppSettings>(), defaults);

        let updated = AppSettings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            planetarium: PlanetariumSettings {
                atmosphere: AtmosphereSettings {
                    enabled: false,
                    strength: 0.45,
                },
                ground: GroundSettings {
                    enabled: false,
                    opacity: 0.65,
                },
                horizon: HorizonLayerSettings {
                    enabled: false,
                    opacity: 0.8,
                    show_line: true,
                },
                hips_survey: SurveySettings {
                    enabled: false,
                    opacity: 0.35,
                },
                stars: StarsSettings {
                    enabled: false,
                    opacity: 0.75,
                },
                altaz_grid: GridLayerSettings {
                    enabled: false,
                    opacity: 0.55,
                },
                equatorial_grid: GridLayerSettings {
                    enabled: true,
                    opacity: 0.85,
                },
            },
            debug: DebugSettings {
                show_hips_grid: true,
            },
            keymap: KeymapSettings::default(),
        };
        store.save(&updated).expect("save succeeds");
        let loaded: AppSettings = store.load().expect("load succeeds");
        assert_eq!(loaded, updated);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_settings_backcompat_and_planetarium_sanitization() {
        let dir = temp_test_dir();
        let store = DocumentStore::open_dir(dir.clone());
        let backend = JsonFileBackend::new(dir.clone());

        // Legacy settings.json without `planetarium` field loads with atmosphere & ground defaults.
        backend
            .write_raw(
                AppSettings::DOCUMENT_KEY,
                br#"{"schema_version":1,"debug":{"show_hips_grid":true}}"#,
            )
            .expect("raw write succeeds");
        let legacy: AppSettings = store.load().expect("legacy settings load");
        assert!(legacy.debug.show_hips_grid);
        assert!(legacy.planetarium.atmosphere.enabled);
        assert_eq!(
            legacy.planetarium.atmosphere.strength,
            DEFAULT_ATMOSPHERE_STRENGTH
        );
        assert!(legacy.planetarium.ground.enabled);
        assert_eq!(legacy.planetarium.ground.opacity, DEFAULT_GROUND_OPACITY);
        assert!(legacy.planetarium.hips_survey.enabled);
        assert_eq!(
            legacy.planetarium.hips_survey.opacity,
            DEFAULT_HIPS_SURVEY_OPACITY
        );
        assert!(legacy.planetarium.stars.enabled);
        assert_eq!(legacy.planetarium.stars.opacity, DEFAULT_STARS_OPACITY);
        assert!(legacy.planetarium.altaz_grid.enabled);
        assert_eq!(legacy.planetarium.altaz_grid.opacity, DEFAULT_GRID_OPACITY);
        assert!(legacy.planetarium.equatorial_grid.enabled);
        assert_eq!(
            legacy.planetarium.equatorial_grid.opacity,
            DEFAULT_GRID_OPACITY
        );

        // Out-of-bounds strengths and opacities are clamped on load.
        backend
            .write_raw(
                AppSettings::DOCUMENT_KEY,
                br#"{"schema_version":1,"planetarium":{"atmosphere":{"enabled":true,"strength":2.5},"ground":{"enabled":true,"opacity":1.8},"hips_survey":{"enabled":true,"opacity":3.0},"stars":{"enabled":true,"opacity":1.4},"altaz_grid":{"enabled":true,"opacity":4.0},"equatorial_grid":{"enabled":true,"opacity":2.1}}}"#,
            )
            .expect("raw write succeeds");
        let clamped_hi: AppSettings = store.load().expect("high values load");
        assert_eq!(clamped_hi.planetarium.atmosphere.strength, 1.0);
        assert_eq!(clamped_hi.planetarium.ground.opacity, 1.0);
        assert_eq!(clamped_hi.planetarium.hips_survey.opacity, 1.0);
        assert_eq!(clamped_hi.planetarium.stars.opacity, 1.0);
        assert_eq!(clamped_hi.planetarium.altaz_grid.opacity, 1.0);
        assert_eq!(clamped_hi.planetarium.equatorial_grid.opacity, 1.0);

        backend
            .write_raw(
                AppSettings::DOCUMENT_KEY,
                br#"{"schema_version":1,"planetarium":{"atmosphere":{"enabled":true,"strength":-0.4},"ground":{"enabled":true,"opacity":-0.2},"hips_survey":{"enabled":true,"opacity":-0.5},"stars":{"enabled":true,"opacity":-1.0},"altaz_grid":{"enabled":true,"opacity":-0.3},"equatorial_grid":{"enabled":true,"opacity":-2.0}}}"#,
            )
            .expect("raw write succeeds");
        let clamped_lo: AppSettings = store.load().expect("negative values load");
        assert_eq!(clamped_lo.planetarium.atmosphere.strength, 0.0);
        assert_eq!(clamped_lo.planetarium.ground.opacity, 0.0);
        assert_eq!(clamped_lo.planetarium.hips_survey.opacity, 0.0);
        assert_eq!(clamped_lo.planetarium.stars.opacity, 0.0);
        assert_eq!(clamped_lo.planetarium.altaz_grid.opacity, 0.0);
        assert_eq!(clamped_lo.planetarium.equatorial_grid.opacity, 0.0);

        let mut non_finite = AtmosphereSettings {
            enabled: true,
            strength: f32::NAN,
        };
        non_finite.sanitize();
        assert_eq!(non_finite.strength, DEFAULT_ATMOSPHERE_STRENGTH);

        let disabled = AtmosphereSettings {
            enabled: false,
            strength: 0.75,
        };
        assert_eq!(disabled.effective_strength(), 0.0);

        let mut non_finite_ground = GroundSettings {
            enabled: true,
            opacity: f32::INFINITY,
        };
        non_finite_ground.sanitize();
        assert_eq!(non_finite_ground.opacity, DEFAULT_GROUND_OPACITY);

        let disabled_ground = GroundSettings {
            enabled: false,
            opacity: 0.6,
        };
        assert_eq!(disabled_ground.effective_opacity(), 0.0);

        let mut non_finite_hips = SurveySettings {
            enabled: true,
            opacity: f32::NEG_INFINITY,
        };
        non_finite_hips.sanitize();
        assert_eq!(non_finite_hips.opacity, DEFAULT_HIPS_SURVEY_OPACITY);

        let disabled_hips = SurveySettings {
            enabled: false,
            opacity: 0.5,
        };
        assert_eq!(disabled_hips.effective_opacity(), 0.0);

        let mut non_finite_stars = StarsSettings {
            enabled: true,
            opacity: f32::NAN,
        };
        non_finite_stars.sanitize();
        assert_eq!(non_finite_stars.opacity, DEFAULT_STARS_OPACITY);

        let disabled_stars = StarsSettings {
            enabled: false,
            opacity: 0.4,
        };
        assert_eq!(disabled_stars.effective_opacity(), 0.0);

        let mut non_finite_grid = GridLayerSettings {
            enabled: true,
            opacity: f32::NAN,
        };
        non_finite_grid.sanitize();
        assert_eq!(non_finite_grid.opacity, DEFAULT_GRID_OPACITY);

        let disabled_grid = GridLayerSettings {
            enabled: false,
            opacity: 0.7,
        };
        assert_eq!(disabled_grid.effective_opacity(), 0.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_settings_rejects_future_schema_version() {
        let store = DocumentStore::in_memory();
        let future = AppSettings {
            schema_version: SETTINGS_SCHEMA_VERSION + 5,
            ..AppSettings::default()
        };
        store.save(&future).expect("save succeeds");

        let err = store.load::<AppSettings>().unwrap_err();
        assert!(matches!(
            err,
            PersistenceError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn binary_assets_round_trip_and_validate_names() {
        for backend_kind in [false, true] {
            let dir = temp_test_dir();
            let store = if backend_kind {
                DocumentStore::open_dir(dir.clone())
            } else {
                DocumentStore::in_memory()
            };

            let payload = vec![1_u8, 2, 3, 4, 250, 255];
            store
                .write_asset(HORIZON_ASSET_NAMESPACE, "garden.jpg", &payload)
                .expect("write_asset succeeds");
            assert_eq!(
                store
                    .read_asset(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                    .expect("read_asset succeeds"),
                Some(payload.clone())
            );
            if backend_kind {
                assert!(
                    store
                        .asset_path(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                        .is_some()
                );
            } else {
                assert!(
                    store
                        .asset_path(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                        .is_none()
                );
            }

            store
                .delete_asset(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                .expect("delete_asset succeeds");
            assert_eq!(
                store
                    .read_asset(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                    .expect("read missing succeeds"),
                None
            );
            // Deleting a missing asset should succeed idempotently.
            store
                .delete_asset(HORIZON_ASSET_NAMESPACE, "garden.jpg")
                .expect("delete missing succeeds");

            assert!(matches!(
                store.write_asset(HORIZON_ASSET_NAMESPACE, "../escape.jpg", b"x"),
                Err(PersistenceError::InvalidAssetName(_))
            ));
            assert!(matches!(
                store.write_asset(HORIZON_ASSET_NAMESPACE, ".hidden", b"x"),
                Err(PersistenceError::InvalidAssetName(_))
            ));

            if backend_kind {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    #[test]
    fn horizon_catalog_crud_persistence_and_observer_link() {
        let dir = temp_test_dir();
        let store = DocumentStore::open_dir(dir.clone());

        let mut horizons = HorizonCatalog::default();
        let bg = horizons.create_background(
            None,
            "Backyard Trees",
            Some("backyard-trees.jpg".to_string()),
            Some("pano.jpg".to_string()),
            Some((4096, 2048)),
            vec![12.0; 720],
            35.0,
        );
        let bg_id = bg.id.clone();
        assert_eq!(bg_id, "backyard-trees");
        assert_eq!(bg.revision, 1);
        assert_eq!(bg.image_revision, 1);

        // Editing horizon curve bumps revision but NOT image_revision.
        horizons
            .commit_horizon(&bg_id, vec![15.0; 720], 42.0)
            .expect("commit_horizon succeeds");
        let after_commit = horizons.background(&bg_id).unwrap();
        assert_eq!(after_commit.revision, 2);
        assert_eq!(after_commit.image_revision, 1);
        assert!((after_commit.azimuth_offset_deg - 42.0).abs() < 1e-9);

        // Replacing image bumps both revision and image_revision.
        let (prev_asset, after_img) = horizons
            .replace_image(
                &bg_id,
                Some("backyard-trees-v2.png".to_string()),
                Some("new.png".to_string()),
                Some((2048, 1024)),
                None,
            )
            .expect("replace_image succeeds");
        assert_eq!(prev_asset.as_deref(), Some("backyard-trees.jpg"));
        assert_eq!(after_img.revision, 3);
        assert_eq!(after_img.image_revision, 2);

        store.save(&horizons).expect("save horizon catalog");
        let loaded: HorizonCatalog = store.load().expect("load horizon catalog");
        assert_eq!(loaded, horizons);

        // ObserverCatalog links and sanitizes horizon_id.
        let mut observers = ObserverCatalog::default();
        assert_eq!(observers.active_profile().horizon_id, None);
        observers
            .set_horizon("backyard", Some(bg_id.clone()))
            .expect("set_horizon succeeds");
        assert_eq!(
            observers.active_profile().horizon_id.as_deref(),
            Some(bg_id.as_str())
        );
        observers
            .set_horizon("backyard", Some("   ".to_string()))
            .expect("clear empty horizon_id succeeds");
        assert_eq!(observers.active_profile().horizon_id, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_horizon_layer_backcompat_and_sanitization() {
        let store = DocumentStore::in_memory();
        let defaults = store.load_or_default::<AppSettings>();
        assert!(defaults.planetarium.horizon.enabled);
        assert_eq!(defaults.planetarium.horizon.opacity, DEFAULT_HORIZON_OPACITY);
        assert!(!defaults.planetarium.horizon.show_line);

        let legacy: HorizonLayerSettings =
            serde_json::from_str(r#"{"enabled":true,"opacity":0.8}"#).unwrap();
        assert!(!legacy.show_line);

        let enabled_line: HorizonLayerSettings =
            serde_json::from_str(r#"{"enabled":true,"opacity":0.8,"show_line":true}"#).unwrap();
        assert!(enabled_line.show_line);

        let mut h = HorizonLayerSettings {
            enabled: true,
            opacity: f32::NAN,
            show_line: false,
        };
        h.sanitize();
        assert_eq!(h.opacity, DEFAULT_HORIZON_OPACITY);

        let disabled = HorizonLayerSettings {
            enabled: false,
            opacity: 0.6,
            show_line: true,
        };
        assert_eq!(disabled.effective_opacity(), 0.0);
    }
}
