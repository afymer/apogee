use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub use ::hips::{HipsGrid, HipsGridConfig, HipsSurvey, HipsTileFetcher};
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::{
        mesh::Mesh,
        render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
    },
};
use glam::Vec3;

use crate::{
    BevyBridge, ViewportDimensions,
    camera::PlanetariumCamera,
    grids::ATTRIBUTE_LINE_TIER,
    material::{
        CelestialLineMaterial, CelestialLineUniforms, CelestialTileMaterial, CelestialTileUniforms,
    },
};

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
}

#[derive(Resource, Default)]
pub struct HipsImageCache {
    pub images: HashMap<(u8, u64), Handle<Image>>,
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
            visible: true,
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
    pub depth: u8,
    pub hash: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub enum TileLoadResult {
    Success(LoadedTile),
    Failed { depth: u8, hash: u64 },
}

#[derive(Resource)]
pub struct HipsTileLoader {
    request_tx: tokio::sync::mpsc::UnboundedSender<(Arc<HipsSurvey>, u8, u64)>,
    result_rx: crossbeam_channel::Receiver<TileLoadResult>,
    in_flight: HashSet<(u8, u64)>,
}

impl HipsTileLoader {
    pub fn new(bridge: Option<BevyBridge>) -> Self {
        let (request_tx, mut req_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Arc<HipsSurvey>, u8, u64)>();
        let (res_tx, result_rx) = crossbeam_channel::unbounded::<TileLoadResult>();

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
                    let fetcher =
                        HipsTileFetcher::new(HipsTileFetcher::default_cache_root());
                    let sem = Arc::new(tokio::sync::Semaphore::new(8));

                    while let Some((survey, depth, hash)) = req_rx.recv().await {
                        let Ok(permit) = sem.clone().acquire_owned().await else {
                            break;
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
                                        let _ = res_tx.send(TileLoadResult::Failed { depth, hash });
                                    }
                                }
                                Err(err) => {
                                    tracing::debug!(
                                        "Failed to fetch tile ({depth}, {hash}): {err}"
                                    );
                                    let _ = res_tx.send(TileLoadResult::Failed { depth, hash });
                                }
                            }
                        });
                    }
                });
            })
            .expect("Failed to spawn hips-tile-fetcher thread");

        Self {
            request_tx,
            result_rx,
            in_flight: HashSet::new(),
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

    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(ATTRIBUTE_LINE_TIER, line_tiers);
    mesh
}

pub(crate) fn create_tile_surface_mesh(depth: u8, hash: u64) -> Mesh {
    let subdivisions = HipsGrid::default_tile_subdivisions(depth);
    let geom = HipsGrid::tile_surface_mesh(depth, hash, subdivisions);

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
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

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn hips_survey_system(
    mut commands: Commands,
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    grid_settings: Res<HipsGridSettings>,
    settings: Res<HipsSurveySettings>,
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
                loader.in_flight.remove(&(tile.depth, tile.hash));
                let image = Image::new(
                    Extent3d {
                        width: tile.width,
                        height: tile.height,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    tile.rgba,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::default(),
                );
                let handle = images.add(image);
                image_cache.images.insert((tile.depth, tile.hash), handle);
            }
            TileLoadResult::Failed { depth, hash } => {
                loader.in_flight.remove(&(depth, hash));
            }
        }
    }

    let survey_name = &settings.survey.name;
    if active_survey_tiles.current_survey_name.as_deref() != Some(survey_name) {
        for entity in active_survey_tiles.tiles.values() {
            commands.entity(*entity).despawn();
        }
        active_survey_tiles.tiles.clear();
        image_cache.images.clear();
        mesh_cache.meshes.clear();
        loader.in_flight.clear();
        active_survey_tiles.current_survey_name = Some(survey_name.clone());
        active_survey_tiles.current_depth = None;

        let survey_arc = Arc::new(settings.survey.clone());
        for h in 0..12 {
            let key = (0, h);
            loader.in_flight.insert(key);
            let _ = loader.request_tx.send((Arc::clone(&survey_arc), 0, h));
        }
    }

    if !settings.enabled {
        if !active_survey_tiles.tiles.is_empty() {
            for entity in active_survey_tiles.tiles.values() {
                commands.entity(*entity).despawn();
            }
            active_survey_tiles.tiles.clear();
            active_survey_tiles.current_depth = None;
        }
        return;
    }

    let Ok(camera) = camera_query.single() else {
        return;
    };

    let aspect = dims.width as f32 / dims.height as f32;
    let proj_params = camera.projection_params(aspect);
    let transform = camera.icrs_to_camera_mat4();

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

    let mut desired_set = HashSet::new();
    let survey_arc = Arc::new(settings.survey.clone());

    let visible_cells = HipsGrid::query_visible_cells(center_icrs, half_diag, target_depth);
    for hash in visible_cells {
        let key = (target_depth, hash);
        if image_cache.images.contains_key(&key) {
            desired_set.insert(key);
        } else {
            if !loader.in_flight.contains(&key) {
                loader.in_flight.insert(key);
                let _ = loader.request_tx.send((Arc::clone(&survey_arc), target_depth, hash));
            }
            let mut fallback_depth = target_depth;
            let mut fallback_hash = hash;
            while fallback_depth > 0 {
                fallback_depth -= 1;
                fallback_hash >>= 2;
                let parent_key = (fallback_depth, fallback_hash);
                if image_cache.images.contains_key(&parent_key) {
                    desired_set.insert(parent_key);
                    break;
                }
            }
        }
    }

    let mut to_remove = Vec::new();
    for (&key, &entity) in &active_survey_tiles.tiles {
        if !desired_set.contains(&key) {
            commands.entity(entity).despawn();
            to_remove.push(key);
        }
    }
    for key in to_remove {
        active_survey_tiles.tiles.remove(&key);
    }

    for key in desired_set {
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
                alpha: settings.alpha,
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

    for &entity in active_survey_tiles.tiles.values() {
        if let Ok(mut mat) = tile_materials.get_mut(entity) {
            mat.uniforms.transform = transform;
            mat.uniforms.projection = proj_params.into();
            mat.uniforms.alpha = settings.alpha;
        }
    }

    if mesh_cache.meshes.len() > 1024 {
        mesh_cache
            .meshes
            .retain(|key, _| active_survey_tiles.tiles.contains_key(key));
    }
    if image_cache.images.len() > 1024 {
        image_cache
            .images
            .retain(|key, _| active_survey_tiles.tiles.contains_key(key));
    }
}
