use bevy::{
    asset::embedded_asset,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::{
        mesh::MeshVertexBufferLayoutRef,
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
    },
    shader::{ShaderRef, load_shader_library},
};

use crate::camera::ProjectionParams;

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct CelestialProjectionUniforms {
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub blend_factor: f32,
    pub scale_factor: f32,
}

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct CelestialLineUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub color: Vec4,
    pub minor_alpha: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct CelestialLineMaterial {
    #[uniform(0)]
    pub uniforms: CelestialLineUniforms,
}

impl From<ProjectionParams> for CelestialProjectionUniforms {
    fn from(projection: ProjectionParams) -> Self {
        Self {
            fov_y: projection.fov_y,
            aspect_ratio: projection.aspect_ratio,
            blend_factor: projection.blend_factor,
            scale_factor: projection.scale_factor,
        }
    }
}

impl Material for CelestialLineMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://planetarium/shaders/celestial_line.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://planetarium/shaders/celestial_line.wgsl".into()
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
        let mut vertex_attributes = vec![Mesh::ATTRIBUTE_POSITION.at_shader_location(0)];
        if layout.0.contains(crate::grids::ATTRIBUTE_LINE_TIER) {
            vertex_attributes.push(crate::grids::ATTRIBUTE_LINE_TIER.at_shader_location(1));
            descriptor
                .vertex
                .shader_defs
                .push("VERTEX_LINE_TIER".into());
        }
        let vertex_layout = layout.0.get_layout(&vertex_attributes)?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(())
    }
}

pub fn init_celestial_materials(app: &mut App) {
    load_shader_library!(app, "shaders/celestial_projection.wgsl");
    embedded_asset!(app, "shaders/celestial_line.wgsl");
    app.add_plugins(MaterialPlugin::<CelestialLineMaterial>::default());
}
