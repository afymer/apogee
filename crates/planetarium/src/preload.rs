use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    thread::JoinHandle,
};

use crossbeam_channel::Receiver;

use crate::{BevyBridge, dso, hips, solar_system, stars};

/// Thread-safe optional bridge wake handle shared between preloading workers and `BevyBridge`.
#[derive(Default)]
pub(crate) struct BridgeWake {
    bridge: Mutex<Option<BevyBridge>>,
}

impl BridgeWake {
    pub(crate) fn new(bridge: Option<BevyBridge>) -> Self {
        Self {
            bridge: Mutex::new(bridge),
        }
    }

    pub(crate) fn attach(&self, bridge: BevyBridge) {
        let mut guard = self.bridge.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = Some(bridge);
    }

    pub(crate) fn request_frame(&self) {
        let guard = self.bridge.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(bridge) = guard.as_ref() {
            bridge.request_frame();
        }
    }
}

/// Star catalog records and spatial index decoded on a startup background thread.
pub(crate) struct PreloadedStarCatalog {
    pub records: Vec<celestial::catalog::StarRecord>,
    pub index: Arc<stars::StarSpatialIndex>,
}

/// Deep-sky catalog GPU records and spatial index decoded on a startup background thread.
pub(crate) struct PreloadedDsoCatalog {
    pub gpu_records: Vec<dso::DsoGpuRecord>,
    pub index: Arc<dso::DsoSpatialIndex>,
}

/// Single decoded cached HiPS depth-0 tile ready for GPU upload.
pub struct PreloadedHipsTile {
    pub depth: u8,
    pub hash: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Locally cached depth-0 HiPS tiles loaded off the main thread without network I/O.
pub struct PreloadedHipsBase {
    pub survey_name: String,
    pub tiles: Vec<PreloadedHipsTile>,
}

/// In-flight startup preload handles overlapped with GPUI window and UI initialization.
pub(crate) struct PreloadState {
    stars_handle: Option<JoinHandle<Option<PreloadedStarCatalog>>>,
    dso_handle: Option<JoinHandle<Option<PreloadedDsoCatalog>>>,
    solar_system_loader: Option<solar_system::SolarSystemTextureLoader>,
    hips_rx: Option<Receiver<PreloadedHipsBase>>,
    hips_wake: Arc<BridgeWake>,
}

impl PreloadState {
    fn spawn() -> Self {
        let stars_handle = std::thread::Builder::new()
            .name("planetarium-preload-stars".to_string())
            .spawn(|| {
                let _span = tracing::info_span!("preload_star_catalog").entered();
                let catalog = stars::load_star_catalog()?;
                let index = Arc::new(stars::StarSpatialIndex::build_with_metadata(
                    &catalog.records,
                    catalog.metadata,
                ));
                Some(PreloadedStarCatalog {
                    records: catalog.records,
                    index,
                })
            })
            .ok();

        let dso_handle = std::thread::Builder::new()
            .name("planetarium-preload-dso".to_string())
            .spawn(|| {
                let _span = tracing::info_span!("preload_dso_catalog").entered();
                let catalog = dso::load_dso_catalog()?;
                let gpu_records: Vec<dso::DsoGpuRecord> = catalog
                    .records
                    .iter()
                    .map(dso::DsoGpuRecord::from_record)
                    .collect();
                let index = Arc::new(dso::DsoSpatialIndex::build(catalog.records));
                Some(PreloadedDsoCatalog { gpu_records, index })
            })
            .ok();

        let solar_system_loader = Some(solar_system::SolarSystemTextureLoader::spawn_preloading());

        let hips_wake = Arc::new(BridgeWake::new(None));
        let hips_wake_worker = Arc::clone(&hips_wake);
        let (hips_tx, hips_rx) = crossbeam_channel::bounded(1);
        let hips_spawned = std::thread::Builder::new()
            .name("planetarium-preload-hips".to_string())
            .spawn(move || {
                let _span = tracing::info_span!("preload_hips_base_tiles").entered();
                let survey = hips::HipsSurvey::dss_color();
                let cache_root = hips::HipsTileFetcher::default_cache_root();
                let base = load_cached_hips_base_tiles(cache_root.as_deref(), &survey);
                if hips_tx.send(base).is_ok() {
                    hips_wake_worker.request_frame();
                }
            })
            .is_ok();

        Self {
            stars_handle,
            dso_handle,
            solar_system_loader,
            hips_rx: hips_spawned.then_some(hips_rx),
            hips_wake,
        }
    }

