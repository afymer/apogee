#![recursion_limit = "256"]

pub(crate) mod bridge;
pub mod control_panel;
pub mod histogram;
pub mod panel;
pub mod raw_image;
pub mod renderer;
pub mod types;

pub use components::{ActiveKnob, DoubleSlider, DoubleSliderEvent, SliderColors, SliderThemeExt};
pub use control_panel::CaptureControlPanel;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::RawViewerBridge;
    use glam::Vec2;

    fn dummy_raw_image(width: u32, height: u32) -> RawImage {
        RawImage {
            make: "Test".into(),
            model: "Sensor".into(),
            width,
            height,
            raw_pixels: vec![0; (width * height) as usize],
            cfa_pattern: [0, 1, 1, 2],
            black_levels: [0.0; 4],
            white_levels: [16383.0; 4],
            wb_coeffs: [1.0; 4],
        }
    }

    #[test]
    fn test_set_one_to_one_pillarboxed_and_letterboxed() {
        // Pillarboxed: screen_aspect (16/9) > img_aspect (4/3).
        let bridge = RawViewerBridge::new(|| {});
        bridge.set_image(dummy_raw_image(4000, 3000));
        bridge.resize(1920, 1080);
        bridge.set_one_to_one();

        let vt = bridge.view_transform();
        let fitted_w = 1080.0_f32 * (4000.0 / 3000.0);
        let expected_zoom = 4000.0_f32 / fitted_w;
        assert_eq!(vt.pan, Vec2::ZERO);
        assert!((vt.zoom - expected_zoom).abs() < 1e-5);

        // Letterboxed: screen_aspect (1.0) < img_aspect (2.0).
        bridge.set_image(dummy_raw_image(4000, 2000));
        bridge.resize(1000, 1000);
        bridge.set_one_to_one();

        let vt = bridge.view_transform();
        assert_eq!(vt.pan, Vec2::ZERO);
        assert!((vt.zoom - 4.0).abs() < 1e-5);
    }

    #[test]
    fn test_histogram_data_from_raw_bins_and_max_visible() {
        let mut bins = [0.0f32; 1024];
        bins[10] = 5.0;
        bins[256 + 100] = 12.0;
        bins[512 + 200] = 8.0;
        bins[768 + 50] = 20.0;

        let hist = HistogramData::from_raw_bins(&bins);
        assert_eq!(hist.r[10], 5.0);
        assert_eq!(hist.g[100], 12.0);
        assert_eq!(hist.b[200], 8.0);
        assert_eq!(hist.l[50], 20.0);
        assert_eq!(hist.max_r, 5.0);
        assert_eq!(hist.max_g, 12.0);
        assert_eq!(hist.max_b, 8.0);
        assert_eq!(hist.max_l, 20.0);

        assert_eq!(hist.max_visible(true, true, true, true), 20.0);
        assert_eq!(hist.max_visible(true, true, true, false), 12.0);
        assert_eq!(hist.max_visible(true, false, true, false), 8.0);
        assert_eq!(hist.max_visible(true, false, false, false), 5.0);
        assert_eq!(hist.max_visible(false, false, false, false), 1e-6);

        let empty = HistogramData::from_raw_bins(&[0.0f32; 1024]);
        assert_eq!(empty.max_visible(true, true, true, true), 1e-6);
    }
}
