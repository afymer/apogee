use std::collections::{HashMap, HashSet};

use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{mesh::Mesh, render_resource::PrimitiveTopology},
};
use glam::Vec3;
use hips::{HipsGrid, HipsGridConfig};

use crate::{
    camera::PlanetariumCamera,
    grids::ATTRIBUTE_LINE_TIER,
    material::{CelestialLineMaterial, CelestialLineUniforms},
    ViewportDimensions,
};

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HipsTile {
    pub depth: u8,
    pub hash: u64,
}

#[derive(Resource, Default)]
pub struct HipsMeshCache {
    pub meshes: HashMap<(u8, u64), Handle<Mesh>>,
}

#[derive(Resource, Default)]
pub struct HipsActiveTiles {
    pub tiles: HashMap<(u8, u64), Entity>,
    pub current_depth: Option<u8>,
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

pub(crate) fn create_tile_boundary_mesh(depth: u8, hash: u64) -> Mesh {
    let segments = HipsGrid::cell_boundary_segments(
        depth,
        hash,
        HipsGrid::default_segments_per_edge(depth),
    );
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
