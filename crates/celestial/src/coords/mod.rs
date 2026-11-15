// Re-inventing the wheel :)

pub mod altitude;
pub mod angle;
pub mod geodetic;
pub mod spherical;

pub use angle::{
    Angle, AngleInterval, canonical_spherical_to_cartesian, cartesian_to_canonical_spherical,
    is_multiple_of, topocentric_enu_to_spherical, topocentric_spherical_to_enu,
};
pub use spherical::SphericalRect;
