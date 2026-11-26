use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub use ::hips::{HipsGrid, HipsGridConfig, HipsSurvey, HipsTileFetcher, TileFetchError};
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::{
        mesh::Mesh,
        render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
    },
};
use glam::{DVec3, Vec3};

use crate::{
    BevyBridge, ViewportDimensions,
    camera::PlanetariumCamera,
    grids::ATTRIBUTE_LINE_TIER,
    material::{
        CelestialLineMaterial, CelestialLineUniforms, CelestialTileMaterial, CelestialTileUniforms,
    },
};

const MAX_DESCENDANT_DEPTH_DELTA: u8 = 3;
const MAX_DESCENDANT_FALLBACK_TILES: usize = 64;
const MAX_CACHED_SURVEY_TILES: usize = 1024;
const TARGET_CACHED_SURVEY_TILES: usize = 768;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HipsTile {
    pub depth: u8,
    pub hash: u64,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HipsSurveyTile {
    pub depth: u8,
    pub hash: u64,
}

#[derive(Resource, Default)]
pub struct HipsMeshCache {
    pub meshes: HashMap<(u8, u64), Handle<Mesh>>,
}

#[derive(Resource, Default)]
pub struct HipsSurfaceMeshCache {
    pub meshes: HashMap<(u8, u64), Handle<Mesh>>,
    last_used: HashMap<(u8, u64), u64>,
    tick: u64,
}

#[derive(Resource, Default)]
pub struct HipsImageCache {
    pub images: HashMap<(u8, u64), Handle<Image>>,
    last_used: HashMap<(u8, u64), u64>,
    tick: u64,
}

#[derive(Resource, Default)]
pub struct HipsActiveTiles {
    pub tiles: HashMap<(u8, u64), Entity>,
    pub current_depth: Option<u8>,
}

#[derive(Resource, Default)]
pub struct HipsActiveSurveyTiles {
    pub tiles: HashMap<(u8, u64), Entity>,
    pub current_depth: Option<u8>,
    pub current_survey_name: Option<String>,
}

#[derive(Resource)]
pub struct HipsGridSettings {
    pub visible: bool,
    pub color: Vec4,
    pub config: HipsGridConfig,
}

impl Default for HipsGridSettings {
    fn default() -> Self {
        Self {
            visible: false,
            color: Vec4::new(0.2, 0.9, 0.4, 0.5),
            config: HipsGridConfig::default(),
        }
    }
}

#[derive(Resource)]
pub struct HipsSurveySettings {
    pub enabled: bool,
    pub survey: HipsSurvey,
    pub alpha: f32,
    pub max_depth_override: Option<u8>,
}

impl Default for HipsSurveySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            survey: HipsSurvey::dss_color(),
            alpha: 1.0,
            max_depth_override: None,
        }
    }
}

