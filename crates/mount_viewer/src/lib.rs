#![recursion_limit = "256"]

pub mod bridge;
pub mod control_panel;
pub mod gizmos;
pub mod slew_keys;
pub mod viewport_panel;

use gpui::{App, Global};
use workspace::workbench::Workbench;

pub use control_panel::MountControlPanel;
pub use mount_renderer::{
    BASE_FOV_Y_DEG, MAX_FOV_Y_DEG, MAX_ORBIT_DISTANCE, MIN_ORBIT_DISTANCE, MountOverlayRenderer,
    MountSceneState, OrbitCamera, WidgetVisibility, step_angular_lowpass,
};
pub use viewport_panel::MountViewportPanel;

/// Shared `BevyBridge` published by `bevy_viewport` so the Mount workspace renders its 3D
/// attitude overlay directly on top of the Planetarium's sky without duplicating GPU resources.
pub struct SharedPlanetariumBridge(pub planetarium::BevyBridge);

impl Global for SharedPlanetariumBridge {}

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        let Some(window) = window else {
            return;
        };

        let viewport = MountViewportPanel::build(window, cx);
        let bridge = viewport.read(cx).bridge().clone();
        let control = MountControlPanel::build(bridge, window, cx);

        cx.observe(&viewport, |_this, _panel, cx| cx.notify())
            .detach();
        cx.observe(&control, |_this, _panel, cx| cx.notify())
            .detach();

        workbench.add_panel(viewport, window, cx);
        workbench.add_panel(control, window, cx);
    })
    .detach();
}
