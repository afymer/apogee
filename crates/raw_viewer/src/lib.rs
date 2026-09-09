pub(crate) mod bridge;
pub mod control_panel;
pub mod histogram;
pub mod panel;
pub mod raw_image;
pub mod renderer;
pub mod types;

pub use control_panel::CaptureControlPanel;
pub use controls::{ActiveKnob, DoubleSlider, DoubleSliderEvent, SliderColors, SliderThemeExt};
pub use histogram::{HistogramData, HistogramWidget, ScaleMode};
pub use panel::RawViewerPanel;
pub use raw_image::{RawError, RawImage};
pub use renderer::{
    ImageUniforms, RawViewerRenderer, StretchUniforms, TARGET_FORMAT, ViewUniforms,
};
pub use types::{DisplayRange, ViewTransform};

use gpui::App;
use workspace::workbench::Workbench;

pub fn init(cx: &mut App) {
    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let viewer = RawViewerPanel::build(window, cx, 1024, 1024);
            let bridge = viewer.read(cx).bridge().clone();
            let control_panel = CaptureControlPanel::build(bridge, cx);

            cx.observe(&viewer, |_this, _panel, cx| {
                cx.notify();
            })
            .detach();

            cx.observe(&control_panel, |_this, _panel, cx| {
                cx.notify();
            })
            .detach();

            workbench.add_panel(viewer, window, cx);
            workbench.add_panel(control_panel, window, cx);
        }
    })
    .detach();
}
