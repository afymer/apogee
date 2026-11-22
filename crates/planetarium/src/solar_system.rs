use bevy::{
    ecs::query::QueryItem,
    mesh::MeshVertexBufferLayoutRef,
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{
            BindGroupLayoutDescriptor, BindGroupLayoutEntries, RenderPipelineDescriptor,
            ShaderStages, ShaderType, SpecializedMeshPipeline, SpecializedMeshPipelineError,
            binding_types::uniform_buffer,
        },
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::{
    HorizonProfile, SolarSystemBody, SolarSystemEphemeris, solar_ephemeris, solar_system_ephemeris,
};
use glam::{DMat4, DVec3};

use crate::{
    ViewportDimensions,
    camera::{PlanetariumCamera, PlanetariumViewInfo, ProjectionParams},
    ground::GroundSettings,
    material::{
        CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
    },
};

pub const SUN_QUAD_SCALE: f32 = 2.8;
pub const MOON_QUAD_SCALE: f32 = 2.0;

/// Renderer-side solar-system bodies (Sun and Moon) layer configuration controlled via [`crate::ViewportCommand`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct SolarSystemRenderSettings {
    pub opacity: f32,
}

impl Default for SolarSystemRenderSettings {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

/// Computes the rendered disc radius (in physical pixels) for a [`SolarSystemBody`],
/// combining a comfortable wide-FOV minimum size with exact physical angular diameter when zoomed in.
pub fn rendered_solar_system_disc_radius_px(
    body: SolarSystemBody,
    angular_radius_deg: f64,
    fov_y_rad: f32,
    viewport_height_px: f32,
) -> f32 {
    let fov = fov_y_rad.max(1e-4);
    let vh = viewport_height_px.max(1.0);
    let ang_rad = (angular_radius_deg.to_radians() as f32).max(1e-4);
    let true_disc_px = (ang_rad / fov) * vh;

    let base_min_px = match body {
        SolarSystemBody::Sun => 8.5_f32,
        SolarSystemBody::Moon => 7.8_f32,
    };
    let zoom_boost = (1.8_f32 / fov).powf(0.18).clamp(0.85, 1.75);
    let wide_disc_px = base_min_px * zoom_boost;

    (true_disc_px * true_disc_px + wide_disc_px * wide_disc_px)
        .sqrt()
        .clamp(4.0, vh * 0.75)
}

/// Picking result for a solar-system body (Sun or Moon) under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickedSolarSystemBody {
    pub body: SolarSystemBody,
    pub ephemeris: SolarSystemEphemeris,
    pub screen_ndc: [f64; 2],
    pub rendered_radius_px: f32,
}

