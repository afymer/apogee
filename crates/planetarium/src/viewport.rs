use crate::camera::ViewMode;

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

#[derive(Debug, Clone, PartialEq)]
pub enum ViewportCommand {
    Resize(u32, u32),
    Pan { delta_x: f64, delta_y: f64 },
    Zoom { delta: f64 },
    SetViewMode(ViewMode),
    ToggleHipsGrid(bool),
    ToggleHipsSurvey(bool),
    SetHipsSurvey(hips::HipsSurvey),
    SetObserver(ObserverUpdate),
}

