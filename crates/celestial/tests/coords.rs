use celestial::catalog::{
    STAR_CATALOG_MAGIC, STAR_CATALOG_VERSION, StarCatalogHeader, StarRecord, read_stars_from_zst,
    write_stars_to_zst,
};
use celestial::coords::{Angle, AngleInterval, altitude::Altitude, geodetic::GeodeticPosition};
use celestial::observer::ObservationContext;
use hifitime::Epoch;
use std::f64::consts::PI;

#[test]
fn test_angle_conversions() {
    let deg = Angle::from_degrees(180.0);
    assert!((deg.radians() - PI).abs() < 1e-12);
    assert!((deg.hours() - 12.0).abs() < 1e-12);

    let hours = Angle::from_hours(12.0);
    assert!((hours.degrees() - 180.0).abs() < 1e-12);
    assert!((hours.radians() - PI).abs() < 1e-12);
}

#[test]
fn test_angle_interval_containment_and_wrapping() {
    let linear = AngleInterval::from_degrees(10.0, 50.0);
    assert!(!linear.is_wrapping());
    assert!(linear.contains(Angle::from_degrees(10.0)));
    assert!(linear.contains(Angle::from_degrees(30.0)));
    assert!(linear.contains(Angle::from_degrees(50.0)));
    assert!(!linear.contains(Angle::from_degrees(9.9)));
    assert!(!linear.contains(Angle::from_degrees(50.1)));

    let wrapping = AngleInterval::from_degrees(350.0, 20.0);
    assert!(wrapping.is_wrapping());
    assert!(wrapping.contains(Angle::from_degrees(355.0)));
    assert!(wrapping.contains(Angle::from_degrees(0.0)));
    assert!(wrapping.contains(Angle::from_degrees(10.0)));
    assert!(!wrapping.contains(Angle::from_degrees(180.0)));
    assert!(!wrapping.contains(Angle::from_degrees(349.0)));
    assert!(!wrapping.contains(Angle::from_degrees(21.0)));

    let full = AngleInterval::full_circle();
    assert!(!full.is_wrapping());
    assert!((full.span().degrees() - 360.0).abs() < 1e-10);

    let lat = AngleInterval::latitude_range();
    assert!((lat.start().degrees() + 90.0).abs() < 1e-10);
    assert!((lat.end().degrees() - 90.0).abs() < 1e-10);
}

#[test]
fn test_observation_context_roundtrip_and_sidereal_time() {
    let location = GeodeticPosition {
        longitude: Angle::from_degrees(2.3364),
        latitude: Angle::from_degrees(48.8364),
        altitude_m: Altitude::from_wgs84(67.0),
    };
    let e1 = Epoch::from_gregorian_utc_hms(2026, 9, 17, 21, 0, 0);
    let e2 = Epoch::from_gregorian_utc_hms(2026, 9, 17, 22, 0, 0);

    let ctx1 = ObservationContext::new(location, e1);
    let ctx2 = ObservationContext::new(location, e2);

    let delta_lst_deg = (ctx2.local_sidereal_time() - ctx1.local_sidereal_time())
        .normalize()
        .degrees();
    // Mean sidereal rate is ~360.98564736629 / 24 ≈ 15.04106864 deg/h.
    assert!(
        (delta_lst_deg - 15.04106864).abs() < 1e-4,
        "unexpected LST advance: {delta_lst_deg}"
    );

    let test_points = [
        (0.0, 0.0),
        (12.5, 45.0),
        (359.25, -35.5),
        (210.0, 82.0),
        (88.0, -70.0),
    ];
    for (ra_deg, dec_deg) in test_points {
        let ra = Angle::from_degrees(ra_deg);
        let dec = Angle::from_degrees(dec_deg);
        let (az, alt) = ctx1.equatorial_to_topocentric_coords(ra, dec);
        let (ra_rt, dec_rt) = ctx1.topocentric_to_equatorial_coords(az, alt);

        let d_ra = (ra_rt - ra).signed_radians().abs();
        let d_dec = (dec_rt - dec).radians().abs();
        assert!(d_ra < 1e-10, "RA roundtrip error too large: {d_ra}");
        assert!(d_dec < 1e-10, "Dec roundtrip error too large: {d_dec}");
    }
}

#[test]
fn test_star_catalog_zstd_roundtrip_and_validation() {
    let stars = vec![
        StarRecord::from_icrs_spherical(1.25, 0.45, -1.46, 0.01),
        StarRecord::from_icrs_spherical(5.1, -0.8, 6.24, 1.35),
    ];
    for s in &stars {
        let norm = (s.pos[0] * s.pos[0] + s.pos[1] * s.pos[1] + s.pos[2] * s.pos[2]).sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
    assert!((stars[0].mag() + 1.46).abs() < 1e-5);
    assert!((stars[0].color_index() - 0.01).abs() < 1e-5);
    assert!((stars[1].mag() - 6.24).abs() < 1e-5);
    assert!((stars[1].color_index() - 1.35).abs() < 1e-5);

    let header = StarCatalogHeader::new(stars.len() as u32, -1.46, 6.24);
    assert!(header.validate().is_ok());

    let mut compressed = Vec::new();
    write_stars_to_zst(&mut compressed, &header, &stars, 3).expect("write catalog");

    let (read_header, read_stars) =
        read_stars_from_zst(compressed.as_slice()).expect("read catalog");
    assert_eq!(read_header, header);
    assert_eq!(read_stars, stars);

    let mut bad_magic = header;
    bad_magic.magic = *b"BAD!";
    assert!(bad_magic.validate().is_err());

    let mut bad_magic_buf = Vec::new();
    write_stars_to_zst(&mut bad_magic_buf, &bad_magic, &stars, 1).unwrap();
    assert!(read_stars_from_zst(bad_magic_buf.as_slice()).is_err());

    let bad_version = StarCatalogHeader {
        magic: STAR_CATALOG_MAGIC,
        version: STAR_CATALOG_VERSION + 1,
        star_count: 0,
        min_mag: 0.0,
        max_mag: 0.0,
        padding: [0; 12],
    };
    assert!(bad_version.validate().is_err());
}
