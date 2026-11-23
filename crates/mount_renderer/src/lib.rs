//! Headless 3D mount kinematics, procedural geometry, attitude filter, and wgpu overlay renderer.
//!
//! Intentionally decoupled from GPUI and Bevy/Planetarium so 3D rendering and math compile
//! quickly and can be unit-tested without any UI runtime.

pub mod attitude;
pub mod mesh;
pub mod renderer;
pub mod scene;

pub use attitude::{ATTITUDE_FILTER_TAU_SECS, ATTITUDE_SETTLE_EPS_DEG, step_angular_lowpass};
pub use renderer::{
    BASE_FOV_Y_DEG, MAX_FOV_Y_DEG, MAX_ORBIT_DISTANCE, MIN_ORBIT_DISTANCE, MountOverlayRenderer,
    OrbitCamera, TARGET_FORMAT,
};
pub use scene::{
    COMPASS_RADIUS, EquatorialFrame, MountSceneState, PIVOT_Y, SceneGeometry, WidgetVisibility,
    build_scene,
};
