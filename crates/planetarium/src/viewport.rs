use crate::camera::{TrackingTarget, ViewMode};
use crate::landscape::HorizonLayerData;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObserverClockSpec {
    Live,
    FixedUnixSeconds(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObserverUpdate {
    pub location: celestial::coords::geodetic::GeodeticPosition,
    pub clock: ObserverClockSpec,
}

/// Topocentric trajectory polyline uploaded to the GPU line pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TrajectoryRenderData {
    /// Ordered `(topocentric_enu, above_horizon, edge_fade)` samples along the path (`edge_fade ∈ [0, 1]`).
    pub points: Vec<([f32; 3], bool, f32)>,
    /// Representative sRGB tint for the trajectory curve.
    pub srgb: [f32; 3],
    /// Whether ground occlusion dims the below-horizon continuation more strongly.
    pub occlude_ground: bool,
}

/// Camera override used when the Mount 3D workspace renders over the shared Planetarium sky.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountCameraOverride {
    /// Horizontal look azimuth in radians (`0` = North, `π/2` = East).
    pub az_rad: f64,
    /// Horizontal look altitude in radians (`-π/2..=π/2`).
    pub alt_rad: f64,
    /// Vertical perspective field of view in radians.
    pub fov_y_rad: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewportCommand {
    Resize(u32, u32),
    Pan {
        delta_x: f64,
        delta_y: f64,
    },
    Zoom {
        delta: f64,
        cursor_ndc: Option<(f64, f64)>,
    },
    SetViewMode(ViewMode),
    ToggleHipsGrid(bool),
    ToggleHipsSurvey(bool),
    SetHipsSurvey(hips::HipsSurvey),
    SetHipsSurveyOpacity(f32),
    SetStarOpacity(f32),
    SetDsoOpacity(f32),
    SetDsoCatalogFilter {
        show_messier: bool,
        show_ngc: bool,
    },
    SetSolarSystemOpacity(f32),
    SetAltAzGridOpacity(f32),
    SetEquatorialGridOpacity(f32),
    SetObserver(ObserverUpdate),
    SetAtmosphereStrength(f32),
    SetGroundOpacity(f32),
    SetHorizon(Option<HorizonLayerData>),
    SetHorizonOpacity(f32),
    SetHorizonLineVisible(bool),
    SetTrajectory(Option<TrajectoryRenderData>),
    SetTrajectoryOpacity(f32),
    TrackTarget(Option<TrackingTarget>),
    SetMountCamera(Option<MountCameraOverride>),
}
