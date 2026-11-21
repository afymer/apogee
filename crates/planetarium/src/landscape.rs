use std::sync::Arc;

use bevy::{
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{
            BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState, ColorTargetState,
            ColorWrites, FragmentState, MultisampleState, PrimitiveState, PrimitiveTopology,
            RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
            SpecializedRenderPipeline, TextureFormat, TextureSampleType, VertexState,
            binding_types::{
                sampler, storage_buffer_read_only_sized, texture_2d, uniform_buffer,
            },
        },
        storage::ShaderBuffer,
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::solar::solar_topocentric_enu;
use glam::DMat4;

use crate::{
    ViewportDimensions,
    camera::{PlanetariumCamera, ProjectionParams},
    ground::compute_ground_palette,
    material::{CelestialPipelineId, CelestialProjectionUniforms},
};

/// Decoded RGBA8 equirectangular panorama pixel payload transferred across [`crate::BevyBridge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HorizonImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Complete custom horizon & 360° background state transferred across [`crate::BevyBridge`].
#[derive(Clone, Debug, PartialEq)]
pub struct HorizonLayerData {
    pub id: String,
    pub revision: u64,
    pub samples_deg: Vec<f32>,
    pub azimuth_offset_deg: f64,
    pub image: Option<Arc<HorizonImageData>>,
    pub image_tag: Option<(String, u64)>,
}

/// Renderer-side custom horizon & 360° landscape configuration.
#[derive(Resource, Clone, Debug)]
pub struct HorizonRenderSettings {
    pub opacity: f32,
    pub active: bool,
    pub has_texture: bool,
    pub show_line: bool,
    pub azimuth_offset_rad: f32,
    pub sample_count: u32,
    pub horizon_buffer: Handle<ShaderBuffer>,
    pub panorama_image: Handle<Image>,
    pub fallback_image: Handle<Image>,
    pub current_image_tag: Option<(String, u64)>,
}

impl Default for HorizonRenderSettings {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            active: false,
            has_texture: false,
            show_line: false,
            azimuth_offset_rad: 0.0,
            sample_count: 0,
            horizon_buffer: Handle::default(),
            panorama_image: Handle::default(),
            fallback_image: Handle::default(),
            current_image_tag: None,
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Computes linear-RGB ambient landscape lighting adapted to solar altitude so daytime
/// 360° panoramas dim naturally through twilight and moonlit night.
pub fn compute_landscape_lighting(sun_alt_deg: f32) -> Vec3 {
    let night = Vec3::new(0.042, 0.052, 0.075);
    let twilight = Vec3::new(0.22, 0.20, 0.24);
    let day = Vec3::new(1.0, 1.0, 1.0);

    let tw_t = smoothstep(-14.0, -2.0, sun_alt_deg);
    let day_t = smoothstep(-2.0, 12.0, sun_alt_deg);

    night.lerp(twilight, tw_t).lerp(day, day_t)
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialLandscapeUniforms {
    pub camera_to_topo: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub params: Vec4,
    pub lighting_and_line: Vec4,
    pub silhouette_horizon_color: Vec4,
    pub silhouette_nadir_color: Vec4,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialLandscapeMaterial {
    pub uniforms: CelestialLandscapeUniforms,
    pub horizon_buffer: Handle<ShaderBuffer>,
    pub panorama_image: Handle<Image>,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialLandscapeDraw {
    pub horizon_buffer: AssetId<ShaderBuffer>,
    pub panorama_image: AssetId<Image>,
}

impl SyncComponent for CelestialLandscapeMaterial {
    type Target = (
        CelestialLandscapeUniforms,
        CelestialLandscapeDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialLandscapeMaterial {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = (CelestialLandscapeUniforms, CelestialLandscapeDraw);

    fn extract_component(material: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        if material.uniforms.params.x <= 1e-4 || material.uniforms.params.w < 1.0 {
            return None;
        }
        Some((
            material.uniforms,
            CelestialLandscapeDraw {
                horizon_buffer: material.horizon_buffer.id(),
                panorama_image: material.panorama_image.id(),
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LandscapePipelineKey {
    pub format: TextureFormat,
    pub samples: u32,
}

#[derive(Resource)]
pub struct CelestialLandscapePipeline {
    pub layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl CelestialLandscapePipeline {
    pub fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "celestial_landscape_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer::<CelestialLandscapeUniforms>(true),
                    storage_buffer_read_only_sized(false, None),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        );
        Self { layout, shader }
    }
}

impl SpecializedRenderPipeline for CelestialLandscapePipeline {
    type Key = LandscapePipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("celestial_landscape_pipeline".into()),
            layout: vec![self.layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                entry_point: Some("vertex".into()),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: key.format,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
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
}

impl CelestialLandscapeMaterial {
    pub fn from_camera_and_settings(
        camera: &PlanetariumCamera,
        projection: ProjectionParams,
        settings: &HorizonRenderSettings,
    ) -> Self {
        let camera_to_topo =
            DMat4::from_mat3(camera.topocentric_to_camera_matrix().transpose()).as_mat4();
        let sun_enu = solar_topocentric_enu(camera.observer.location, camera.observer.epoch);
        let sun_alt_deg = sun_enu.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
        let lighting = compute_landscape_lighting(sun_alt_deg);
        let (sil_horizon, sil_nadir) = compute_ground_palette(sun_alt_deg);

        let effective_opacity = if settings.active && settings.opacity.is_finite() {
            settings.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        };

        let sample_count = if settings.active {
            settings.sample_count
        } else {
            0
        };

        let line_alpha = match (settings.show_line, settings.has_texture) {
            (false, _) => 0.0,
            (true, true) => 0.45,
            (true, false) => 0.85,
        };

        Self {
            uniforms: CelestialLandscapeUniforms {
                camera_to_topo,
                projection: projection.into(),
                params: Vec4::new(
                    effective_opacity,
                    settings.azimuth_offset_rad,
                    if settings.has_texture { 1.0 } else { 0.0 },
                    sample_count as f32,
                ),
                lighting_and_line: lighting.extend(line_alpha),
                silhouette_horizon_color: sil_horizon.extend(0.0),
                silhouette_nadir_color: sil_nadir.extend(0.0),
            },
            horizon_buffer: settings.horizon_buffer.clone(),
            panorama_image: settings.panorama_image.clone(),
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn landscape_system(
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    settings: Res<HorizonRenderSettings>,
    mut materials: Query<&mut CelestialLandscapeMaterial>,
) {
    let Ok(camera) = camera_query.single() else {
        return;
    };
    let aspect = dims.width as f32 / dims.height.max(1) as f32;
    let projection = camera.projection_params(aspect);
    let updated = CelestialLandscapeMaterial::from_camera_and_settings(camera, projection, &settings);
    for mut mat in &mut materials {
        *mat = updated.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_skips_when_inactive_or_zero_opacity() {
        let cam = PlanetariumCamera::default();
        let proj = cam.projection_params(16.0 / 9.0);

        let inactive = HorizonRenderSettings {
            opacity: 1.0,
            active: false,
            sample_count: 720,
            ..Default::default()
        };
        let mat_inactive =
            CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &inactive);
        assert!(CelestialLandscapeMaterial::extract_component(&mat_inactive).is_none());

        let zero_opacity = HorizonRenderSettings {
            opacity: 0.0,
            active: true,
            sample_count: 720,
            ..Default::default()
        };
        let mat_zero =
            CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &zero_opacity);
        assert!(CelestialLandscapeMaterial::extract_component(&mat_zero).is_none());

        let active = HorizonRenderSettings {
            opacity: 0.6,
            active: true,
            sample_count: 720,
            ..Default::default()
        };
        let mat_active = CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &active);
        assert!(CelestialLandscapeMaterial::extract_component(&mat_active).is_some());
    }

    #[test]
    fn landscape_lighting_dims_smoothly_from_day_to_night() {
        let day = compute_landscape_lighting(40.0);
        let tw = compute_landscape_lighting(-5.0);
        let night = compute_landscape_lighting(-20.0);
        assert!(day.length() > tw.length());
        assert!(tw.length() > night.length());
        assert!((day - Vec3::ONE).length() < 1e-5);
        assert!(night.length() > 0.05 && night.length() < 0.25);
    }

    #[test]
    fn line_alpha_hidden_unless_enabled() {
        let cam = PlanetariumCamera::default();
        let proj = cam.projection_params(16.0 / 9.0);

        let textured_hidden = HorizonRenderSettings {
            opacity: 1.0,
            active: true,
            has_texture: true,
            show_line: false,
            sample_count: 720,
            ..Default::default()
        };
        let mat_hidden =
            CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &textured_hidden);
        assert_eq!(mat_hidden.uniforms.lighting_and_line.w, 0.0);

        let textured_shown = HorizonRenderSettings {
            show_line: true,
            ..textured_hidden
        };
        let mat_shown =
            CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &textured_shown);
        assert!(mat_shown.uniforms.lighting_and_line.w > 0.0);

        let silhouette = HorizonRenderSettings {
            opacity: 1.0,
            active: true,
            has_texture: false,
            show_line: false,
            sample_count: 720,
            ..Default::default()
        };
        let mat_sil = CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &silhouette);
        assert_eq!(mat_sil.uniforms.lighting_and_line.w, 0.0);

        let silhouette_shown = HorizonRenderSettings {
            show_line: true,
            ..silhouette
        };
        let mat_sil_shown =
            CelestialLandscapeMaterial::from_camera_and_settings(&cam, proj, &silhouette_shown);
        assert!(mat_sil_shown.uniforms.lighting_and_line.w > 0.0);
    }
}
