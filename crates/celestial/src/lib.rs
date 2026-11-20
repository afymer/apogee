pub mod catalog;
pub mod coords;
pub mod observer;
pub mod projection;
pub mod solar;
pub mod visibility;

pub use catalog::{
    STAR_CATALOG_MAGIC, STAR_CATALOG_VERSION, StarCatalog, StarCatalogHeader, StarMetadata,
    StarRecord, read_star_catalog_from_zst, read_stars_from_zst, write_star_catalog_to_zst,
    write_stars_to_zst,
};
pub use observer::ObservationContext;
pub use projection::{CameraProjection, ProjectionKind};
pub use visibility::{
    DailyTargetVisibility, compute_target_visibility, equatorial_altitude_deg,
    sample_day_equatorial_altitude,
};

