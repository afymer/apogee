use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use glam::Vec2;

use crate::{
    histogram::{HistogramData, ScaleMode},
    raw_image::{RawError, RawImage},
    renderer::RawViewerRenderer,
    types::{DisplayRange, ViewTransform},
};

#[derive(Default)]
pub struct PendingChanges {
    pub image: Option<Arc<RawImage>>,
    pub demosaic: Option<bool>,
    pub resize: Option<(u32, u32)>,
    pub range: Option<DisplayRange>,
    pub view_transform: Option<ViewTransform>,
    pub histogram_scale_modes: Option<(ScaleMode, ScaleMode)>,
}

pub struct RawViewerBridgeState {
    pub is_dirty: bool,
    pub latest_texture: Option<Arc<wgpu::TextureView>>,
    pub last_size: Option<(u32, u32)>,
    pub image_size: Option<(u32, u32)>,
    pub pending: PendingChanges,
    pub demosaic: bool,
    pub display_range: DisplayRange,
    pub view_transform: ViewTransform,
    pub histogram_scale_modes: (ScaleMode, ScaleMode),
    pub histogram: Option<Arc<HistogramData>>,
}

impl Default for RawViewerBridgeState {
    fn default() -> Self {
        Self {
            is_dirty: true,
            latest_texture: None,
            last_size: None,
            image_size: None,
            pending: PendingChanges::default(),
            demosaic: true,
            display_range: DisplayRange::DEFAULT,
            view_transform: ViewTransform::DEFAULT,
            histogram_scale_modes: (ScaleMode::Log, ScaleMode::Log),
            histogram: None,
        }
    }
}

struct BridgeInner {
    state: Mutex<RawViewerBridgeState>,
    cvar: Condvar,
    shutdown: AtomicBool,
    notify_ui_fn: Box<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub struct RawViewerBridge {
    inner: Arc<BridgeInner>,
}

impl RawViewerBridge {
    pub fn new(notify_ui: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(BridgeInner {
                state: Mutex::new(RawViewerBridgeState::default()),
                cvar: Condvar::new(),
                shutdown: AtomicBool::new(false),
                notify_ui_fn: Box::new(notify_ui),
            }),
        }
    }

