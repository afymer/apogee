use crate::camera::ViewMode;
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
    /// Ordered `(topocentric_enu, above_horizon)` samples along the 24-hour path.
    pub points: Vec<([f32; 3], bool)>,
    /// Representative sRGB tint for the trajectory curve.
    pub srgb: [f32; 3],
    /// Whether ground occlusion dims the below-horizon continuation more strongly.
    pub occlude_ground: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewportCommand {
    Resize(u32, u32),
    Pan { delta_x: f64, delta_y: f64 },
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
}

