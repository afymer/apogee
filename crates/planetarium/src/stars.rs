use std::{fs::File, path::PathBuf};

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::{
        mesh::MeshVertexBufferLayoutRef,
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
        storage::ShaderBuffer,
    },
    shader::ShaderRef,
};
use celestial::catalog::{StarCatalogHeader, StarRecord, read_stars_from_zst};
use tracing::{error, info, warn};

use crate::material::CelestialProjectionUniforms;

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct CelestialStarUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub viewport_size: Vec2,
    pub limiting_mag: f32,
    pub fade_range: f32,
    pub base_radius: f32,
    pub min_radius: f32,
    pub max_radius: f32,
    pub padding: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct CelestialStarMaterial {
    #[uniform(0)]
    pub uniforms: CelestialStarUniforms,
    #[storage(1, read_only)]
    pub stars: Handle<ShaderBuffer>,
}

impl Material for CelestialStarMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://planetarium/shaders/celestial_star.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://planetarium/shaders/celestial_star.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(())
    }
}

pub fn create_star_quad_mesh(star_count: usize) -> Mesh {
    let mut positions = Vec::with_capacity(star_count * 4);
    let mut indices = Vec::with_capacity(star_count * 6);

    for i in 0..star_count {
        let base = (i * 4) as u32;
        positions.push(Vec3::new(-1.0, -1.0, 0.0));
        positions.push(Vec3::new(1.0, -1.0, 0.0));
        positions.push(Vec3::new(1.0, 1.0, 0.0));
        positions.push(Vec3::new(-1.0, 1.0, 0.0));

        indices.push(base);
        indices.push(base + 1);
        indices.push(base + 2);
        indices.push(base);
        indices.push(base + 2);
        indices.push(base + 3);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Discovers and loads the star catalog from disk, checks `APOGEE_STARS_PATH`
/// first, then searches common relative locations
pub fn load_star_catalog() -> Option<(StarCatalogHeader, Vec<StarRecord>)> {
    let mut candidates = Vec::new();

    if let Ok(env_path) = std::env::var("APOGEE_STARS_PATH") {
        candidates.push(PathBuf::from(env_path));
    }

    candidates.push(PathBuf::from("assets/catalogs/stars.bin.zst"));
    candidates.push(PathBuf::from("../assets/catalogs/stars.bin.zst"));
    candidates.push(PathBuf::from("../../assets/catalogs/stars.bin.zst"));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets/catalogs/stars.bin.zst"));
            candidates.push(dir.join("stars.bin.zst"));
        }
    }

    for path in &candidates {
        if path.exists() {
            info!("Loading star catalog from {:?}", path);
            match File::open(path) {
                Ok(file) => match read_stars_from_zst(file) {
                    Ok((header, stars)) => {
                        info!(
                            "Loaded {} stars (magnitudes {:.2} to {:.2}) from {:?}",
                            stars.len(),
                            header.min_mag,
                            header.max_mag,
                            path
                        );
                        return Some((header, stars));
                    }
                    Err(e) => {
                        warn!("Failed to parse star catalog at {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    warn!("Failed to open star catalog at {:?}: {}", path, e);
                }
            }
        }
    }

    error!(
        "Star catalog file (stars.bin.zst) not found in candidate paths: {:?}",
        candidates
    );
    None
}
