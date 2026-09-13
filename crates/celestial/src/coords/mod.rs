// Re-inventing the wheel :)

pub mod altitude;
pub mod angle;
pub mod frames;
pub mod geodetic;
pub mod spherical;
pub mod topocentric;

pub use angle::{
    Angle, AngleInterval, canonical_spherical_to_cartesian, is_multiple_of,
    topocentric_spherical_to_enu,
};
pub use spherical::SphericalRect;
