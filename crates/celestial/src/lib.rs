pub mod catalog;
pub mod coords;
pub mod dso;
pub mod horizon;
pub mod observer;
pub mod planets;
pub mod projection;
pub mod solar;
pub mod visibility;

pub use catalog::{
    STAR_CATALOG_MAGIC, STAR_CATALOG_VERSION, StarCatalog, StarCatalogHeader, StarMetadata,
    StarRecord, read_star_catalog_from_zst, read_stars_from_zst, write_star_catalog_to_zst,
    write_stars_to_zst,
};
pub use dso::{
    DSO_CATALOG_MAGIC, DSO_CATALOG_VERSION, DsoCatalog, DsoCatalogHeader, DsoGroup, DsoKind,
    DsoRecord, read_dso_catalog_from_zst, write_dso_catalog_to_zst,
};
pub use horizon::{
    DEFAULT_HORIZON_SAMPLES, HorizonProfile, MAX_HORIZON_ALT_DEG, MIN_HORIZON_ALT_DEG,
};
pub use observer::ObservationContext;
pub use planets::PlanetOrientation;
pub use projection::{CameraProjection, ProjectionKind};
pub use solar::{
    LunarPhaseInfo, SkyPhase, SolarSystemBody, SolarSystemEphemeris, lunar_ephemeris,
    lunar_geocentric_of_date, lunar_topocentric_of_date, solar_ephemeris, solar_system_ephemeris,
};
pub use visibility::{
    DailyTargetVisibility, DailyTrajectory, TrajectoryEventPoint, TrajectoryPointKind,
    TrajectorySample, compute_equatorial_daily_trajectory, compute_solar_system_daily_trajectory,
    compute_solar_system_visibility_with_horizon, compute_solar_system_window_trajectory,
    compute_target_visibility, compute_target_visibility_with_horizon, equatorial_altitude_deg,
    equatorial_az_alt_deg, sample_day_equatorial_altitude, sample_day_equatorial_track,
    sample_day_solar_system_track, solar_system_az_alt_deg,
};