    pub fn texture_view(&self) -> Option<Arc<wgpu::TextureView>> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.latest_texture.clone()
    }

    pub fn histogram(&self) -> Option<Arc<HistogramData>> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.histogram.clone()
    }

    pub fn histogram_scale_modes(&self) -> (ScaleMode, ScaleMode) {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.histogram_scale_modes
    }

    pub fn set_histogram_scale_modes(&self, x_mode: ScaleMode, y_mode: ScaleMode) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.histogram_scale_modes = (x_mode, y_mode);
            guard.pending.histogram_scale_modes = Some((x_mode, y_mode));
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn last_size(&self) -> Option<(u32, u32)> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.last_size
    }

    pub fn demosaic(&self) -> bool {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.demosaic
    }

    pub fn set_image(&self, image: RawImage) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.image_size = Some((image.width, image.height));
            guard.pending.image = Some(Arc::new(image));
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn image_size(&self) -> Option<(u32, u32)> {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.image_size
    }

    pub fn load_from_memory(&self, buffer: &[u8]) -> Result<(), RawError> {
        let image = RawImage::decode_from_memory(buffer)?;
        self.set_image(image);
        Ok(())
    }

    pub fn set_demosaic(&self, enabled: bool) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.demosaic = enabled;
            guard.pending.demosaic = Some(enabled);
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn toggle_demosaic(&self) {
        let current = self.demosaic();
        self.set_demosaic(!current);
    }

    pub fn display_range(&self) -> (f32, f32) {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (guard.display_range.min, guard.display_range.max)
    }

    pub fn range(&self) -> DisplayRange {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.display_range
    }

    pub fn set_display_range(&self, min: f32, max: f32) {
        self.set_range(DisplayRange::new(min, max));
    }

    pub fn set_range(&self, range: DisplayRange) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.display_range = range;
            guard.pending.range = Some(range);
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn reset_display_range(&self) {
        self.set_range(DisplayRange::DEFAULT);
    }

    pub fn pan_zoom(&self) -> (Vec2, f32) {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (guard.view_transform.pan, guard.view_transform.zoom)
    }

    pub fn view_transform(&self) -> ViewTransform {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.view_transform
    }

    pub fn set_pan_zoom(&self, pan: Vec2, zoom: f32) {
        self.set_view_transform(ViewTransform::new(pan, zoom));
    }

    pub fn set_view_transform(&self, transform: ViewTransform) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.view_transform = transform;
            guard.pending.view_transform = Some(transform);
            guard.is_dirty = true;
        }
        self.inner.cvar.notify_one();
    }

    pub fn reset_view(&self) {
        self.set_view_transform(ViewTransform::DEFAULT);
    }

    pub fn resize(&self, width: u32, height: u32) {
        {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if guard.last_size != Some((width, height)) {
                guard.pending.resize = Some((width, height));
                guard.last_size = Some((width, height));
                guard.is_dirty = true;
            }
        }
        self.inner.cvar.notify_one();
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.inner.cvar.notify_all();
    }

    pub fn spawn_renderer(
        &self,
        device: wgpu::Device,
        queue: wgpu::Queue,
        initial_width: u32,
        initial_height: u32,
    ) -> Result<JoinHandle<()>, std::io::Error> {
        let bridge = self.clone();
        std::thread::Builder::new()
            .name("Raw viewer renderer".to_string())
            .spawn(move || {
                let mut renderer = RawViewerRenderer::new(device, queue);
                let mut current_size = (initial_width.max(1), initial_height.max(1));

                {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    guard.last_size = Some(current_size);
                }

                let initial_view = renderer.render(current_size.0, current_size.1);
                {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    guard.latest_texture = Some(initial_view);
                }
                (bridge.inner.notify_ui_fn)();

                while !bridge.inner.shutdown.load(Ordering::Acquire) {
                    let mut guard = bridge
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);

                    if !guard.is_dirty && !bridge.inner.shutdown.load(Ordering::Acquire) {
                        guard = bridge
                            .inner
                            .cvar
                            .wait_while(guard, |s| {
                                !s.is_dirty && !bridge.inner.shutdown.load(Ordering::Acquire)
                            })
                            .unwrap_or_else(PoisonError::into_inner);
                    }

                    if bridge.inner.shutdown.load(Ordering::Acquire) {
                        break;
                    }

                    guard.is_dirty = false;
                    let pending = std::mem::take(&mut guard.pending);
                    drop(guard);

                    if let Some(image) = pending.image {
                        let (x_mode, y_mode) = {
                            let guard = bridge
                                .inner
                                .state
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            guard.histogram_scale_modes
                        };
                        let hist = renderer.set_image(
                            image,
                            x_mode == ScaleMode::Log,
                            y_mode == ScaleMode::Log,
                        );
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        guard.histogram = hist.map(Arc::new);
                    } else if let Some((x_mode, y_mode)) = pending.histogram_scale_modes {
                        let hist = renderer
                            .compute_histogram(x_mode == ScaleMode::Log, y_mode == ScaleMode::Log);
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        guard.histogram = hist.map(Arc::new);
                    }

                    if let Some(demosaic) = pending.demosaic {
                        renderer.set_demosaic(demosaic);
                    }

                    if let Some(range) = pending.range {
                        renderer.set_range(range);
                    }

                    if let Some(transform) = pending.view_transform {
                        renderer.set_view_transform(transform);
                    }

                    if let Some(size) = pending.resize {
                        current_size = size;
                    }

                    let texture_view = renderer.render(current_size.0, current_size.1);

                    {
                        let mut guard = bridge
                            .inner
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        guard.latest_texture = Some(texture_view);
                    }

                    (bridge.inner.notify_ui_fn)();
                }

                let mut guard = bridge
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                guard.latest_texture = None;
            })
    }
}