/// Picks the highest-priority solar-system body (Sun or Moon) under `cursor_ndc`.
pub fn pick_solar_system_body(
    view_info: &PlanetariumViewInfo,
    cursor_ndc: (f64, f64),
    viewport_logical: (f64, f64),
    scale_factor: f64,
    occlude_below_horizon: bool,
    occlude_horizon: Option<&HorizonProfile>,
) -> Option<PickedSolarSystemBody> {
    let vw = viewport_logical.0;
    let vh = viewport_logical.1;
    if vw <= 1.0 || vh <= 1.0 {
        return None;
    }

    let safe_scale = scale_factor.max(0.25);
    let fov_y_rad = view_info.projection.fov_y.radians() as f32;
    let physical_h = (vh * safe_scale) as f32;
    let loc = view_info.observer.location;
    let epoch = view_info.observer.epoch;

    let mut best: Option<(f64, PickedSolarSystemBody)> = None;

    for body in SolarSystemBody::ALL {
        let eph = solar_system_ephemeris(body, loc, epoch);
        if occlude_below_horizon && eph.alt_deg < 0.0 {
            continue;
        }
        if let Some(horizon) = occlude_horizon {
            if !horizon.is_visible(eph.az_deg, eph.alt_deg) {
                continue;
            }
        }

        let Some((nx, ny)) = view_info.project_icrs_to_ndc(eph.icrs_dir) else {
            continue;
        };
        if nx.abs() > 1.08 || ny.abs() > 1.08 {
            continue;
        }

        let dx_px = (nx - cursor_ndc.0) * 0.5 * vw;
        let dy_px = (ny - cursor_ndc.1) * 0.5 * vh;
        let dist_px = (dx_px * dx_px + dy_px * dy_px).sqrt();

        let rendered_px = rendered_solar_system_disc_radius_px(
            body,
            eph.angular_radius_deg,
            fov_y_rad,
            physical_h,
        );
        let rendered_logical = f64::from(rendered_px) / safe_scale;
        let hit_radius = (rendered_logical * 1.35 + 6.0).max(11.0);
        if dist_px > hit_radius {
            continue;
        }

        let score = dist_px / hit_radius;
        if best.as_ref().is_none_or(|(prev_score, _)| score < *prev_score) {
            best = Some((
                score,
                PickedSolarSystemBody {
                    body,
                    ephemeris: eph,
                    screen_ndc: [nx, ny],
                    rendered_radius_px: rendered_px,
                },
            ));
        }
    }

    best.map(|(_, picked)| picked)
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialSolarSystemUniforms {
    pub camera_to_topo: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub viewport_and_opacity: Vec4,
    pub sun_cam_and_disc_px: Vec4,
    pub moon_cam_and_disc_px: Vec4,
    pub moon_light_params: Vec4,
    pub render_scales: Vec4,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialSolarSystemMaterial {
    pub uniforms: CelestialSolarSystemUniforms,
}

impl CelestialSolarSystemMaterial {
    pub fn from_camera(
        camera: &PlanetariumCamera,
        projection: ProjectionParams,
        viewport_size: Vec2,
        opacity: f32,
        ground_opacity: f32,
    ) -> Self {
        let loc = camera.observer.location;
        let epoch = camera.observer.epoch;
        let icrs_to_cam = camera.icrs_to_camera_matrix();
        let camera_to_topo =
            DMat4::from_mat3(camera.topocentric_to_camera_matrix().transpose()).as_mat4();

        let sun = solar_ephemeris(loc, epoch);
        let moon = solar_system_ephemeris(SolarSystemBody::Moon, loc, epoch);

        let sun_cam = (icrs_to_cam * sun.icrs_dir).normalize_or_zero();
        let moon_cam = (icrs_to_cam * moon.icrs_dir).normalize_or_zero();

        let sun_disc_px = rendered_solar_system_disc_radius_px(
            SolarSystemBody::Sun,
            sun.angular_radius_deg,
            projection.fov_y,
            viewport_size.y,
        );
        let moon_disc_px = rendered_solar_system_disc_radius_px(
            SolarSystemBody::Moon,
            moon.angular_radius_deg,
            projection.fov_y,
            viewport_size.y,
        );

        // Compute 2D bright-limb screen direction for the Moon from the Sun's direction
        let (limb_x, limb_y) = compute_moon_screen_limb_dir(camera, projection.aspect_ratio, moon_cam, sun_cam);
        let (sin_alpha, cos_alpha) = if let Some(phase) = moon.lunar_phase {
            let (s, c) = phase.phase_angle_deg.to_radians().sin_cos();
            (s as f32, c as f32)
        } else {
            (0.0, 1.0)
        };

        let safe_opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let safe_ground = if ground_opacity.is_finite() {
            ground_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };

        Self {
            uniforms: CelestialSolarSystemUniforms {
                camera_to_topo,
                projection: projection.into(),
                viewport_and_opacity: Vec4::new(
                    viewport_size.x.max(1.0),
                    viewport_size.y.max(1.0),
                    safe_opacity,
                    safe_ground,
                ),
                sun_cam_and_disc_px: Vec4::new(
                    sun_cam.x as f32,
                    sun_cam.y as f32,
                    sun_cam.z as f32,
                    sun_disc_px,
                ),
                moon_cam_and_disc_px: Vec4::new(
                    moon_cam.x as f32,
                    moon_cam.y as f32,
                    moon_cam.z as f32,
                    moon_disc_px,
                ),
                moon_light_params: Vec4::new(limb_x, limb_y, sin_alpha, cos_alpha),
                render_scales: Vec4::new(
                    SUN_QUAD_SCALE,
                    MOON_QUAD_SCALE,
                    sun.alt_deg as f32,
                    0.0,
                ),
            },
        }
    }
}

fn compute_moon_screen_limb_dir(
    camera: &PlanetariumCamera,
    aspect_ratio: f32,
    moon_cam: DVec3,
    sun_cam: DVec3,
) -> (f32, f32) {
    // Tangent vector from Moon toward Sun on the celestial sphere in camera space
    let tangent = sun_cam - moon_cam * moon_cam.dot(sun_cam);
    let t_norm = tangent.normalize_or_zero();
    if t_norm == DVec3::ZERO {
        return (1.0, 0.0);
    }

    let proj = camera.projection(aspect_ratio);
    let eps = 0.01_f64;
    let p_slightly_toward_sun = (moon_cam + t_norm * eps).normalize_or_zero();
    if let (Some((x0, y0)), Some((x1, y1))) = (
        proj.project_cam_dir(moon_cam),
        proj.project_cam_dir(p_slightly_toward_sun),
    ) {
        let dx = (x1 - x0) * f64::from(aspect_ratio);
        let dy = y1 - y0;
        let len = dx.hypot(dy);
        if len > 1e-9 {
            return ((dx / len) as f32, (dy / len) as f32);
        }
    }

    let xy_len = t_norm.x.hypot(t_norm.y);
    if xy_len > 1e-9 {
        ((t_norm.x / xy_len) as f32, (t_norm.y / xy_len) as f32)
    } else {
        (1.0, 0.0)
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialSolarSystemDraw {
    pub mesh: AssetId<Mesh>,
}

impl SyncComponent for CelestialSolarSystemMaterial {
    type Target = (
        CelestialSolarSystemUniforms,
        CelestialSolarSystemDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialSolarSystemMaterial {
    type QueryData = (&'static Self, &'static Mesh3d);
    type QueryFilter = ();
    type Out = (CelestialSolarSystemUniforms, CelestialSolarSystemDraw);

    fn extract_component(
        (material, mesh): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        if material.uniforms.viewport_and_opacity.z <= 1e-4 {
            return None;
        }
        Some((
            material.uniforms,
            CelestialSolarSystemDraw { mesh: mesh.id() },
        ))
    }
}

#[derive(Resource)]
pub struct CelestialSolarSystemPipeline {
    pub layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl CelestialSolarSystemPipeline {
    pub fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "celestial_solar_system_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (uniform_buffer::<CelestialSolarSystemUniforms>(true),),
            ),
        );
        Self { layout, shader }
    }
}

impl SpecializedMeshPipeline for CelestialSolarSystemPipeline {
    type Key = CelestialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = pipeline_descriptor(
            "celestial_solar_system_pipeline",
            &self.layout,
            &self.shader,
            key,
        );
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn solar_system_system(
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    settings: Res<SolarSystemRenderSettings>,
    ground_settings: Res<GroundSettings>,
    mut materials: Query<&mut CelestialSolarSystemMaterial>,
) {
    let Ok(camera) = camera_query.single() else {
        return;
    };
    let aspect = dims.width as f32 / dims.height.max(1) as f32;
    let projection = camera.projection_params(aspect);
    let updated = CelestialSolarSystemMaterial::from_camera(
        camera,
        projection,
        Vec2::new(dims.width as f32, dims.height as f32),
        settings.opacity,
        ground_settings.opacity,
    );
    for mut mat in &mut materials {
        mat.uniforms = updated.uniforms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_skips_solar_system_when_opacity_zero() {
        let cam = PlanetariumCamera::default();
        let proj = cam.projection_params(16.0 / 9.0);
        let mesh = Mesh3d(Handle::default());

        let zero = CelestialSolarSystemMaterial::from_camera(
            &cam,
            proj,
            Vec2::new(1600.0, 900.0),
            0.0,
            1.0,
        );
        assert!(CelestialSolarSystemMaterial::extract_component((&zero, &mesh)).is_none());

        let visible = CelestialSolarSystemMaterial::from_camera(
            &cam,
            proj,
            Vec2::new(1600.0, 900.0),
            0.85,
            1.0,
        );
        assert!(CelestialSolarSystemMaterial::extract_component((&visible, &mesh)).is_some());
    }

    #[test]
    fn picker_selects_sun_when_pointed_at_sun() {
        let mut cam = PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        // Vernal equinox solar noon at 0° lon
        cam.observer.set_epoch_unix_seconds(1_774_008_420.0);
        let sun = solar_ephemeris(cam.observer.location, cam.observer.epoch);
        cam.view_mode = crate::camera::ViewMode::Horizontal;
        cam.yaw = sun.az_deg.to_radians();
        cam.pitch = sun.alt_deg.to_radians();
        cam.target_yaw = cam.yaw;
        cam.target_pitch = cam.pitch;
        cam.fov_y = 60.0_f64.to_radians();
        cam.target_fov_y = cam.fov_y;

        let info = cam.view_info(16.0 / 9.0);
        let picked = pick_solar_system_body(&info, (0.0, 0.0), (1600.0, 900.0), 1.0, true, None)
            .expect("should pick Sun at view center");
        assert_eq!(picked.body, SolarSystemBody::Sun);
    }
}
