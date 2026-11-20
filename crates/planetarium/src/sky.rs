use bevy::{
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{
            BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState, ColorTargetState,
            ColorWrites, FragmentState, MultisampleState, PrimitiveState, PrimitiveTopology,
            RenderPipelineDescriptor, ShaderStages, ShaderType, SpecializedRenderPipeline,
            TextureFormat, VertexState, binding_types::uniform_buffer,
        },
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::solar::solar_topocentric_enu;
use glam::DMat4;

use crate::{
    ViewportDimensions,
    atmosphere::{AtmosphereSettings, compute_sky_palette},
    camera::{PlanetariumCamera, ProjectionParams},
    ground::{GroundSettings, compute_ground_palette},
    material::{CelestialPipelineId, CelestialProjectionUniforms},
};

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialSkyUniforms {
    pub camera_to_topo: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub sun_dir_and_atmo_strength: Vec4,
    pub zenith_color: Vec4,
    pub horizon_color: Vec4,
    pub dusk_color: Vec4,
    pub sun_glow_color: Vec4,
    pub ground_horizon_color: Vec4,
    pub ground_nadir_color: Vec4,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialSkyMaterial {
    pub uniforms: CelestialSkyUniforms,
}

impl CelestialSkyMaterial {
    pub fn from_camera(
        camera: &PlanetariumCamera,
        projection: ProjectionParams,
        atmosphere_strength: f32,
        ground_opacity: f32,
    ) -> Self {
        let camera_to_topo =
            DMat4::from_mat3(camera.topocentric_to_camera_matrix().transpose()).as_mat4();
        let sun_enu = solar_topocentric_enu(camera.observer.location, camera.observer.epoch);
        let sun_alt_deg = sun_enu.z.clamp(-1.0, 1.0).asin().to_degrees() as f32;
        let palette = compute_sky_palette(sun_alt_deg);
        let (ground_horizon, ground_nadir) = compute_ground_palette(sun_alt_deg);

        let safe_strength = if atmosphere_strength.is_finite() {
            atmosphere_strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let effective_strength = if palette.max_intensity() > 1e-4 {
            safe_strength
        } else {
            0.0
        };

        let safe_opacity = if ground_opacity.is_finite() {
            ground_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };

        Self {
            uniforms: CelestialSkyUniforms {
                camera_to_topo,
                projection: projection.into(),
                sun_dir_and_atmo_strength: Vec4::new(
                    sun_enu.x as f32,
                    sun_enu.y as f32,
                    sun_enu.z as f32,
                    effective_strength,
                ),
                zenith_color: palette.zenith_color.extend(0.0),
                horizon_color: palette.horizon_color.extend(0.0),
                dusk_color: palette.dusk_color.extend(0.0),
                sun_glow_color: palette.sun_glow_color.extend(palette.sun_glow_weight),
                ground_horizon_color: ground_horizon.extend(safe_opacity),
                ground_nadir_color: ground_nadir.extend(0.0),
            },
        }
    }
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CelestialSkyDraw;

impl SyncComponent for CelestialSkyMaterial {
    type Target = (CelestialSkyUniforms, CelestialSkyDraw, CelestialPipelineId);
}

impl ExtractComponent for CelestialSkyMaterial {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = (CelestialSkyUniforms, CelestialSkyDraw);

    fn extract_component(material: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        if material.uniforms.sun_dir_and_atmo_strength.w <= 1e-4
            && material.uniforms.ground_horizon_color.w <= 1e-4
        {
            return None;
        }
        Some((material.uniforms, CelestialSkyDraw))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SkyPipelineKey {
    pub format: TextureFormat,
    pub samples: u32,
}

#[derive(Resource)]
pub struct CelestialSkyPipeline {
    pub layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl CelestialSkyPipeline {
    pub fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "celestial_sky_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (uniform_buffer::<CelestialSkyUniforms>(true),),
            ),
        );
        Self { layout, shader }
    }
}

impl SpecializedRenderPipeline for CelestialSkyPipeline {
    type Key = SkyPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("celestial_sky_pipeline".into()),
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

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn sky_system(
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    atmo_settings: Res<AtmosphereSettings>,
    ground_settings: Res<GroundSettings>,
    mut materials: Query<&mut CelestialSkyMaterial>,
) {
    let Ok(camera) = camera_query.single() else {
        return;
    };
    let aspect = dims.width as f32 / dims.height.max(1) as f32;
    let projection = camera.projection_params(aspect);
    let updated = CelestialSkyMaterial::from_camera(
        camera,
        projection,
        atmo_settings.strength,
        ground_settings.opacity,
    );
    for mut mat in &mut materials {
        mat.uniforms = updated.uniforms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_camera_at_unix(unix_seconds: f64) -> PlanetariumCamera {
        let mut cam = PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        cam.observer.set_epoch_unix_seconds(unix_seconds);
        cam
    }

    #[test]
    fn extract_skips_only_when_both_atmosphere_and_ground_are_inactive() {
        // 2025-03-20T00:00:00Z -> midnight at lon=0, lat=0
        let midnight_cam = test_camera_at_unix(1_742_428_800.0);
        let proj = midnight_cam.projection_params(16.0 / 9.0);

        let all_off = CelestialSkyMaterial::from_camera(&midnight_cam, proj, 0.0, 0.0);
        assert!(all_off.uniforms.sun_dir_and_atmo_strength.w <= 1e-4);
        assert!(all_off.uniforms.ground_horizon_color.w <= 1e-4);
        assert!(CelestialSkyMaterial::extract_component(&all_off).is_none());

        // Even at deep night when atmosphere shuts off, non-zero ground opacity keeps the pass active.
        let ground_only_at_night = CelestialSkyMaterial::from_camera(&midnight_cam, proj, 1.0, 0.8);
        assert_eq!(
            ground_only_at_night.uniforms.sun_dir_and_atmo_strength.w,
            0.0
        );
        assert!((ground_only_at_night.uniforms.ground_horizon_color.w - 0.8).abs() < 1e-6);
        assert!(CelestialSkyMaterial::extract_component(&ground_only_at_night).is_some());

        // 2025-03-20T12:00:00Z -> local noon at lon=0, lat=0
        let noon_cam = test_camera_at_unix(1_742_472_000.0);
        let noon_proj = noon_cam.projection_params(16.0 / 9.0);
        let noon_sky = CelestialSkyMaterial::from_camera(&noon_cam, noon_proj, 0.75, 0.0);
        assert!((noon_sky.uniforms.sun_dir_and_atmo_strength.w - 0.75).abs() < 1e-6);
        assert_eq!(noon_sky.uniforms.ground_horizon_color.w, 0.0);
        assert!(CelestialSkyMaterial::extract_component(&noon_sky).is_some());
    }
}
