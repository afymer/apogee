//! This is a replacement for the painfully slow bevy_pbr crate

use std::ops::Range;

use bevy::{
    asset::embedded_asset,
    core_pipeline::{Core3d, Core3dSystems, core_3d::main_transparent_pass_3d},
    ecs::query::QueryItem,
    mesh::MeshVertexBufferLayoutRef,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BlendState, Buffer, BufferId, BufferSlice, CachedRenderPipelineId, ColorTargetState, ColorWrites,
            FragmentState, IndexFormat, MultisampleState, PipelineCache, PrimitiveState,
            PrimitiveTopology, RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor,
            SamplerBindingType, ShaderStages, ShaderType, SpecializedMeshPipeline,
            SpecializedMeshPipelineError, SpecializedMeshPipelines, SpecializedRenderPipelines,
            TextureFormat, TextureSampleType, VertexState,
            binding_types::{sampler, texture_2d, uniform_buffer},
        },
        renderer::{RenderContext, ViewQuery},
        storage::GpuShaderBuffer,
        sync_component::SyncComponent,
        texture::GpuImage,
        view::{Msaa, ViewTarget},
    },
    shader::{Shader, load_shader_library},
};
use tracing::error;

use crate::{
    camera::ProjectionParams,
    sky::{
        CelestialSkyDraw, CelestialSkyMaterial, CelestialSkyPipeline, CelestialSkyUniforms,
        SkyPipelineKey,
    },
    stars::{
        CelestialStarDraw, CelestialStarMaterial, CelestialStarPipeline, CelestialStarUniforms,
    },
};

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct CelestialProjectionUniforms {
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub blend_factor: f32,
    pub scale_factor: f32,
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialLineUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub color: Vec4,
    pub minor_alpha: f32,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialLineMaterial {
    pub uniforms: CelestialLineUniforms,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialLineDraw {
    pub mesh: AssetId<Mesh>,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialPipelineId(pub CachedRenderPipelineId);

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

impl SyncComponent for CelestialLineMaterial {
    type Target = (
        CelestialLineUniforms,
        CelestialLineDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialLineMaterial {
    type QueryData = (&'static Self, &'static Mesh3d);
    type QueryFilter = ();
    type Out = (CelestialLineUniforms, CelestialLineDraw);

    fn extract_component(
        (material, mesh): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        Some((material.uniforms, CelestialLineDraw { mesh: mesh.id() }))
    }
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialTileUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub alpha: f32,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialTileMaterial {
    pub uniforms: CelestialTileUniforms,
    pub image: Handle<Image>,
    pub depth: u8,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialTileDraw {
    pub mesh: AssetId<Mesh>,
    pub image: AssetId<Image>,
    pub depth: u8,
}

impl SyncComponent for CelestialTileMaterial {
    type Target = (
        CelestialTileUniforms,
        CelestialTileDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialTileMaterial {
    type QueryData = (&'static Self, &'static Mesh3d);
    type QueryFilter = ();
    type Out = (CelestialTileUniforms, CelestialTileDraw);

    fn extract_component(
        (material, mesh): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        if material.uniforms.alpha <= 1e-4 {
            return None;
        }
        Some((
            material.uniforms,
            CelestialTileDraw {
                mesh: mesh.id(),
                image: material.image.id(),
                depth: material.depth,
            },
        ))
    }
}

/// Everything a pipeline variant depends on besides the mesh vertex layout
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CelestialPipelineKey {
    pub format: TextureFormat,
    pub samples: u32,
    pub topology: PrimitiveTopology,
}


pub fn pipeline_descriptor(
    label: &'static str,
    layout: &BindGroupLayoutDescriptor,
    shader: &Handle<Shader>,
    key: CelestialPipelineKey,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some(label.into()),
        layout: vec![layout.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            entry_point: Some("vertex".into()),
            ..default()
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("fragment".into()),
            targets: vec![Some(ColorTargetState {
                format: key.format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        primitive: PrimitiveState {
            topology: key.topology,
            ..default()
        },
        multisample: MultisampleState {
            count: key.samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        ..default()
    }
}

#[derive(Resource)]
pub struct CelestialTilePipeline {
    pub layout: BindGroupLayoutDescriptor,
    pub shader: Handle<Shader>,
}

#[derive(Resource, Default)]
pub struct CelestialTileBindGroupCache {
    pub buffer_id: Option<BufferId>,
    pub cache: std::collections::HashMap<AssetId<Image>, BindGroup>,
}

impl SpecializedMeshPipeline for CelestialTilePipeline {
    type Key = CelestialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor =
            pipeline_descriptor("celestial_tile_pipeline", &self.layout, &self.shader, key);
        let vertex_attributes = vec![
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(1),
        ];
        let vertex_layout = layout.0.get_layout(&vertex_attributes)?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

#[derive(Resource)]
struct CelestialLinePipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl SpecializedMeshPipeline for CelestialLinePipeline {
    type Key = CelestialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor =
            pipeline_descriptor("celestial_line_pipeline", &self.layout, &self.shader, key);
        let vertex_attributes = vec![
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            crate::grids::ATTRIBUTE_LINE_TIER.at_shader_location(1),
        ];
        let vertex_layout = layout.0.get_layout(&vertex_attributes)?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct CelestialPipelines<'w> {
    tile: Res<'w, CelestialTilePipeline>,
    line: Res<'w, CelestialLinePipeline>,
    star: Res<'w, CelestialStarPipeline>,
    sky: Res<'w, CelestialSkyPipeline>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct SpecializedCelestialPipelines<'w> {
    tile: ResMut<'w, SpecializedMeshPipelines<CelestialTilePipeline>>,
    line: ResMut<'w, SpecializedMeshPipelines<CelestialLinePipeline>>,
    star: ResMut<'w, SpecializedMeshPipelines<CelestialStarPipeline>>,
    sky: ResMut<'w, SpecializedRenderPipelines<CelestialSkyPipeline>>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct CelestialUniforms<'w> {
    tile: Res<'w, ComponentUniforms<CelestialTileUniforms>>,
    line: Res<'w, ComponentUniforms<CelestialLineUniforms>>,
    star: Res<'w, ComponentUniforms<CelestialStarUniforms>>,
    sky: Res<'w, ComponentUniforms<CelestialSkyUniforms>>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct CelestialDrawQueries<'w, 's> {
    tiles: Query<
        'w,
        's,
        (
            &'static CelestialTileDraw,
            &'static DynamicUniformIndex<CelestialTileUniforms>,
            &'static CelestialPipelineId,
        ),
    >,
    lines: Query<
        'w,
        's,
        (
            &'static CelestialLineDraw,
            &'static DynamicUniformIndex<CelestialLineUniforms>,
            &'static CelestialPipelineId,
        ),
    >,
    stars: Query<
        'w,
        's,
        (
            &'static CelestialStarDraw,
            &'static DynamicUniformIndex<CelestialStarUniforms>,
            &'static CelestialPipelineId,
        ),
    >,
    skies: Query<
        'w,
        's,
        (
            &'static DynamicUniformIndex<CelestialSkyUniforms>,
            &'static CelestialPipelineId,
        ),
        With<CelestialSkyDraw>,
    >,
}

pub fn init_celestial_materials(app: &mut App) {
    load_shader_library!(app, "shaders/celestial_projection.wgsl");
    embedded_asset!(app, "shaders/celestial_tile.wgsl");
    embedded_asset!(app, "shaders/celestial_line.wgsl");
    embedded_asset!(app, "shaders/celestial_star.wgsl");
    embedded_asset!(app, "shaders/celestial_sky.wgsl");

    app.add_plugins((
        ExtractComponentPlugin::<CelestialTileMaterial>::default(),
        UniformComponentPlugin::<CelestialTileUniforms>::default(),
        ExtractComponentPlugin::<CelestialLineMaterial>::default(),
        UniformComponentPlugin::<CelestialLineUniforms>::default(),
        ExtractComponentPlugin::<CelestialStarMaterial>::default(),
        UniformComponentPlugin::<CelestialStarUniforms>::default(),
        ExtractComponentPlugin::<CelestialSkyMaterial>::default(),
        UniformComponentPlugin::<CelestialSkyUniforms>::default(),
    ));

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<SpecializedMeshPipelines<CelestialTilePipeline>>()
        .init_resource::<SpecializedMeshPipelines<CelestialLinePipeline>>()
        .init_resource::<SpecializedMeshPipelines<CelestialStarPipeline>>()
        .init_resource::<SpecializedRenderPipelines<CelestialSkyPipeline>>()
        .init_resource::<CelestialTileBindGroupCache>()
        .add_systems(RenderStartup, init_celestial_pipelines)
        .add_systems(
            Render,
            queue_celestial_pipelines.in_set(RenderSystems::Queue),
        )
        .add_systems(
            Core3d,
            draw_celestial
                .in_set(Core3dSystems::MainPass)
                .after(main_transparent_pass_3d),
        );
}

#[allow(clippy::needless_pass_by_value)]
fn init_celestial_pipelines(mut commands: Commands, asset_server: Res<AssetServer>) {
    let tile_layout = BindGroupLayoutDescriptor::new(
        "celestial_tile_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                uniform_buffer::<CelestialTileUniforms>(true),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    commands.insert_resource(CelestialTilePipeline {
        layout: tile_layout,
        shader: asset_server.load("embedded://planetarium/shaders/celestial_tile.wgsl"),
    });

    let line_layout = BindGroupLayoutDescriptor::new(
        "celestial_line_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (uniform_buffer::<CelestialLineUniforms>(true),),
        ),
    );
    commands.insert_resource(CelestialLinePipeline {
        layout: line_layout,
        shader: asset_server.load("embedded://planetarium/shaders/celestial_line.wgsl"),
    });
    commands.insert_resource(CelestialStarPipeline::new(
        asset_server.load("embedded://planetarium/shaders/celestial_star.wgsl"),
    ));
    commands.insert_resource(CelestialSkyPipeline::new(
        asset_server.load("embedded://planetarium/shaders/celestial_sky.wgsl"),
    ));
}

fn specialize_pipeline<S: SpecializedMeshPipeline<Key = CelestialPipelineKey>>(
    cache: &PipelineCache,
    specializer: &S,
    pipelines: &mut SpecializedMeshPipelines<S>,
    meshes: &RenderAssets<RenderMesh>,
    mesh: AssetId<Mesh>,
    target: &ViewTarget,
    msaa: Msaa,
) -> Option<CelestialPipelineId> {
    let render_mesh = meshes.get(mesh)?;
    let key = CelestialPipelineKey {
        format: target.main_texture_format(),
        samples: msaa.samples(),
        topology: render_mesh.primitive_topology(),
    };
    match pipelines.specialize(cache, specializer, key, &render_mesh.layout) {
        Ok(id) => Some(CelestialPipelineId(id)),
        Err(err) => {
            error!("Failed to specialize celestial pipeline: {err}");
            None
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn queue_celestial_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    pipelines: CelestialPipelines,
    mut specialized: SpecializedCelestialPipelines,
    meshes: Res<RenderAssets<RenderMesh>>,
    views: Query<(&ViewTarget, &Msaa)>,
    tiles: Query<(Entity, &CelestialTileDraw)>,
    lines: Query<(Entity, &CelestialLineDraw)>,
    stars: Query<(Entity, &CelestialStarDraw)>,
    skies: Query<(Entity, &CelestialSkyDraw)>,
) {
    for (target, msaa) in &views {
        for (entity, draw) in &tiles {
            if let Some(id) = specialize_pipeline(
                &cache,
                &*pipelines.tile,
                &mut specialized.tile,
                &meshes,
                draw.mesh,
                target,
                *msaa,
            ) {
                commands.entity(entity).insert(id);
            }
        }
        for (entity, draw) in &lines {
            if let Some(id) = specialize_pipeline(
                &cache,
                &*pipelines.line,
                &mut specialized.line,
                &meshes,
                draw.mesh,
                target,
                *msaa,
            ) {
                commands.entity(entity).insert(id);
            }
        }
        for (entity, draw) in &stars {
            if let Some(id) = specialize_pipeline(
                &cache,
                &*pipelines.star,
                &mut specialized.star,
                &meshes,
                draw.mesh,
                target,
                *msaa,
            ) {
                commands.entity(entity).insert(id);
            }
        }
        for (entity, _draw) in &skies {
            let id = specialized.sky.specialize(
                &cache,
                &pipelines.sky,
                SkyPipelineKey {
                    format: target.main_texture_format(),
                    samples: msaa.samples(),
                },
            );
            commands.entity(entity).insert(CelestialPipelineId(id));
        }
    }
}

enum Geometry<'a> {
    Indexed {
        buffer: &'a Buffer,
        format: IndexFormat,
        range: Range<u32>,
    },
    Plain {
        vertex_count: u32,
    },
}

struct DrawItem<'a> {
    pipeline: &'a RenderPipeline,
    bind_group: BindGroup,
    uniform_offset: u32,
    vertex_buffer: Option<BufferSlice<'a>>,
    geometry: Geometry<'a>,
    instances: Range<u32>,
}

/// Locates a mesh's GPU buffers.
///
/// The vertex buffer is bound at the mesh's byte offset inside its slab and
/// draws use a base vertex of zero, so `@builtin(vertex_index)` starts at 0 in
/// the shaders regardless of where the allocator placed the mesh.
fn locate_mesh<'a>(
    mesh: AssetId<Mesh>,
    meshes: &RenderAssets<RenderMesh>,
    allocator: &'a MeshAllocator,
) -> Option<(BufferSlice<'a>, Geometry<'a>)> {
    let render_mesh = meshes.get(mesh)?;
    let vertex_slice = allocator.mesh_vertex_slice(&mesh)?;
    let stride = render_mesh.layout.0.layout().array_stride;
    let vertex_buffer = vertex_slice
        .buffer
        .slice(u64::from(vertex_slice.range.start) * stride..);

    let geometry = match &render_mesh.buffer_info {
        RenderMeshBufferInfo::Indexed { index_format, .. } => {
            let index_slice = allocator.mesh_index_slice(&mesh)?;
            Geometry::Indexed {
                buffer: index_slice.buffer,
                format: *index_format,
                range: index_slice.range,
            }
        }
        RenderMeshBufferInfo::NonIndexed => Geometry::Plain {
            vertex_count: render_mesh.vertex_count,
        },
    };
    Some((vertex_buffer, geometry))
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn draw_celestial(
    view: ViewQuery<&ViewTarget>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: CelestialPipelines,
    meshes: Res<RenderAssets<RenderMesh>>,
    images: Res<RenderAssets<GpuImage>>,
    allocator: Res<MeshAllocator>,
    shader_buffers: Res<RenderAssets<GpuShaderBuffer>>,
    uniforms: CelestialUniforms,
    queries: CelestialDrawQueries,
    mut tile_bind_groups: ResMut<CelestialTileBindGroupCache>,
    mut ctx: RenderContext,
) {
    let target = view.into_inner();
    let mut items: Vec<DrawItem> = Vec::new();

    if let Some(binding) = uniforms.tile.binding() {
        let current_buf_id = uniforms.tile.buffer().map(|b| b.id());
        if tile_bind_groups.buffer_id != current_buf_id {
            tile_bind_groups.cache.clear();
            tile_bind_groups.buffer_id = current_buf_id;
        }
        tile_bind_groups
            .cache
            .retain(|img_id, _| images.get(*img_id).is_some());

        let mut sorted_tiles: Vec<_> = queries.tiles.iter().collect();
        sorted_tiles.sort_by_key(|(draw, _, _)| draw.depth);

        for (draw, uniform_index, pipeline_id) in sorted_tiles {
            let (Some(pipeline), Some((vertex_buffer, geometry)), Some(gpu_image)) = (
                pipeline_cache.get_render_pipeline(pipeline_id.0),
                locate_mesh(draw.mesh, &meshes, &allocator),
                images.get(draw.image),
            ) else {
                continue;
            };
            let bind_group = tile_bind_groups
                .cache
                .entry(draw.image)
                .or_insert_with(|| {
                    ctx.render_device().create_bind_group(
                        "celestial_tile_bind_group",
                        &pipeline_cache.get_bind_group_layout(&pipelines.tile.layout),
                        &BindGroupEntries::sequential((
                            binding.clone(),
                            &gpu_image.texture_view,
                            &gpu_image.sampler,
                        )),
                    )
                })
                .clone();
            items.push(DrawItem {
                pipeline,
                bind_group,
                uniform_offset: uniform_index.index(),
                vertex_buffer: Some(vertex_buffer),
                geometry,
                instances: 0..1,
            });
        }
    }

    if let Some(binding) = uniforms.star.binding() {
        for (draw, uniform_index, pipeline_id) in &queries.stars {
            let (Some(pipeline), Some((vertex_buffer, geometry)), Some(stars_buffer)) = (
                pipeline_cache.get_render_pipeline(pipeline_id.0),
                locate_mesh(draw.mesh, &meshes, &allocator),
                shader_buffers.get(draw.stars),
            ) else {
                continue;
            };
            let bind_group = ctx.render_device().create_bind_group(
                "celestial_star_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.star.layout),
                &BindGroupEntries::sequential((
                    binding.clone(),
                    stars_buffer.buffer.as_entire_binding(),
                )),
            );
            items.push(DrawItem {
                pipeline,
                bind_group,
                uniform_offset: uniform_index.index(),
                vertex_buffer: Some(vertex_buffer),
                geometry,
                instances: 0..draw.star_count,
            });
        }
    }

    if !queries.skies.is_empty() {
        if let Some(binding) = uniforms.sky.binding() {
            let bind_group = ctx.render_device().create_bind_group(
                "celestial_sky_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipelines.sky.layout),
                &BindGroupEntries::sequential((binding,)),
            );
            for (uniform_index, pipeline_id) in &queries.skies {
                let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id.0) else {
                    continue;
                };
                items.push(DrawItem {
                    pipeline,
                    bind_group: bind_group.clone(),
                    uniform_offset: uniform_index.index(),
                    vertex_buffer: None,
                    geometry: Geometry::Plain { vertex_count: 3 },
                    instances: 0..1,
                });
            }
        }
    }

    if let Some(binding) = uniforms.line.binding() {
        let bind_group = ctx.render_device().create_bind_group(
            "celestial_line_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipelines.line.layout),
            &BindGroupEntries::sequential((binding,)),
        );
        for (draw, uniform_index, pipeline_id) in &queries.lines {
            let (Some(pipeline), Some((vertex_buffer, geometry))) = (
                pipeline_cache.get_render_pipeline(pipeline_id.0),
                locate_mesh(draw.mesh, &meshes, &allocator),
            ) else {
                continue;
            };
            items.push(DrawItem {
                pipeline,
                bind_group: bind_group.clone(),
                uniform_offset: uniform_index.index(),
                vertex_buffer: Some(vertex_buffer),
                geometry,
                instances: 0..1,
            });
        }
    }

    if items.is_empty() {
        return;
    }

    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("celestial_pass"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    for item in &items {
        pass.set_render_pipeline(item.pipeline);
        pass.set_bind_group(0, &item.bind_group, &[item.uniform_offset]);
        if let Some(vertex_buffer) = &item.vertex_buffer {
            pass.set_vertex_buffer(0, vertex_buffer.clone());
        }
        match &item.geometry {
            Geometry::Indexed {
                buffer,
                format,
                range,
            } => {
                pass.set_index_buffer(buffer.slice(..), *format);
                pass.draw_indexed(range.clone(), 0, item.instances.clone());
            }
            Geometry::Plain { vertex_count } => {
                pass.draw(0..*vertex_count, item.instances.clone());
            }
        }
    }
}
