use std::path::PathBuf;

use bevy::{
    asset::RenderAssetUsages,
    ecs::query::QueryItem,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    mesh::MeshVertexBufferLayoutRef,
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{
            BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState, Extent3d,
            RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
            SpecializedMeshPipeline, SpecializedMeshPipelineError, TextureDimension, TextureFormat,
            TextureSampleType,
            binding_types::{sampler, texture_2d, uniform_buffer},
        },
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::{
    HorizonProfile, SolarSystemBody, SolarSystemEphemeris, solar_ephemeris, solar_system_ephemeris,
};
use glam::{DMat4, DVec3};
use tracing::{info, warn};

use crate::{
    ViewportDimensions,
    atmosphere::AtmosphereSettings,
    camera::{PlanetariumCamera, PlanetariumViewInfo, ProjectionParams},
    ground::GroundSettings,
    material::{
        CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
    },
};

pub const SUN_QUAD_SCALE: f32 = 6.5;
pub const MOON_QUAD_SCALE: f32 = 2.4;

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

/// GPU image handles for the Moon's equirectangular albedo and tangent-space normal/height textures.
#[derive(Resource, Clone, Debug, Default)]
pub struct MoonTextureHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub has_textures: bool,
}

fn moon_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    })
}

fn discover_texture_path(env_var: &str, rel_path: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(env_path) = std::env::var(env_var) {
        candidates.push(PathBuf::from(env_path));
    }
    candidates.push(PathBuf::from(rel_path));
    candidates.push(PathBuf::from("..").join(rel_path));
    candidates.push(PathBuf::from("../..").join(rel_path));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(rel_path));
            if let Some(file_name) = PathBuf::from(rel_path).file_name() {
                candidates.push(dir.join(file_name));
            }
        }
    }
    candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(rel_path));
    candidates.into_iter().find(|p| p.exists())
}

fn load_rgba_texture(
    images: &mut Assets<Image>,
    env_var: &str,
    rel_path: &str,
    format: TextureFormat,
) -> Option<Handle<Image>> {
    let path = discover_texture_path(env_var, rel_path)?;
    match image::open(&path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            let mut bevy_img = Image::new(
                Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba.into_raw(),
                format,
                RenderAssetUsages::RENDER_WORLD,
            );
            bevy_img.sampler = moon_sampler();
            info!("Loaded Moon texture {}x{} from {:?}", width, height, path);
            Some(images.add(bevy_img))
        }
        Err(err) => {
            warn!("Failed to decode Moon texture at {:?}: {}", path, err);
            None
        }
    }
}

fn create_fallback_texture(
    images: &mut Assets<Image>,
    pixel: [u8; 4],
    format: TextureFormat,
) -> Handle<Image> {
    let mut img = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixel.to_vec(),
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = moon_sampler();
    images.add(img)
}

