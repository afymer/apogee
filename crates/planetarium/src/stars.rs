use std::{fs::File, path::PathBuf};

use bevy::{
    asset::RenderAssetUsages,
    ecs::query::QueryItem,
    mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology},
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{
            BindGroupLayoutDescriptor, BindGroupLayoutEntries, RenderPipelineDescriptor,
            ShaderStages, ShaderType, SpecializedMeshPipeline, SpecializedMeshPipelineError,
            binding_types::{storage_buffer_read_only_sized, uniform_buffer},
        },
        storage::ShaderBuffer,
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::catalog::{StarCatalogHeader, StarRecord, read_stars_from_zst};
use tracing::{error, info, warn};

use crate::material::{
    CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
};

/// Renderer-side catalog stars layer configuration controlled via [`crate::ViewportCommand`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct StarSettings {
    pub opacity: f32,
}

impl Default for StarSettings {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialStarUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub viewport_size: Vec2,
    pub limiting_mag: f32,
    pub fade_range: f32,
    pub base_radius: f32,
    pub min_radius: f32,
    pub max_radius: f32,
    pub opacity: f32,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialStarMaterial {
    pub uniforms: CelestialStarUniforms,
    pub stars: Handle<ShaderBuffer>,
    pub star_count: u32,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialStarDraw {
    pub mesh: AssetId<Mesh>,
    pub stars: AssetId<ShaderBuffer>,
    pub star_count: u32,
}

impl SyncComponent for CelestialStarMaterial {
    type Target = (
        CelestialStarUniforms,
        CelestialStarDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialStarMaterial {
    type QueryData = (&'static Self, &'static Mesh3d);
    type QueryFilter = ();
    type Out = (CelestialStarUniforms, CelestialStarDraw);

    fn extract_component(
        (material, mesh): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        if material.uniforms.opacity <= 1e-4 {
            return None;
        }
        Some((
            material.uniforms,
            CelestialStarDraw {
                mesh: mesh.id(),
                stars: material.stars.id(),
                star_count: material.star_count,
            },
        ))
    }
}

#[derive(Resource)]
pub struct CelestialStarPipeline {
    pub layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl CelestialStarPipeline {
    pub fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "celestial_star_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer::<CelestialStarUniforms>(true),
                    storage_buffer_read_only_sized(false, None),
                ),
            ),
        );
        Self { layout, shader }
    }
}

impl SpecializedMeshPipeline for CelestialStarPipeline {
    type Key = CelestialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor =
            pipeline_descriptor("celestial_star_pipeline", &self.layout, &self.shader, key);
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

pub fn create_star_quad_mesh() -> Mesh {
    let _span = tracing::info_span!("create_star_quad_mesh").entered();
    let positions = vec![
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
    ];
    let indices = vec![0u32, 1, 2, 0, 2, 3];

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
    let _span = tracing::info_span!("load_star_catalog").entered();
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
            let file = {
                let _span = tracing::info_span!("open_catalog_file", ?path).entered();
                File::open(path)
            };
            match file {
                Ok(file) => {
                    let parse_res = {
                        let _span = tracing::info_span!("read_stars_from_zst").entered();
                        read_stars_from_zst(file)
                    };
                    match parse_res {
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
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_skips_stars_when_opacity_is_zero() {
        let mut mat = CelestialStarMaterial {
            uniforms: CelestialStarUniforms {
                transform: Mat4::IDENTITY,
                projection: CelestialProjectionUniforms {
                    fov_y: 1.0,
                    aspect_ratio: 1.0,
                    blend_factor: 0.0,
                    scale_factor: 1.0,
                },
                viewport_size: Vec2::new(800.0, 600.0),
                limiting_mag: 10.5,
                fade_range: 0.6,
                base_radius: 2.2,
                min_radius: 0.8,
                max_radius: 24.0,
                opacity: 0.0,
            },
            stars: Handle::default(),
            star_count: 10,
        };
        let mesh = Mesh3d(Handle::default());
        assert!(CelestialStarMaterial::extract_component((&mat, &mesh)).is_none());

        mat.uniforms.opacity = 1e-4;
        assert!(CelestialStarMaterial::extract_component((&mat, &mesh)).is_none());

        mat.uniforms.opacity = 0.25;
        assert!(CelestialStarMaterial::extract_component((&mat, &mesh)).is_some());
    }
}