pub struct LoadedTile {
    pub generation: u64,
    pub depth: u8,
    pub hash: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub enum TileLoadResult {
    Success(LoadedTile),
    Failed {
        generation: u64,
        depth: u8,
        hash: u64,
        is_not_found: bool,
    },
}

type TileRequest = (u64, Arc<HipsSurvey>, u8, u64);

#[derive(Resource)]
pub struct HipsTileLoader {
    pending_queue: Arc<Mutex<Vec<TileRequest>>>,
    notify: Arc<tokio::sync::Notify>,
    shutdown: Arc<AtomicBool>,
    result_rx: crossbeam_channel::Receiver<TileLoadResult>,
    in_flight: HashSet<(u8, u64)>,
    failed_tiles: HashMap<(u8, u64), Option<Instant>>,
    generation: u64,
}

impl Drop for HipsTileLoader {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

impl HipsTileLoader {
    pub fn new(bridge: Option<BevyBridge>) -> Self {
        let pending_queue = Arc::new(Mutex::new(Vec::<TileRequest>::new()));
        let notify = Arc::new(tokio::sync::Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let (res_tx, result_rx) = crossbeam_channel::unbounded::<TileLoadResult>();

        let worker_queue = Arc::clone(&pending_queue);
        let worker_notify = Arc::clone(&notify);
        let worker_shutdown = Arc::clone(&shutdown);

        std::thread::Builder::new()
            .name("hips-tile-fetcher".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    tracing::error!("Failed to initialize tokio runtime for hips fetcher");
                    return;
                };

                rt.block_on(async move {
                    let fetcher = HipsTileFetcher::new(HipsTileFetcher::default_cache_root());
                    let sem = Arc::new(tokio::sync::Semaphore::new(8));

                    loop {
                        if worker_shutdown.load(Ordering::Acquire) {
                            break;
                        }
                        let Ok(permit) = sem.clone().acquire_owned().await else {
                            break;
                        };
                        let next = {
                            let mut q = worker_queue.lock().unwrap_or_else(|e| e.into_inner());
                            q.pop()
                        };
                        let Some((generation, survey, depth, hash)) = next else {
                            drop(permit);
                            if worker_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            worker_notify.notified().await;
                            continue;
                        };
                        let fetcher = fetcher.clone();
                        let res_tx = res_tx.clone();
                        let bridge = bridge.clone();

                        tokio::spawn(async move {
                            let _permit = permit;
                            match fetcher.fetch_tile(&survey, depth, hash).await {
                                Ok(bytes) => {
                                    let decode_res = tokio::task::spawn_blocking(move || {
                                        let img = image::load_from_memory(&bytes).ok()?;
                                        let rgba = img.to_rgba8();
                                        let (width, height) = rgba.dimensions();
                                        Some((width, height, rgba.into_raw()))
                                    })
                                    .await;

                                    if let Ok(Some((width, height, rgba))) = decode_res {
                                        let _ = res_tx.send(TileLoadResult::Success(LoadedTile {
                                            generation,
                                            depth,
                                            hash,
                                            width,
                                            height,
                                            rgba,
                                        }));
                                        if let Some(ref b) = bridge {
                                            b.request_frame();
                                        }
                                    } else {
                                        let _ = res_tx.send(TileLoadResult::Failed {
                                            generation,
                                            depth,
                                            hash,
                                            is_not_found: false,
                                        });
                                    }
                                }
                                Err(err) => {
                                    let is_not_found = matches!(err, TileFetchError::NotFound(_));
                                    tracing::debug!(
                                        "Failed to fetch tile ({depth}, {hash}): {err}"
                                    );
                                    let _ = res_tx.send(TileLoadResult::Failed {
                                        generation,
                                        depth,
                                        hash,
                                        is_not_found,
                                    });
                                }
                            }
                        });
                    }
                });
            })
            .expect("Failed to spawn hips-tile-fetcher thread");

        Self {
            pending_queue,
            notify,
            shutdown,
            result_rx,
            in_flight: HashSet::new(),
            failed_tiles: HashMap::new(),
            generation: 0,
        }
    }

    fn clear_pending(&mut self) {
        let mut q = self.pending_queue.lock().unwrap_or_else(|e| e.into_inner());
        for (_, _, depth, hash) in q.drain(..) {
            self.in_flight.remove(&(depth, hash));
        }
    }

    fn retain_pending(&mut self, survey_name: &str, keep: &HashSet<(u8, u64)>) {
        let generation = self.generation;
        let mut q = self.pending_queue.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|(req_gen, req_survey, d, h)| {
            let keep_item = *req_gen == generation
                && req_survey.name == survey_name
                && (*d == 0 || keep.contains(&(*d, *h)));
            if !keep_item {
                self.in_flight.remove(&(*d, *h));
            }
            keep_item
        });
    }

    fn is_in_failure_cooldown(&mut self, key: (u8, u64)) -> bool {
        match self.failed_tiles.get(&key) {
            None => false,
            Some(None) => true,
            Some(Some(until)) => {
                if Instant::now() < *until {
                    true
                } else {
                    self.failed_tiles.remove(&key);
                    false
                }
            }
        }
    }

    fn enqueue(&mut self, survey: &Arc<HipsSurvey>, keys: &[(u8, u64)]) {
        if keys.is_empty() {
            return;
        }
        let generation = self.generation;
        let mut pushed = false;
        {
            let mut q = self.pending_queue.lock().unwrap_or_else(|e| e.into_inner());
            for &(depth, hash) in keys {
                if self.in_flight.insert((depth, hash)) {
                    q.push((generation, Arc::clone(survey), depth, hash));
                    pushed = true;
                }
            }
        }
        if pushed {
            self.notify.notify_one();
        }
    }
}

