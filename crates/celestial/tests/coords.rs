use celestial::ObservationContext;
use celestial::coords::Angle;
use glam::DVec3;
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