    pub(crate) fn take_stars(&mut self) -> Option<PreloadedStarCatalog> {
        self.stars_handle.take()?.join().ok().flatten()
    }

    pub(crate) fn take_dso(&mut self) -> Option<PreloadedDsoCatalog> {
        self.dso_handle.take()?.join().ok().flatten()
    }

    pub(crate) fn take_solar_system_loader(
        &mut self,
    ) -> Option<solar_system::SolarSystemTextureLoader> {
        self.solar_system_loader.take()
    }

    pub(crate) fn take_hips(&mut self, bridge: BevyBridge) -> Option<Receiver<PreloadedHipsBase>> {
        // Attach the bridge before callers poll `try_recv()` so completion notifications cannot
        // slip through a race between an Empty poll and bridge registration.
        self.hips_wake.attach(bridge);
        self.hips_rx.take()
    }
}

static PRELOAD_SLOT: OnceLock<Mutex<Option<PreloadState>>> = OnceLock::new();

/// Starts background preloading of planetarium catalogs, planetary textures, and cached depth-0
/// HiPS tiles without blocking the caller thread. Safe to call multiple times (only the first
/// invocation spawns workers).
pub fn start_preload() {
    let _ = PRELOAD_SLOT.get_or_init(|| Mutex::new(Some(PreloadState::spawn())));
}

/// Claims the preloaded startup handles once for [`crate::PlanetariumRenderer::spawn`].
pub(crate) fn take_preload() -> Option<PreloadState> {
    let slot = PRELOAD_SLOT.get()?;
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Reads and decodes already-cached depth-0 HiPS tiles from `cache_root` without performing any
/// network requests. Missing directories or uncached tiles are skipped cleanly.
pub fn load_cached_hips_base_tiles(
    cache_root: Option<&Path>,
    survey: &hips::HipsSurvey,
) -> PreloadedHipsBase {
    let mut tiles = Vec::new();
    let Some(root) = cache_root else {
        return PreloadedHipsBase {
            survey_name: survey.name.clone(),
            tiles,
        };
    };

    let survey_dir = root.join(survey.slug());
    let paths: Vec<(u64, PathBuf)> = (0..12_u64)
        .filter_map(|hash| {
            let path = survey_dir.join(survey.tile_relative_path(0, hash));
            path.exists().then_some((hash, path))
        })
        .collect();

    if !paths.is_empty() {
        let decoded: Mutex<Vec<PreloadedHipsTile>> = Mutex::new(Vec::with_capacity(paths.len()));
        let chunk_size = paths.len().div_ceil(4).max(1);
        std::thread::scope(|scope| {
            for chunk in paths.chunks(chunk_size) {
                let decoded = &decoded;
                scope.spawn(move || {
                    for (hash, path) in chunk {
                        let Ok(bytes) = std::fs::read(path) else {
                            continue;
                        };
                        let Ok(img) = image::load_from_memory(&bytes) else {
                            continue;
                        };
                        let rgba = img.to_rgba8();
                        let (width, height) = rgba.dimensions();
                        if let Ok(mut guard) = decoded.lock() {
                            guard.push(PreloadedHipsTile {
                                depth: 0,
                                hash: *hash,
                                width,
                                height,
                                rgba: rgba.into_raw(),
                            });
                        }
                    }
                });
            }
        });
        tiles = decoded.into_inner().unwrap_or_else(PoisonError::into_inner);
        tiles.sort_unstable_by_key(|tile| tile.hash);
    }

    PreloadedHipsBase {
        survey_name: survey.name.clone(),
        tiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_cached_hips_base_tiles_handles_missing_cache() {
        let survey = hips::HipsSurvey::dss_color();

        let no_root = load_cached_hips_base_tiles(None, &survey);
        assert_eq!(no_root.survey_name, survey.name);
        assert!(no_root.tiles.is_empty());

        let missing = std::env::temp_dir().join("apogee-missing-hips-cache-dir-test");
        let _ = std::fs::remove_dir_all(&missing);
        let empty = load_cached_hips_base_tiles(Some(&missing), &survey);
        assert_eq!(empty.survey_name, survey.name);
        assert!(empty.tiles.is_empty());
    }
}
