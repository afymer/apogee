pub(crate) mod bridge;
pub mod control_panel;
pub mod double_slider;
pub mod panel;
pub mod raw_image;
pub mod renderer;
pub mod types;

pub use control_panel::CaptureControlPanel;
pub use double_slider::{ActiveKnob, DoubleSlider};
pub use panel::RawViewerPanel;
pub use raw_image::{RawError, RawImage};
pub use renderer::{RawViewerRenderer, TARGET_FORMAT};
pub use types::{DisplayRange, ViewTransform};
pub use workspace::theme::{SliderColors, SliderThemeExt};

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
    use std::path::Path;

    #[test]
    fn test_shader_compilation() {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }));

        if let Ok(adapter) = adapter {
            if let Ok((device, _queue)) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            {
                let shader_code = include_str!("shader.wgsl");
                let _shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Test Shader Module"),
                    source: wgpu::ShaderSource::Wgsl(shader_code.into()),
                });
            }
        }
    }

    #[test]
    fn test_empty_buffer_fails() {
        let result = RawImage::decode_from_memory(&[]);
        assert!(matches!(result, Err(RawError::EmptyBuffer)));
    }

    #[test]
    fn test_decode_from_memory_if_sample_exists() {
        let sample_path = Path::new("/tmp/full_image.cr2");
        if sample_path.exists() {
            let bytes = std::fs::read(sample_path).expect("failed to read test file");
            let raw =
                RawImage::decode_from_memory(&bytes).expect("failed to decode CR2 from memory");
            assert!(raw.width > 0);
            assert!(raw.height > 0);
            assert_eq!(raw.raw_pixels.len(), (raw.width * raw.height) as usize);
            println!(
                "Decoded sample CR2: {} {} ({}x{})",
                raw.make, raw.model, raw.width, raw.height
            );
        }
    }

    #[test]
    fn test_renderer_end_to_end() {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }));

        let Ok(adapter) = adapter else {
            return;
        };

        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };

        let mut renderer = RawViewerRenderer::new(device, queue);

        // Render empty frame
        let _view1 = renderer.render(800, 600);
        // Test setting display range
        renderer.set_display_range(0.02, 0.15);
        let (min, max) = renderer.display_range();
        assert!((min - 0.02).abs() < 1e-5);
        assert!((max - 0.15).abs() < 1e-5);

        // Test pan & zoom in renderer
        renderer.set_pan_zoom(glam::vec2(0.1, -0.05), 2.5);
        let (pan, zoom) = renderer.pan_zoom();
        assert!((pan.x - 0.1).abs() < 1e-5);
        assert!((pan.y - -0.05).abs() < 1e-5);
        assert!((zoom - 2.5).abs() < 1e-5);

        // Render with custom range and pan/zoom
        let _view4 = renderer.render(800, 600);

        // Test clamping
        renderer.set_display_range(-0.5, 2.0);
        let (c_min, c_max) = renderer.display_range();
        assert_eq!(c_min, 0.0);
        assert_eq!(c_max, 1.0);
    }

    #[test]
    fn test_bridge_display_range() {
        let bridge = RawViewerBridge::new(|| {});
        assert_eq!(bridge.display_range(), (0.0, 1.0));

        bridge.set_display_range(0.05, 0.25);
        let (min, max) = bridge.display_range();
        assert!((min - 0.05).abs() < 1e-5);
        assert!((max - 0.25).abs() < 1e-5);

        bridge.reset_display_range();
        assert_eq!(bridge.display_range(), (0.0, 1.0));
    }

    #[test]
    fn test_bridge_pan_zoom() {
        let bridge = RawViewerBridge::new(|| {});
        assert_eq!(bridge.pan_zoom(), (glam::Vec2::ZERO, 1.0));

        bridge.set_pan_zoom(glam::vec2(0.2, -0.3), 3.0);
        let (pan, zoom) = bridge.pan_zoom();
        assert!((pan.x - 0.2).abs() < 1e-5);
        assert!((pan.y - -0.3).abs() < 1e-5);
        assert!((zoom - 3.0).abs() < 1e-5);

        bridge.reset_view();
        assert_eq!(bridge.pan_zoom(), (glam::Vec2::ZERO, 1.0));
    }

    #[test]
    fn test_display_range_and_view_transform_types() {
        let range = DisplayRange::new(-0.2, 1.5);
        assert_eq!(range.min, 0.0);
        assert_eq!(range.max, 1.0);
        assert_eq!(range.as_array(), [0.0, 1.0]);

        let custom_range = DisplayRange::new(0.1, 0.8);
        assert_eq!(custom_range.min, 0.1);
        assert_eq!(custom_range.max, 0.8);
        assert_eq!(custom_range.as_array(), [0.1, 0.8]);

        let vt = ViewTransform::new(glam::vec2(10.0, -5.0), 150.0);
        assert_eq!(vt.pan, glam::vec2(10.0, -5.0));
        assert_eq!(vt.zoom, 100.0); // clamped to 100.0

        let min_vt = ViewTransform::new(glam::Vec2::ZERO, 0.01);
        assert_eq!(min_vt.zoom, 0.05); // clamped to 0.05
    }

    #[test]
    fn test_slider_theme_colors() {
        let colors = SliderColors::default();
        assert_eq!(colors.knob_border(false), colors.knob_border);
        assert_eq!(colors.knob_border(true), colors.knob_border_active);
        assert_eq!(colors.knob_background(false), colors.knob_background);
        assert_eq!(colors.knob_background(true), colors.knob_background_active);
    }
}