pub(crate) fn create_tile_boundary_mesh(depth: u8, hash: u64) -> Mesh {
    let segments =
        HipsGrid::cell_boundary_segments(depth, hash, HipsGrid::default_segments_per_edge(depth));
    let mut positions = Vec::with_capacity(segments.len() * 2);
    let mut line_tiers = Vec::with_capacity(segments.len() * 2);

    for (start, end) in segments {
        positions.push(Vec3::new(start.x as f32, start.y as f32, start.z as f32));
        positions.push(Vec3::new(end.x as f32, end.y as f32, end.z as f32));
        line_tiers.push(0.0);
        line_tiers.push(0.0);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(ATTRIBUTE_LINE_TIER, line_tiers);
    mesh
}

pub(crate) fn create_tile_surface_mesh(depth: u8, hash: u64) -> Mesh {
    let subdivisions = HipsGrid::default_tile_subdivisions(depth);
    let geom = HipsGrid::tile_surface_mesh(depth, hash, subdivisions);

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, geom.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, geom.uvs);
    mesh.insert_indices(Indices::U32(geom.indices));
    mesh
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn hips_grid_system(
    mut commands: Commands,
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    settings: Res<HipsGridSettings>,
    mut active_tiles: ResMut<HipsActiveTiles>,
    mut mesh_cache: ResMut<HipsMeshCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut line_materials: Query<&mut CelestialLineMaterial, With<HipsTile>>,
) {
    if !settings.visible {
        if !active_tiles.tiles.is_empty() {
            for entity in active_tiles.tiles.values() {
                commands.entity(*entity).despawn();
            }
            active_tiles.tiles.clear();
            active_tiles.current_depth = None;
        }
        return;
    }

    let Ok(camera) = camera_query.single() else {
        return;
    };

    let aspect = dims.width as f32 / dims.height as f32;
    let proj_params = camera.projection_params(aspect);
    let transform = camera.icrs_to_camera_mat4();

    let target_depth = HipsGrid::target_depth(
        camera.fov_y,
        dims.height,
        active_tiles.current_depth,
        &settings.config,
    );
    active_tiles.current_depth = Some(target_depth);

    let center_icrs = camera.forward_direction_eq();
    let theta_y = camera.fov_y * 0.5;
    let half_diag = (theta_y * theta_y + (theta_y * aspect as f64).powi(2)).sqrt() * 1.1;

    let visible_cells = HipsGrid::query_visible_cells(center_icrs, half_diag, target_depth);
    let desired_set: HashSet<(u8, u64)> = visible_cells
        .into_iter()
        .map(|h| (target_depth, h))
        .collect();

    // Despawn tiles that are no longer in view
    let mut to_remove = Vec::new();
    for (&key, &entity) in &active_tiles.tiles {
        if !desired_set.contains(&key) {
            commands.entity(entity).despawn();
            to_remove.push(key);
        }
    }
    for key in to_remove {
        active_tiles.tiles.remove(&key);
    }

    // Spawn newly entered tiles
    for key in desired_set {
        if active_tiles.tiles.contains_key(&key) {
            continue;
        }

        let (depth, hash) = key;
        let mesh_handle = mesh_cache
            .meshes
            .entry(key)
            .or_insert_with(|| {
                let mesh = create_tile_boundary_mesh(depth, hash);
                meshes.add(mesh)
            })
            .clone();

        let material = CelestialLineMaterial {
            uniforms: CelestialLineUniforms {
                transform,
                projection: proj_params.into(),
                color: settings.color,
                minor_alpha: 1.0,
            },
        };

        let entity = commands
            .spawn((Mesh3d(mesh_handle), material, HipsTile { depth, hash }))
            .id();

        active_tiles.tiles.insert(key, entity);
    }

    // Update transforms and projections for existing tiles
    for &entity in active_tiles.tiles.values() {
        if let Ok(mut mat) = line_materials.get_mut(entity) {
            mat.uniforms.transform = transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.color = settings.color;
            mat.uniforms.minor_alpha = 1.0;
        }
    }

    // Cap cache size to avoid unbounded memory growth
    if mesh_cache.meshes.len() > 2048 {
        mesh_cache
            .meshes
            .retain(|key, _| active_tiles.tiles.contains_key(key));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DescendantFallbackLimits {
    max_depth_delta: u8,
    max_tiles: usize,
}

impl Default for DescendantFallbackLimits {
    fn default() -> Self {
        Self {
            max_depth_delta: MAX_DESCENDANT_DEPTH_DELTA,
            max_tiles: MAX_DESCENDANT_FALLBACK_TILES,
        }
    }
}

#[derive(Debug, Default)]
struct SurveyTileSelection {
    desired: HashSet<(u8, u64)>,
    missing_to_fetch: Vec<(u8, u64)>,
}

fn select_survey_tiles<V>(
    visible_cells: &[u64],
    target_depth: u8,
    max_survey_depth: u8,
    center_icrs: DVec3,
    half_diag_rad: f64,
    cached_images: &HashMap<(u8, u64), V>,
    limits: DescendantFallbackLimits,
) -> SurveyTileSelection {
    let mut desired = HashSet::new();
    let mut missing_hashes = HashSet::new();
    let mut missing_scored: Vec<(f64, u64)> = Vec::new();

    for &hash in visible_cells {
        let key = (target_depth, hash);
        if cached_images.contains_key(&key) {
            desired.insert(key);
        } else {
            if missing_hashes.insert(hash) {
                let dot = HipsGrid::cell_center_icrs(target_depth, hash).dot(center_icrs);
                missing_scored.push((dot, hash));
            }
            let mut fallback_depth = target_depth;
            let mut fallback_hash = hash;
            while fallback_depth > 0 {
                fallback_depth -= 1;
                fallback_hash >>= 2;
                let parent_key = (fallback_depth, fallback_hash);
                if cached_images.contains_key(&parent_key) {
                    desired.insert(parent_key);
                    break;
                }
            }
        }
    }

    // Push farthest-from-center missing tiles first so LIFO pending_queue.pop() fetches center first.
    missing_scored.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let missing_to_fetch = missing_scored
        .into_iter()
        .map(|(_, hash)| (target_depth, hash))
        .collect();

    let max_desc_depth = target_depth
        .saturating_add(limits.max_depth_delta)
        .min(max_survey_depth);
    if !missing_hashes.is_empty() && max_desc_depth > target_depth && limits.max_tiles > 0 {
        let mut descendant_candidates: Vec<(u8, f64, u64)> = Vec::new();
        for &(d, h) in cached_images.keys() {
            if d <= target_depth || d > max_desc_depth {
                continue;
            }
            let delta = u32::from(d - target_depth);
            if delta >= 32 {
                continue;
            }
            let target_anc = h >> (2 * delta);
            if !missing_hashes.contains(&target_anc) {
                continue;
            }
            let has_cached_intermediate = ((target_depth + 1)..d).any(|inter_d| {
                let shift = 2 * u32::from(d - inter_d);
                cached_images.contains_key(&(inter_d, h >> shift))
            });
            if has_cached_intermediate {
                continue;
            }
            let dot = HipsGrid::cell_center_icrs(d, h).dot(center_icrs);
            let min_dot = (half_diag_rad + HipsGrid::max_cell_radius_rad(d))
                .min(std::f64::consts::PI)
                .cos();
            if dot < min_dot {
                continue;
            }
            descendant_candidates.push((d, dot, h));
        }

        if descendant_candidates.len() > limits.max_tiles {
            // Prioritize shallower descendants (broader screen coverage) and tiles closer to view center.
            descendant_candidates.sort_unstable_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| b.1.total_cmp(&a.1))
                    .then_with(|| a.2.cmp(&b.2))
            });
            descendant_candidates.truncate(limits.max_tiles);
        }

        for (d, _, h) in descendant_candidates {
            desired.insert((d, h));
        }
    }

    SurveyTileSelection {
        desired,
        missing_to_fetch,
    }
}

