pub mod catalog;
pub mod coords;
pub mod observer;
pub mod projection;

pub use catalog::{
    read_stars_from_zst, write_stars_to_zst, StarCatalogHeader, StarRecord, STAR_CATALOG_MAGIC,
    STAR_CATALOG_VERSION,
};
pub use observer::ObservationContext;
pub use projection::{CameraProjection, ProjectionKind};