/// Discovers and uploads NASA LROC albedo and LOLA normal+height Moon textures, falling back
/// to neutral 1x1 placeholders when texture assets are unavailable.
pub fn load_moon_textures(images: &mut Assets<Image>) -> MoonTextureHandles {
    let albedo_opt = load_rgba_texture(
        images,
        "APOGEE_MOON_ALBEDO_PATH",
        "assets/textures/moon_albedo.webp",
        TextureFormat::Rgba8UnormSrgb,
    );
    let normal_opt = load_rgba_texture(
        images,
        "APOGEE_MOON_NORMAL_PATH",
        "assets/textures/moon_normal_height.png",
        TextureFormat::Rgba8Unorm,
    );
    let has_textures = albedo_opt.is_some() && normal_opt.is_some();
    if !has_textures {
        warn!("Moon textures not found on disk; falling back to procedural lunar shading");
    }
    let albedo = albedo_opt.unwrap_or_else(|| {
        create_fallback_texture(images, [220, 215, 205, 255], TextureFormat::Rgba8UnormSrgb)
    });
    let normal = normal_opt.unwrap_or_else(|| {
        create_fallback_texture(images, [128, 128, 255, 128], TextureFormat::Rgba8Unorm)
    });

    MoonTextureHandles {
        albedo,
        normal,
        has_textures,
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
    let fov = fov_y_rad.max(1e-6);
    let vh = viewport_height_px.max(1.0);
    let ang_rad = (angular_radius_deg.to_radians() as f32).max(1e-6);
    let half_fov_tan = (0.5 * fov)
        .min(std::f32::consts::FRAC_PI_2 - 1e-4)
        .tan()
        .max(1e-6);
    let true_disc_px = (ang_rad.tan() / half_fov_tan) * (0.5 * vh);

    let base_min_px = match body {
        SolarSystemBody::Sun => 8.5_f32,
        SolarSystemBody::Moon => 7.8_f32,
    };
    let zoom_boost = (1.8_f32 / fov).powf(0.18).clamp(0.85, 1.75);
    let wide_disc_px = base_min_px * zoom_boost;

    (true_disc_px * true_disc_px + wide_disc_px * wide_disc_px)
        .sqrt()
        .clamp(4.0, vh * 4096.0)
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

        let rendered_px = rendered_solar_system_disc_radius_px(
            body,
            eph.angular_radius_deg,
            fov_y_rad,
            physical_h,
        );
        let rendered_logical = f64::from(rendered_px) / safe_scale;
        if nx.abs() > 1.08 + (2.0 * rendered_logical / vw)
            || ny.abs() > 1.08 + (2.0 * rendered_logical / vh)
        {
            continue;
        }

        let dx_px = (nx - cursor_ndc.0) * 0.5 * vw;
        let dy_px = (ny - cursor_ndc.1) * 0.5 * vh;
        let dist_px = (dx_px * dx_px + dy_px * dy_px).sqrt();

        let hit_radius = (rendered_logical * 1.35 + 6.0).max(11.0);
        if dist_px > hit_radius {
            continue;
        }

        let score = dist_px / hit_radius;
        if best
            .as_ref()
            .is_none_or(|(prev_score, _)| score < *prev_score)
        {
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
    /// x = atmosphere_strength [0..1], y = moon_alt_deg, zw = 2D unit lunar-North direction on Moon disc
    pub atmo_and_moon_north: Vec4,
    /// x = optical libration lon (rad), y = optical libration lat (rad), z = unused, w = unused
    pub moon_libration: Vec4,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialSolarSystemMaterial {
    pub uniforms: CelestialSolarSystemUniforms,
    pub moon_albedo: Handle<Image>,
    pub moon_normal: Handle<Image>,
}

impl CelestialSolarSystemMaterial {
    pub fn from_camera(
        camera: &PlanetariumCamera,
        projection: ProjectionParams,
        viewport_size: Vec2,
        opacity: f32,
        ground_opacity: f32,
        atmosphere_strength: f32,
        moon_textures: &MoonTextureHandles,
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
        let pole_icrs = moon
            .lunar_phase
            .map_or(DVec3::Z, |phase| phase.north_pole_icrs);
        let north_cam = (icrs_to_cam * pole_icrs).normalize_or_zero();

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

        // Compute 2D bright-limb and lunar rotation-axis North screen directions on the Moon disc.
        let (limb_x, limb_y) =
            compute_moon_screen_tangent_dir(camera, projection.aspect_ratio, moon_cam, sun_cam);
        let (north_x, north_y) =
            compute_moon_screen_tangent_dir(camera, projection.aspect_ratio, moon_cam, north_cam);
        let (sin_alpha, cos_alpha, lib_lon_rad, lib_lat_rad) = if let Some(phase) = moon.lunar_phase
        {
            let (s, c) = phase.phase_angle_deg.to_radians().sin_cos();
            (
                s as f32,
                c as f32,
                phase.libration_lon_deg.to_radians() as f32,
                phase.libration_lat_deg.to_radians() as f32,
            )
        } else {
            (0.0, 1.0, 0.0, 0.0)
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
        let safe_atmo = if atmosphere_strength.is_finite() {
            atmosphere_strength.clamp(0.0, 1.0)
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
                    if moon_textures.has_textures { 1.0 } else { 0.0 },
                ),
                atmo_and_moon_north: Vec4::new(safe_atmo, moon.alt_deg as f32, north_x, north_y),
                moon_libration: Vec4::new(lib_lon_rad, lib_lat_rad, 0.0, 0.0),
            },
            moon_albedo: moon_textures.albedo.clone(),
            moon_normal: moon_textures.normal.clone(),
        }
    }
}

fn compute_moon_screen_tangent_dir(
    camera: &PlanetariumCamera,
    aspect_ratio: f32,
    moon_cam: DVec3,
    target_cam: DVec3,
) -> (f32, f32) {
    let tangent = target_cam - moon_cam * moon_cam.dot(target_cam);
    let t_norm = tangent.normalize_or_zero();
    if t_norm == DVec3::ZERO {
        return (0.0, 1.0);
    }

    let proj = camera.projection(aspect_ratio);
    let eps = 0.01_f64;
    let p_step = (moon_cam + t_norm * eps).normalize_or_zero();
    if let (Some((x0, y0)), Some((x1, y1))) =
        (proj.project_cam_dir(moon_cam), proj.project_cam_dir(p_step))
    {
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
        (0.0, 1.0)
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialSolarSystemDraw {
    pub mesh: AssetId<Mesh>,
    pub moon_albedo: AssetId<Image>,
    pub moon_normal: AssetId<Image>,
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
            CelestialSolarSystemDraw {
                mesh: mesh.id(),
                moon_albedo: material.moon_albedo.id(),
                moon_normal: material.moon_normal.id(),
            },
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
                (
                    uniform_buffer::<CelestialSolarSystemUniforms>(true),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
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
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING);
            }
        }
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn solar_system_system(
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    settings: Res<SolarSystemRenderSettings>,
    ground_settings: Res<GroundSettings>,
    atmo_settings: Res<AtmosphereSettings>,
    moon_textures: Res<MoonTextureHandles>,
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
        atmo_settings.strength,
        &moon_textures,
    );
    for mut mat in &mut materials {
        *mat = updated.clone();
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
        let textures = MoonTextureHandles::default();

        let zero = CelestialSolarSystemMaterial::from_camera(
            &cam,
            proj,
            Vec2::new(1600.0, 900.0),
            0.0,
            1.0,
            1.0,
            &textures,
        );
        assert!(CelestialSolarSystemMaterial::extract_component((&zero, &mesh)).is_none());

        let visible = CelestialSolarSystemMaterial::from_camera(
            &cam,
            proj,
            Vec2::new(1600.0, 900.0),
            0.85,
            1.0,
            1.0,
            &textures,
        );
        assert!(CelestialSolarSystemMaterial::extract_component((&visible, &mesh)).is_some());
    }

    #[test]
    fn moon_north_dir_is_unit_and_atmo_clamped() {
        let cam = PlanetariumCamera::default();
        let proj = cam.projection_params(16.0 / 9.0);
        let textures = MoonTextureHandles::default();
        let mat = CelestialSolarSystemMaterial::from_camera(
            &cam,
            proj,
            Vec2::new(1600.0, 900.0),
            1.0,
            1.0,
            1.5,
            &textures,
        );
        let north = Vec2::new(
            mat.uniforms.atmo_and_moon_north.z,
            mat.uniforms.atmo_and_moon_north.w,
        );
        assert!((north.length() - 1.0).abs() < 1e-4);
        assert_eq!(mat.uniforms.atmo_and_moon_north.x, 1.0);
        assert_eq!(mat.uniforms.render_scales.w, 0.0);
    }

    #[test]
    fn bundled_moon_textures_exist_and_decode_cleanly() {
        let albedo_path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/textures/moon_albedo.webp"
        ));
        let normal_path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/textures/moon_normal_height.png"
        ));

        let albedo = image::open(&albedo_path)
            .expect("open bundled moon_albedo.webp")
            .to_rgba8();
        let normal = image::open(&normal_path)
            .expect("open bundled moon_normal_height.png")
            .to_rgba8();

        assert_eq!(albedo.dimensions(), (2048, 1024));
        assert_eq!(normal.dimensions(), (2048, 1024));

        // Verify normal map z-component averages near +1.0 (tangent-space up) and elevation varies.
        let mut min_h = 255_u8;
        let mut max_h = 0_u8;
        let mut nz_sum = 0_u64;
        let mut px_count = 0_u64;
        for px in normal.pixels().step_by(97) {
            min_h = min_h.min(px[3]);
            max_h = max_h.max(px[3]);
            nz_sum += u64::from(px[2]);
            px_count += 1;
        }
        assert!(max_h > min_h + 150, "expected full height dynamic range");
        assert!(
            nz_sum / px_count > 180,
            "expected positive tangent-space normal z"
        );
    }

    #[test]
    fn disc_radius_scales_continuously_past_22_arcminutes() {
        let vh = 1000.0_f32;
        let ang_radius_deg = 0.258_f64; // ~15.5 arcminutes (Moon)
        let fov_20_arcmin = (20.0_f32 / 60.0).to_radians();
        let fov_10_arcmin = (10.0_f32 / 60.0).to_radians();
        let fov_2_arcmin = (2.0_f32 / 60.0).to_radians();

        let r_20 = rendered_solar_system_disc_radius_px(
            SolarSystemBody::Moon,
            ang_radius_deg,
            fov_20_arcmin,
            vh,
        );
        let r_10 = rendered_solar_system_disc_radius_px(
            SolarSystemBody::Moon,
            ang_radius_deg,
            fov_10_arcmin,
            vh,
        );
        let r_2 = rendered_solar_system_disc_radius_px(
            SolarSystemBody::Moon,
            ang_radius_deg,
            fov_2_arcmin,
            vh,
        );

        assert!(r_20 > vh * 0.75, "expected > 0.75*vh at 20', got {r_20}");
        assert!(
            (r_10 / r_20 - 2.0).abs() < 0.05,
            "expected ~2x radius at 10' vs 20', got {} / {}",
            r_10,
            r_20
        );
        assert!(
            (r_2 / r_20 - 10.0).abs() < 0.2,
            "expected ~10x radius at 2' vs 20', got {} / {}",
            r_2,
            r_20
        );
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