fn evict_lru_cache<V>(
    entries: &mut HashMap<(u8, u64), V>,
    last_used: &mut HashMap<(u8, u64), u64>,
    active: &HashMap<(u8, u64), Entity>,
    max_len: usize,
    target_len: usize,
) {
    if entries.len() <= max_len {
        return;
    }
    let target = target_len.min(max_len);
    let mut candidates: Vec<(u64, (u8, u64))> = entries
        .keys()
        .copied()
        .filter(|key| key.0 != 0 && !active.contains_key(key))
        .map(|key| (last_used.get(&key).copied().unwrap_or(0), key))
        .collect();
    candidates.sort_unstable_by_key(|&(stamp, key)| (stamp, key));
    let to_remove = entries.len().saturating_sub(target).min(candidates.len());
    for (_, key) in candidates.into_iter().take(to_remove) {
        entries.remove(&key);
        last_used.remove(&key);
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn hips_survey_system(
    mut commands: Commands,
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    grid_settings: Res<HipsGridSettings>,
    settings: Res<HipsSurveySettings>,
    atmo_settings: Res<crate::atmosphere::AtmosphereSettings>,
    mut active_survey_tiles: ResMut<HipsActiveSurveyTiles>,
    mut mesh_cache: ResMut<HipsSurfaceMeshCache>,
    mut image_cache: ResMut<HipsImageCache>,
    mut loader: ResMut<HipsTileLoader>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut tile_materials: Query<&mut CelestialTileMaterial, With<HipsSurveyTile>>,
) {
    while let Ok(result) = loader.result_rx.try_recv() {
        match result {
            TileLoadResult::Success(tile) => {
                if tile.generation != loader.generation {
                    continue;
                }
                loader.in_flight.remove(&(tile.depth, tile.hash));
                loader.failed_tiles.remove(&(tile.depth, tile.hash));
                let image = Image::new(
                    Extent3d {
                        width: tile.width,
                        height: tile.height,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    tile.rgba,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::RENDER_WORLD,
                );
                let handle = images.add(image);
                let key = (tile.depth, tile.hash);
                image_cache.images.insert(key, handle);
                image_cache.tick += 1;
                let stamp = image_cache.tick;
                image_cache.last_used.insert(key, stamp);
            }
            TileLoadResult::Failed {
                generation,
                depth,
                hash,
                is_not_found,
            } => {
                if generation != loader.generation {
                    continue;
                }
                loader.in_flight.remove(&(depth, hash));
                let until = if is_not_found {
                    None
                } else {
                    Some(Instant::now() + Duration::from_secs(30))
                };
                loader.failed_tiles.insert((depth, hash), until);
            }
        }
    }

    if !settings.enabled {
        loader.clear_pending();
        if !active_survey_tiles.tiles.is_empty() {
            for entity in active_survey_tiles.tiles.values() {
                commands.entity(*entity).despawn();
            }
            active_survey_tiles.tiles.clear();
            active_survey_tiles.current_depth = None;
        }
        return;
    }

    let survey_name = &settings.survey.name;
    if active_survey_tiles.current_survey_name.as_deref() != Some(survey_name) {
        for entity in active_survey_tiles.tiles.values() {
            commands.entity(*entity).despawn();
        }
        active_survey_tiles.tiles.clear();
        image_cache.images.clear();
        image_cache.last_used.clear();
        image_cache.tick = 0;
        mesh_cache.meshes.clear();
        mesh_cache.last_used.clear();
        mesh_cache.tick = 0;
        loader.generation = loader.generation.wrapping_add(1);
        loader.clear_pending();
        loader.in_flight.clear();
        loader.failed_tiles.clear();
        active_survey_tiles.current_survey_name = Some(survey_name.clone());
        active_survey_tiles.current_depth = None;

        let survey_arc = Arc::new(settings.survey.clone());
        let base_keys: Vec<(u8, u64)> = (0..12).map(|h| (0, h)).collect();
        loader.enqueue(&survey_arc, &base_keys);
    }

    let Ok(camera) = camera_query.single() else {
        return;
    };

    let aspect = dims.width as f32 / dims.height as f32;
    let proj_params = camera.projection_params(aspect);
    let transform = camera.icrs_to_camera_mat4();
    let sun_alt_deg =
        celestial::solar::solar_altitude_deg(camera.observer.location, camera.observer.epoch)
            as f32;
    let effective_alpha = settings.alpha
        * crate::atmosphere::compute_hips_atmosphere_visibility(
            sun_alt_deg,
            atmo_settings.strength,
        );

    let mut target_depth = HipsGrid::target_depth(
        camera.fov_y,
        dims.height,
        active_survey_tiles.current_depth,
        &grid_settings.config,
    );
    if let Some(max_override) = settings.max_depth_override {
        target_depth = target_depth.min(max_override);
    }
    target_depth = target_depth.min(settings.survey.max_depth);
    active_survey_tiles.current_depth = Some(target_depth);

    let center_icrs = camera.forward_direction_eq();
    let theta_y = camera.fov_y * 0.5;
    let half_diag = (theta_y * theta_y + (theta_y * aspect as f64).powi(2)).sqrt() * 1.1;

    let survey_arc = Arc::new(settings.survey.clone());

    let visible_cells = HipsGrid::query_visible_cells(center_icrs, half_diag, target_depth);
    let visible_keys: HashSet<(u8, u64)> = visible_cells
        .iter()
        .map(|&hash| (target_depth, hash))
        .collect();
    loader.retain_pending(&settings.survey.name, &visible_keys);

    let selection = select_survey_tiles(
        &visible_cells,
        target_depth,
        settings.survey.max_depth,
        center_icrs,
        half_diag,
        &image_cache.images,
        DescendantFallbackLimits::default(),
    );

    let missing_keys: Vec<(u8, u64)> = selection
        .missing_to_fetch
        .into_iter()
        .filter(|&key| !loader.in_flight.contains(&key) && !loader.is_in_failure_cooldown(key))
        .collect();
    loader.enqueue(&survey_arc, &missing_keys);

    let mut to_remove = Vec::new();
    for (&key, &entity) in &active_survey_tiles.tiles {
        if !selection.desired.contains(&key) {
            commands.entity(entity).despawn();
            to_remove.push(key);
        }
    }
    for key in to_remove {
        active_survey_tiles.tiles.remove(&key);
    }

    for key in selection.desired {
        if active_survey_tiles.tiles.contains_key(&key) {
            continue;
        }

        let (depth, hash) = key;
        let Some(image_handle) = image_cache.images.get(&key).cloned() else {
            continue;
        };

        let mesh_handle = mesh_cache
            .meshes
            .entry(key)
            .or_insert_with(|| {
                let mesh = create_tile_surface_mesh(depth, hash);
                meshes.add(mesh)
            })
            .clone();

        let material = CelestialTileMaterial {
            uniforms: CelestialTileUniforms {
                transform,
                projection: proj_params.into(),
                alpha: effective_alpha,
            },
            image: image_handle,
            depth,
        };

        let entity = commands
            .spawn((
                Mesh3d(mesh_handle),
                material,
                HipsSurveyTile { depth, hash },
            ))
            .id();

        active_survey_tiles.tiles.insert(key, entity);
    }

    if !active_survey_tiles.tiles.is_empty() {
        image_cache.tick += 1;
        let img_stamp = image_cache.tick;
        mesh_cache.tick += 1;
        let mesh_stamp = mesh_cache.tick;
        for &key in active_survey_tiles.tiles.keys() {
            image_cache.last_used.insert(key, img_stamp);
            mesh_cache.last_used.insert(key, mesh_stamp);
        }
    }

    for &entity in active_survey_tiles.tiles.values() {
        if let Ok(mut mat) = tile_materials.get_mut(entity) {
            mat.uniforms.transform = transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.alpha = effective_alpha;
        }
    }

    let mc = &mut *mesh_cache;
    evict_lru_cache(
        &mut mc.meshes,
        &mut mc.last_used,
        &active_survey_tiles.tiles,
        MAX_CACHED_SURVEY_TILES,
        TARGET_CACHED_SURVEY_TILES,
    );
    let ic = &mut *image_cache;
    evict_lru_cache(
        &mut ic.images,
        &mut ic.last_used,
        &active_survey_tiles.tiles,
        MAX_CACHED_SURVEY_TILES,
        TARGET_CACHED_SURVEY_TILES,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_cache(keys: &[(u8, u64)]) -> HashMap<(u8, u64), u8> {
        keys.iter().map(|&k| (k, 1)).collect()
    }

    #[test]
    fn missing_target_tile_uses_ancestor_and_cached_descendants() {
        let center = DVec3::X;
        let visible = HipsGrid::query_visible_cells(center, 0.05, 4);
        let h4 = visible[0];
        let child0 = h4 << 2;
        let child1 = (h4 << 2) | 1;
        let cache = fake_cache(&[(0, h4 >> 8), (5, child0), (5, child1)]);

        let sel = select_survey_tiles(
            &[h4],
            4,
            9,
            center,
            0.25,
            &cache,
            DescendantFallbackLimits::default(),
        );
        assert!(sel.desired.contains(&(0, h4 >> 8)));
        assert!(sel.desired.contains(&(5, child0)));
        assert!(sel.desired.contains(&(5, child1)));
        assert!(!sel.desired.contains(&(4, h4)));
        assert_eq!(sel.missing_to_fetch, vec![(4, h4)]);
    }

    #[test]
    fn loaded_target_tile_suppresses_cached_descendants() {
        let center = DVec3::X;
        let visible = HipsGrid::query_visible_cells(center, 0.05, 4);
        let h4 = visible[0];
        let cache = fake_cache(&[(0, h4 >> 8), (4, h4), (5, h4 << 2), (5, (h4 << 2) | 1)]);

        let sel = select_survey_tiles(
            &[h4],
            4,
            9,
            center,
            0.25,
            &cache,
            DescendantFallbackLimits::default(),
        );
        assert_eq!(sel.desired, HashSet::from([(4, h4)]));
        assert_eq!(sel.missing_to_fetch, [] as [(u8, u64); 0]);
    }

    #[test]
    fn shallower_descendant_suppresses_redundant_grandchildren() {
        let center = DVec3::X;
        let visible = HipsGrid::query_visible_cells(center, 0.05, 4);
        let h4 = visible[0];
        let c5_covered = h4 << 2;
        let g6_covered = (c5_covered << 2) | 2;
        let c5_uncovered = (h4 << 2) | 1;
        let g6_uncovered = (c5_uncovered << 2) | 1;

        let cache = fake_cache(&[
            (0, h4 >> 8),
            (5, c5_covered),
            (6, g6_covered),
            (6, g6_uncovered),
        ]);

        let sel = select_survey_tiles(
            &[h4],
            4,
            9,
            center,
            0.25,
            &cache,
            DescendantFallbackLimits::default(),
        );
        assert!(sel.desired.contains(&(0, h4 >> 8)));
        assert!(sel.desired.contains(&(5, c5_covered)));
        assert!(sel.desired.contains(&(6, g6_uncovered)));
        assert!(!sel.desired.contains(&(6, g6_covered)));
    }

    #[test]
    fn descendant_fallback_respects_depth_delta_and_tile_count_limits() {
        let center = DVec3::X;
        let visible = HipsGrid::query_visible_cells(center, 0.05, 4);
        let h4 = visible[0];

        // 1. Depth delta limit (depth 4 + max_depth_delta 3 = max depth 7; depth 8 ignored).
        let d7 = h4 << 6;
        let d8 = ((h4 << 6) | 1) << 2;
        let cache_depth = fake_cache(&[(0, h4 >> 8), (7, d7), (8, d8)]);

        let sel_depth = select_survey_tiles(
            &[h4],
            4,
            9,
            center,
            0.5,
            &cache_depth,
            DescendantFallbackLimits::default(),
        );
        assert!(sel_depth.desired.contains(&(7, d7)));
        assert!(!sel_depth.desired.contains(&(8, d8)));

        // 2. Tile count budget prioritizes shallower depth first, then closer to view center.
        let d5_tile = (h4 << 2) | 3;
        let mut cache_budget = fake_cache(&[(0, h4 >> 8), (5, d5_tile)]);

        let mut d6_tiles = [h4 << 4, ((h4 << 2) | 1) << 2, ((h4 << 2) | 2) << 2];
        d6_tiles.sort_unstable_by(|&a, &b| {
            let da = HipsGrid::cell_center_icrs(6, a).dot(center);
            let db = HipsGrid::cell_center_icrs(6, b).dot(center);
            db.total_cmp(&da)
        });
        for &h6 in &d6_tiles {
            cache_budget.insert((6, h6), 1);
        }

        let sel_budget = select_survey_tiles(
            &[h4],
            4,
            9,
            center,
            0.5,
            &cache_budget,
            DescendantFallbackLimits {
                max_depth_delta: 3,
                max_tiles: 2,
            },
        );
        assert!(sel_budget.desired.contains(&(0, h4 >> 8)));
        assert!(sel_budget.desired.contains(&(5, d5_tile)));
        assert!(sel_budget.desired.contains(&(6, d6_tiles[0])));
        assert!(!sel_budget.desired.contains(&(6, d6_tiles[1])));
        assert!(!sel_budget.desired.contains(&(6, d6_tiles[2])));

        // 3. Off-screen descendants inside a large visible missing ancestor are culled by view cone.
        let half_diag = 0.01_f64;
        let visible_d1 = HipsGrid::query_visible_cells(center, half_diag, 1);
        let h1 = visible_d1[0];
        let mut d4_children: Vec<u64> = (0..64).map(|sub| (h1 << 6) | sub).collect();
        d4_children.sort_unstable_by(|&a, &b| {
            let da = HipsGrid::cell_center_icrs(4, a).dot(center);
            let db = HipsGrid::cell_center_icrs(4, b).dot(center);
            db.total_cmp(&da)
        });
        let near_d4 = d4_children[0];
        let far_d4 = *d4_children.last().expect("non-empty depth-4 children");
        assert!(
            HipsGrid::cell_center_icrs(4, far_d4).dot(center)
                < (half_diag + HipsGrid::max_cell_radius_rad(4)).cos()
        );

        let cache_cone = fake_cache(&[(0, h1 >> 2), (4, near_d4), (4, far_d4)]);

        let sel_cone = select_survey_tiles(
            &[h1],
            1,
            9,
            center,
            half_diag,
            &cache_cone,
            DescendantFallbackLimits::default(),
        );
        assert!(sel_cone.desired.contains(&(4, near_d4)));
        assert!(!sel_cone.desired.contains(&(4, far_d4)));
    }

    #[test]
    fn missing_tiles_prioritize_center_for_lifo_queue() {
        let center = DVec3::X;
        let visible = HipsGrid::query_visible_cells(center, 0.25, 4);
        assert!(visible.len() >= 4);

        let empty_cache: HashMap<(u8, u64), u8> = HashMap::new();
        let sel = select_survey_tiles(
            &visible,
            4,
            9,
            center,
            0.25,
            &empty_cache,
            DescendantFallbackLimits::default(),
        );
        assert_eq!(sel.missing_to_fetch.len(), visible.len());

        let dots: Vec<f64> = sel
            .missing_to_fetch
            .iter()
            .map(|&(d, h)| HipsGrid::cell_center_icrs(d, h).dot(center))
            .collect();
        for pair in dots.windows(2) {
            assert!(
                pair[0] <= pair[1] + 1e-12,
                "missing tiles must be ordered farthest-first for LIFO pop: {} > {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn lru_eviction_preserves_depth_zero_active_and_most_recent_tiles() {
        let mut entries = fake_cache(&[(0, 3), (4, 10), (4, 11), (5, 20), (5, 21)]);
        let mut last_used = HashMap::from([
            ((0_u8, 3_u64), 1_u64),
            ((4, 10), 2),
            ((4, 11), 3),
            ((5, 20), 5),
            ((5, 21), 10),
        ]);
        let active = HashMap::from([((4_u8, 10_u64), Entity::PLACEHOLDER)]);

        evict_lru_cache(&mut entries, &mut last_used, &active, 3, 3);

        assert_eq!(entries.len(), 3);
        assert!(entries.contains_key(&(0, 3)));
        assert!(entries.contains_key(&(4, 10)));
        assert!(entries.contains_key(&(5, 21)));
        assert!(!entries.contains_key(&(4, 11)));
        assert!(!entries.contains_key(&(5, 20)));
        assert!(!last_used.contains_key(&(4, 11)));
        assert!(!last_used.contains_key(&(5, 20)));
    }
}
