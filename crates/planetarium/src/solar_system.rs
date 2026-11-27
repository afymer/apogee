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
            SpecializedMeshPipeline, SpecializedMeshPipelineError, TextureDataOrder,
            TextureDimension, TextureFormat, TextureSampleType, TextureViewDescriptor,
            TextureViewDimension,
            binding_types::{sampler, texture_2d, texture_2d_array, uniform_buffer},
        },
        sync_component::SyncComponent,
    },
    shader::Shader,
};
use celestial::{
    HorizonProfile, ObservationContext, SolarSystemBody, SolarSystemEphemeris, solar_ephemeris,
    solar_system_ephemeris,
};
use glam::{DMat4, DVec3};
use tracing::{info, warn};

use crate::{
    BevyBridge, ViewportDimensions,
    atmosphere::AtmosphereSettings,
    camera::{PlanetariumCamera, PlanetariumViewInfo, ProjectionParams},
    ground::GroundSettings,
    material::{
        CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
    },
    preload::BridgeWake,
};

pub const SUN_QUAD_SCALE: f32 = 6.5;
pub const MOON_QUAD_SCALE: f32 = 2.4;

/// Number of point-like bodies (planets and satellites) drawn by the solar-system layer.
pub const POINT_BODY_COUNT: usize = SolarSystemBody::POINT_BODIES.len();

/// Minimum on-screen separation (physical px) between a satellite and its planet for the
/// satellite to be drawn and pickable; closer than this it is lost in the planet's glow.
pub const SATELLITE_MIN_SEPARATION_PX: f32 = 6.0;

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

/// GPU image handles for the Moon's equirectangular albedo and tangent-space normal/height
/// textures, plus the planet surface-texture array and ring-profile array.
#[derive(Resource, Clone, Debug, Default)]
pub struct MoonTextureHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub has_textures: bool,
    /// Mipmapped `texture_2d_array` of equirectangular planet albedo maps, indexed by point slot
    /// (Mercury, Venus, Mars, Jupiter, Saturn, Uranus, Neptune).
    pub planet_albedo: Handle<Image>,
    /// Mipmapped `texture_2d_array` of radial ring profiles (see [`ring_spec`]).
    pub ring_profiles: Handle<Image>,
    pub has_planet_textures: bool,
}

/// Number of planets with a surface texture; they occupy the first point slots.
pub const TEXTURED_PLANET_COUNT: usize = 7;

/// Ring system of a planet: radial extent in planet equatorial radii and profile-array layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RingSpec {
    pub inner_radii: f32,
    pub outer_radii: f32,
    pub layer: u32,
}

/// Saturn's ring profile strip spans 74,500 km (C ring) to 140,220 km (F ring).
const SATURN_RING_INNER_KM: f32 = 74_500.0;
const SATURN_RING_OUTER_KM: f32 = 140_220.0;
const SATURN_RADIUS_KM: f32 = 60_268.0;
/// Uranus' generated ring strip spans 40,000 km to 52,500 km from the planet's center.
const URANUS_RING_INNER_KM: f32 = 40_000.0;
const URANUS_RING_OUTER_KM: f32 = 52_500.0;
const URANUS_RADIUS_KM: f32 = 25_559.0;

/// Rings that are rendered. Jupiter's and Neptune's rings are far too faint to be visible.
pub fn ring_spec(body: SolarSystemBody) -> Option<RingSpec> {
    match body {
        SolarSystemBody::Saturn => Some(RingSpec {
            inner_radii: SATURN_RING_INNER_KM / SATURN_RADIUS_KM,
            outer_radii: SATURN_RING_OUTER_KM / SATURN_RADIUS_KM,
            layer: 0,
        }),
        SolarSystemBody::Uranus => Some(RingSpec {
            inner_radii: URANUS_RING_INNER_KM / URANUS_RADIUS_KM,
            outer_radii: URANUS_RING_OUTER_KM / URANUS_RADIUS_KM,
            layer: 1,
        }),
        _ => None,
    }
}

const RING_STRIP_WIDTH: u32 = 2048;
const RING_STRIP_HEIGHT: u32 = 16;

/// Uranus' narrow dark rings (6, 5, 4, alpha, beta, eta, gamma, delta, epsilon) as
/// `(radius_km, width_km, opacity)`.
const URANUS_RINGS: [(f32, f32, f32); 9] = [
    (41_837.0, 2.0, 0.55),
    (42_234.0, 3.0, 0.55),
    (42_570.0, 3.0, 0.55),
    (44_718.0, 10.0, 0.6),
    (45_661.0, 9.0, 0.6),
    (47_176.0, 3.0, 0.5),
    (47_627.0, 3.0, 0.5),
    (48_300.0, 4.0, 0.55),
    (51_149.0, 40.0, 0.75),
];

fn generate_uranus_ring_strip() -> image::RgbaImage {
    let km_per_px = (URANUS_RING_OUTER_KM - URANUS_RING_INNER_KM) / RING_STRIP_WIDTH as f32;
    image::RgbaImage::from_fn(RING_STRIP_WIDTH, RING_STRIP_HEIGHT, |x, _| {
        let r_km = URANUS_RING_INNER_KM + (x as f32 + 0.5) * km_per_px;
        let opacity = URANUS_RINGS
            .iter()
            .map(|&(center, width, o)| {
                let half = 0.5 * width.max(km_per_px * 1.5);
                let t = ((r_km - center).abs() / half).min(1.0);
                o * (1.0 - t * t)
            })
            .fold(0.0_f32, f32::max);
        image::Rgba([70, 70, 74, (opacity * 255.0) as u8])
    })
}

/// Downsamples an RGBA8 image by 2x along each axis > 1 using an exact 2x2 (or 2x1 / 1x2) box filter.
fn downsample_rgba_2x(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let nw = (width / 2).max(1);
    let nh = (height / 2).max(1);
    let w = width as usize;
    let h = height as usize;
    let nw_usize = nw as usize;
    let nh_usize = nh as usize;
    let mut dst = vec![0u8; nw_usize * nh_usize * 4];

    if w >= 2 && h >= 2 {
        let src_stride = w * 4;
        let dst_stride = nw_usize * 4;
        for y in 0..nh_usize {
            let row0 = &src[(2 * y) * src_stride..(2 * y + 1) * src_stride];
            let row1 = &src[(2 * y + 1) * src_stride..(2 * y + 2) * src_stride];
            let dst_row = &mut dst[y * dst_stride..(y + 1) * dst_stride];
            let (r0_chunks, _) = row0.as_chunks::<8>();
            let (r1_chunks, _) = row1.as_chunks::<8>();
            let (dst_chunks, _) = dst_row.as_chunks_mut::<4>();
            for ((p0, p1), out) in r0_chunks.iter().zip(r1_chunks).zip(dst_chunks) {
                out[0] = ((u16::from(p0[0])
                    + u16::from(p0[4])
                    + u16::from(p1[0])
                    + u16::from(p1[4])
                    + 2)
                    >> 2) as u8;
                out[1] = ((u16::from(p0[1])
                    + u16::from(p0[5])
                    + u16::from(p1[1])
                    + u16::from(p1[5])
                    + 2)
                    >> 2) as u8;
                out[2] = ((u16::from(p0[2])
                    + u16::from(p0[6])
                    + u16::from(p1[2])
                    + u16::from(p1[6])
                    + 2)
                    >> 2) as u8;
                out[3] = ((u16::from(p0[3])
                    + u16::from(p0[7])
                    + u16::from(p1[3])
                    + u16::from(p1[7])
                    + 2)
                    >> 2) as u8;
            }
        }
    } else if w >= 2 {
        let (src_chunks, _) = src.as_chunks::<8>();
        let (dst_chunks, _) = dst.as_chunks_mut::<4>();
        for (p, out) in src_chunks.iter().zip(dst_chunks) {
            out[0] = ((u16::from(p[0]) + u16::from(p[4]) + 1) >> 1) as u8;
            out[1] = ((u16::from(p[1]) + u16::from(p[5]) + 1) >> 1) as u8;
            out[2] = ((u16::from(p[2]) + u16::from(p[6]) + 1) >> 1) as u8;
            out[3] = ((u16::from(p[3]) + u16::from(p[7]) + 1) >> 1) as u8;
        }
    } else if h >= 2 {
        for y in 0..nh_usize {
            let p0 = &src[(2 * y) * 4..(2 * y + 1) * 4];
            let p1 = &src[(2 * y + 1) * 4..(2 * y + 2) * 4];
            let out = &mut dst[y * 4..(y + 1) * 4];
            out[0] = ((u16::from(p0[0]) + u16::from(p1[0]) + 1) >> 1) as u8;
            out[1] = ((u16::from(p0[1]) + u16::from(p1[1]) + 1) >> 1) as u8;
            out[2] = ((u16::from(p0[2]) + u16::from(p1[2]) + 1) >> 1) as u8;
            out[3] = ((u16::from(p0[3]) + u16::from(p1[3]) + 1) >> 1) as u8;
        }
    } else {
        let len = dst.len();
        dst.copy_from_slice(&src[..len]);
    }

    (dst, nw, nh)
}

fn mip_chain_byte_len(width: u32, height: u32, mip_count: u32) -> usize {
    (0..mip_count)
        .map(|m| {
            let w = (width as usize >> m).max(1);
            let h = (height as usize >> m).max(1);
            w * h * 4
        })
        .sum()
}

fn build_layer_mip_chain(layer: &image::RgbaImage, mip_count: u32) -> Vec<u8> {
    let (mut w, mut h) = layer.dimensions();
    let mut out = Vec::with_capacity(mip_chain_byte_len(w, h, mip_count));
    out.extend_from_slice(layer.as_raw());
    if mip_count <= 1 {
        return out;
    }
    let (mut level, nw, nh) = downsample_rgba_2x(layer.as_raw(), w, h);
    w = nw;
    h = nh;
    for mip in 1..mip_count {
        out.extend_from_slice(&level);
        if mip + 1 < mip_count {
            let (next, nw, nh) = downsample_rgba_2x(&level, w, h);
            level = next;
            w = nw;
            h = nh;
        }
    }
    out
}

/// Builds a mipmapped `texture_2d_array` image from equally sized RGBA layers.
fn build_mipmapped_array_image(layers: &[image::RgbaImage]) -> Image {
    let (width, height) = layers[0].dimensions();
    let mip_count = 32 - width.max(height).leading_zeros();
    let per_layer = mip_chain_byte_len(width, height, mip_count);
    let layer_chains: Vec<Vec<u8>> = if layers.len() > 1 && per_layer > 4096 {
        std::thread::scope(|s| {
            let handles: Vec<_> = layers
                .iter()
                .map(|layer| s.spawn(move || build_layer_mip_chain(layer, mip_count)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("mipmap worker panicked"))
                .collect()
        })
    } else {
        layers
            .iter()
            .map(|layer| build_layer_mip_chain(layer, mip_count))
            .collect()
    };

    let mut data = Vec::with_capacity(per_layer * layers.len());
    for chain in layer_chains {
        data.extend_from_slice(&chain);
    }

    let mut img = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: layers.len() as u32,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.data_order = TextureDataOrder::LayerMajor;
    img.texture_descriptor.mip_level_count = mip_count;
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..Default::default()
    });
    img.sampler = moon_sampler();
    img
}

fn load_rgba_image(env_var: &str, rel_path: &str) -> Option<image::RgbaImage> {
    let path = discover_texture_path(env_var, rel_path)?;
    match image::open(&path) {
        Ok(img) => Some(img.to_rgba8()),
        Err(err) => {
            warn!("Failed to decode planet texture at {:?}: {}", path, err);
            None
        }
    }
}

const PLANET_TEXTURE_FILES: [(&str, &str); TEXTURED_PLANET_COUNT] = [
    (
        "APOGEE_MERCURY_TEXTURE_PATH",
        "assets/textures/planets/mercury.jpg",
    ),
    (
        "APOGEE_VENUS_TEXTURE_PATH",
        "assets/textures/planets/venus_atmosphere.jpg",
    ),
    (
        "APOGEE_MARS_TEXTURE_PATH",
        "assets/textures/planets/mars.jpg",
    ),
    (
        "APOGEE_JUPITER_TEXTURE_PATH",
        "assets/textures/planets/jupiter.jpg",
    ),
    (
        "APOGEE_SATURN_TEXTURE_PATH",
        "assets/textures/planets/saturn.jpg",
    ),
    (
        "APOGEE_URANUS_TEXTURE_PATH",
        "assets/textures/planets/uranus.jpg",
    ),
    (
        "APOGEE_NEPTUNE_TEXTURE_PATH",
        "assets/textures/planets/neptune.jpg",
    ),
];

/// Decodes the planet albedo array and ring profiles into GPU-ready `Image`s in parallel.
/// Returns `None` when any file is missing.
fn load_planet_array_images() -> Option<(Image, Image)> {
    let (planet_results, ring_result) = std::thread::scope(|s| {
        let planet_handles: Vec<_> = PLANET_TEXTURE_FILES
            .iter()
            .map(|&(env_var, rel_path)| {
                s.spawn(move || {
                    let mut img = load_rgba_image(env_var, rel_path)?;
                    if img.dimensions() != (2048, 1024) {
                        img = image::imageops::resize(
                            &img,
                            2048,
                            1024,
                            image::imageops::FilterType::Triangle,
                        );
                    }
                    Some(img)
                })
            })
            .collect();
        let ring_handle = s.spawn(|| {
            let saturn_ring = load_rgba_image(
                "APOGEE_SATURN_RING_TEXTURE_PATH",
                "assets/textures/planets/saturn_ring_alpha.png",
            )?;
            let saturn_strip = image::imageops::resize(
                &saturn_ring,
                RING_STRIP_WIDTH,
                RING_STRIP_HEIGHT,
                image::imageops::FilterType::Triangle,
            );
            let rings = [saturn_strip, generate_uranus_ring_strip()];
            Some(build_mipmapped_array_image(&rings))
        });

        let planets: Vec<Option<image::RgbaImage>> = planet_handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect();
        let rings = ring_handle.join().ok().flatten();
        (planets, rings)
    });

    let planets: Option<Vec<image::RgbaImage>> = planet_results.into_iter().collect();
    let planets = planets?;
    let ring_image = ring_result?;
    Some((build_mipmapped_array_image(&planets), ring_image))
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

fn decode_rgba_bevy_image(env_var: &str, rel_path: &str, format: TextureFormat) -> Option<Image> {
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
            Some(bevy_img)
        }
        Err(err) => {
            warn!("Failed to decode Moon texture at {:?}: {}", path, err);
            None
        }
    }
}

fn load_moon_image_pair() -> Option<(Image, Image)> {
    let (albedo, normal) = std::thread::scope(|s| {
        let a = s.spawn(|| {
            decode_rgba_bevy_image(
                "APOGEE_MOON_ALBEDO_PATH",
                "assets/textures/moon_albedo.webp",
                TextureFormat::Rgba8UnormSrgb,
            )
        });
        let n = s.spawn(|| {
            decode_rgba_bevy_image(
                "APOGEE_MOON_NORMAL_PATH",
                "assets/textures/moon_normal_height.png",
                TextureFormat::Rgba8Unorm,
            )
        });
        (a.join().ok().flatten(), n.join().ok().flatten())
    });
    Some((albedo?, normal?))
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

/// Creates lightweight 1x1 placeholder textures immediately so the planetarium can render its
/// first frame without waiting for disk I/O, image decoding, or mipmap generation.
pub fn fallback_moon_textures(images: &mut Assets<Image>) -> MoonTextureHandles {
    let albedo =
        create_fallback_texture(images, [220, 215, 205, 255], TextureFormat::Rgba8UnormSrgb);
    let normal = create_fallback_texture(images, [128, 128, 255, 128], TextureFormat::Rgba8Unorm);
    let blank = [image::RgbaImage::new(1, 1)];
    let planet_albedo = images.add(build_mipmapped_array_image(&blank));
    let ring_profiles = images.add(build_mipmapped_array_image(&blank));
    MoonTextureHandles {
        albedo,
        normal,
        has_textures: false,
        planet_albedo,
        ring_profiles,
        has_planet_textures: false,
    }
}

enum LoadedSolarSystemTextures {
    Moon {
        albedo: Image,
        normal: Image,
    },
    Planets {
        planet_albedo: Image,
        ring_profiles: Image,
    },
}

/// Loads Moon and planet surface textures off the renderer thread and notifies [`BevyBridge`]
/// when each texture set is ready to swap in.
#[derive(Resource)]
pub struct SolarSystemTextureLoader {
    rx: Option<crossbeam_channel::Receiver<LoadedSolarSystemTextures>>,
    wake: std::sync::Arc<BridgeWake>,
}

impl SolarSystemTextureLoader {
    /// Starts asynchronous Moon & planet texture loading without a [`BevyBridge`] yet.
    /// Call [`Self::attach_bridge`] once [`crate::PlanetariumRenderer`] creates its bridge.
    pub fn spawn_preloading() -> Self {
        Self::spawn_inner(None)
    }

    pub fn spawn(bridge: BevyBridge) -> Self {
        Self::spawn_inner(Some(bridge))
    }

    fn spawn_inner(bridge: Option<BevyBridge>) -> Self {
        let (tx, rx) = crossbeam_channel::bounded(2);
        let wake = std::sync::Arc::new(BridgeWake::new(bridge));
        let wake_thread = std::sync::Arc::clone(&wake);
        let spawn_result = std::thread::Builder::new()
            .name("solar-system-textures".to_string())
            .spawn(move || {
                std::thread::scope(|s| {
                    let tx_moon = tx.clone();
                    let wake_moon = std::sync::Arc::clone(&wake_thread);
                    s.spawn(move || {
                        let start = std::time::Instant::now();
                        if let Some((albedo, normal)) = load_moon_image_pair() {
                            info!(
                                "Decoded Moon textures asynchronously in {:.1}ms",
                                start.elapsed().as_secs_f64() * 1000.0
                            );
                            if tx_moon
                                .send(LoadedSolarSystemTextures::Moon { albedo, normal })
                                .is_ok()
                            {
                                wake_moon.request_frame();
                            }
                        } else {
                            warn!(
                                "Moon textures not found on disk; falling back to procedural lunar shading"
                            );
                        }
                    });

                    let tx_planets = tx;
                    let wake_planets = wake_thread;
                    s.spawn(move || {
                        let start = std::time::Instant::now();
                        if let Some((planet_albedo, ring_profiles)) = load_planet_array_images() {
                            info!(
                                "Decoded & mipmapped planet textures asynchronously in {:.1}ms",
                                start.elapsed().as_secs_f64() * 1000.0
                            );
                            if tx_planets
                                .send(LoadedSolarSystemTextures::Planets {
                                    planet_albedo,
                                    ring_profiles,
                                })
                                .is_ok()
                            {
                                wake_planets.request_frame();
                            }
                        } else {
                            warn!(
                                "Planet textures not found on disk; planets fall back to flat shading"
                            );
                        }
                    });
                });
            });

        if let Err(err) = spawn_result {
            warn!("Failed to spawn solar-system texture loader thread: {err}");
            return Self { rx: None, wake };
        }
        Self { rx: Some(rx), wake }
    }

    /// Attaches [`BevyBridge`] so subsequent background completions wake the renderer.
    pub fn attach_bridge(&self, bridge: BevyBridge) {
        self.wake.attach(bridge);
    }

    /// Drains any completed texture sets into [`MoonTextureHandles`].
    pub fn apply_ready(
        &mut self,
        images: &mut Assets<Image>,
        moon_textures: &mut MoonTextureHandles,
    ) {
        let mut disconnected = false;
        if let Some(rx) = self.rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(LoadedSolarSystemTextures::Moon { albedo, normal }) => {
                        moon_textures.albedo = images.add(albedo);
                        moon_textures.normal = images.add(normal);
                        moon_textures.has_textures = true;
                    }
                    Ok(LoadedSolarSystemTextures::Planets {
                        planet_albedo,
                        ring_profiles,
                    }) => {
                        moon_textures.planet_albedo = images.add(planet_albedo);
                        moon_textures.ring_profiles = images.add(ring_profiles);
                        moon_textures.has_planet_textures = true;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.rx = None;
        }
    }
}

/// Polls completed background texture loads and swaps them into [`MoonTextureHandles`].
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn poll_solar_system_textures_system(
    mut loader: ResMut<SolarSystemTextureLoader>,
    mut images: ResMut<Assets<Image>>,
    mut moon_textures: ResMut<MoonTextureHandles>,
) {
    loader.apply_ready(&mut images, &mut moon_textures);
}

/// Discovers and uploads NASA LROC albedo and LOLA normal+height Moon textures plus planet and
/// ring arrays synchronously, falling back to neutral 1x1 placeholders when assets are unavailable.
pub fn load_moon_textures(images: &mut Assets<Image>) -> MoonTextureHandles {
    let (moon_pair, planet_arrays) = std::thread::scope(|s| {
        let m = s.spawn(load_moon_image_pair);
        let p = s.spawn(load_planet_array_images);
        (m.join().ok().flatten(), p.join().ok().flatten())
    });

    let mut handles = fallback_moon_textures(images);
    if let Some((albedo, normal)) = moon_pair {
        handles.albedo = images.add(albedo);
        handles.normal = images.add(normal);
        handles.has_textures = true;
    } else {
        warn!("Moon textures not found on disk; falling back to procedural lunar shading");
    }

    if let Some((planet_albedo, ring_profiles)) = planet_arrays {
        handles.planet_albedo = images.add(planet_albedo);
        handles.ring_profiles = images.add(ring_profiles);
        handles.has_planet_textures = true;
    } else {
        warn!("Planet textures not found on disk; planets fall back to flat shading");
    }

    handles
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

    let (base_min_px, floor_px) = match body {
        SolarSystemBody::Sun => (8.5_f32, 4.0_f32),
        SolarSystemBody::Moon => (7.8, 4.0),
        _ if body.is_planet() => (1.6, 1.2),
        _ => (1.2, 1.0),
    };
    let zoom_boost = (1.8_f32 / fov).powf(0.18).clamp(0.85, 1.75);
    let wide_disc_px = base_min_px * zoom_boost;

    (true_disc_px * true_disc_px + wide_disc_px * wide_disc_px)
        .sqrt()
        .clamp(floor_px, vh * 4096.0)
}

/// On-screen separation in physical pixels between a satellite and its parent planet.
pub fn satellite_separation_px(
    satellite: &SolarSystemEphemeris,
    parent: &SolarSystemEphemeris,
    fov_y_rad: f32,
    viewport_height_px: f32,
) -> f32 {
    let sep_rad = satellite
        .icrs_dir
        .dot(parent.icrs_dir)
        .clamp(-1.0, 1.0)
        .acos() as f32;
    let half_fov_tan = (0.5 * fov_y_rad.max(1e-6))
        .min(std::f32::consts::FRAC_PI_2 - 1e-4)
        .tan()
        .max(1e-6);
    (sep_rad.tan() / half_fov_tan) * (0.5 * viewport_height_px.max(1.0))
}

/// Returns `true` when `body` should be drawn/picked at the current zoom. Satellites collapse
/// into their planet's glow at wide fields of view and are hidden there.
pub fn point_body_resolvable(
    body: SolarSystemBody,
    eph: &SolarSystemEphemeris,
    observer: &ObservationContext,
    fov_y_rad: f32,
    viewport_height_px: f32,
) -> bool {
    let Some(parent) = body.parent() else {
        return true;
    };
    let parent_eph = solar_system_ephemeris(parent, observer.location, observer.epoch);
    satellite_separation_px(eph, &parent_eph, fov_y_rad, viewport_height_px)
        >= SATELLITE_MIN_SEPARATION_PX
}

/// Glow strength `[0.18, 1.0]` of a point-like body derived from its apparent magnitude.
pub fn point_body_strength(apparent_magnitude: f32) -> f32 {
    10.0_f32
        .powf(-0.2 * (apparent_magnitude + 1.0))
        .clamp(0.18, 1.0)
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
        if !point_body_resolvable(body, &eph, &view_info.observer, fov_y_rad, physical_h) {
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
    /// x = optical libration lon (rad), y = optical libration lat (rad), z = has_planet_textures (0 or 1), w = unused
    pub moon_libration: Vec4,
    /// Per point-like body (planets and satellites): xyz = unit camera-space direction,
    /// w = disc radius in physical px (`0` hides the body).
    pub point_cam_and_disc_px: [Vec4; POINT_BODY_COUNT],
    /// Per point-like body: rgb = linear-ish sRGB tint, w = glow strength `[0, 1]`.
    pub point_tint_and_strength: [Vec4; POINT_BODY_COUNT],
    /// Per point-like body: xy = 2D unit bright-limb direction, z = sin(phase angle), w = cos(phase angle).
    pub point_light: [Vec4; POINT_BODY_COUNT],
    /// Per point-like body: xy = 2D unit screen direction of the rotation-axis north pole,
    /// z = sub-observer longitude (rad, east positive), w = sub-observer latitude (rad).
    pub point_axis: [Vec4; POINT_BODY_COUNT],
    /// Per point-like body: x = ring inner radius, y = ring outer radius (planet radii, `0` = no
    /// rings), z = ring profile array layer.
    pub point_ring: [Vec4; POINT_BODY_COUNT],
}

#[derive(Component, Debug, Clone)]
pub struct CelestialSolarSystemMaterial {
    pub uniforms: CelestialSolarSystemUniforms,
    pub moon_albedo: Handle<Image>,
    pub moon_normal: Handle<Image>,
    pub planet_albedo: Handle<Image>,
    pub ring_profiles: Handle<Image>,
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

        let fov_y = projection.fov_y;
        let mut point_cam_and_disc_px = [Vec4::ZERO; POINT_BODY_COUNT];
        let mut point_tint_and_strength = [Vec4::ZERO; POINT_BODY_COUNT];
        let mut point_light = [Vec4::new(0.0, 1.0, 0.0, 1.0); POINT_BODY_COUNT];
        let mut point_axis = [Vec4::new(0.0, 1.0, 0.0, 0.0); POINT_BODY_COUNT];
        let mut point_ring = [Vec4::ZERO; POINT_BODY_COUNT];
        for (slot, body) in SolarSystemBody::POINT_BODIES.iter().copied().enumerate() {
            let eph = solar_system_ephemeris(body, loc, epoch);
            let cam = (icrs_to_cam * eph.icrs_dir).normalize_or_zero();
            let resolvable =
                point_body_resolvable(body, &eph, &camera.observer, fov_y, viewport_size.y);
            let disc_px = if resolvable {
                rendered_solar_system_disc_radius_px(
                    body,
                    eph.angular_radius_deg,
                    fov_y,
                    viewport_size.y,
                )
            } else {
                0.0
            };
            let srgb = body.srgb();
            point_cam_and_disc_px[slot] =
                Vec4::new(cam.x as f32, cam.y as f32, cam.z as f32, disc_px);
            point_tint_and_strength[slot] = Vec4::new(
                srgb[0],
                srgb[1],
                srgb[2],
                point_body_strength(eph.apparent_magnitude),
            );
            let sun_dir_cam = (icrs_to_cam * eph.sun_icrs_dir).normalize_or_zero();
            let (lx, ly) =
                compute_moon_screen_tangent_dir(camera, projection.aspect_ratio, cam, sun_dir_cam);
            let cos_alpha = (2.0 * eph.illuminated_fraction - 1.0).clamp(-1.0, 1.0);
            let sin_alpha = (1.0 - cos_alpha * cos_alpha).max(0.0).sqrt();
            point_light[slot] = Vec4::new(lx, ly, sin_alpha as f32, cos_alpha as f32);

            if let Some(orientation) = eph.orientation(epoch) {
                let pole_cam = (icrs_to_cam * orientation.north_pole_icrs).normalize_or_zero();
                let (nx, ny) =
                    compute_moon_screen_tangent_dir(camera, projection.aspect_ratio, cam, pole_cam);
                point_axis[slot] = Vec4::new(
                    nx,
                    ny,
                    orientation.sub_observer_lon_deg.to_radians() as f32,
                    orientation.sub_observer_lat_deg.to_radians() as f32,
                );
            }
            if let Some(ring) = ring_spec(body) {
                point_ring[slot] =
                    Vec4::new(ring.inner_radii, ring.outer_radii, ring.layer as f32, 0.0);
            }
        }

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
                moon_libration: Vec4::new(
                    lib_lon_rad,
                    lib_lat_rad,
                    if moon_textures.has_planet_textures {
                        1.0
                    } else {
                        0.0
                    },
                    0.0,
                ),
                point_cam_and_disc_px,
                point_tint_and_strength,
                point_light,
                point_axis,
                point_ring,
            },
            moon_albedo: moon_textures.albedo.clone(),
            moon_normal: moon_textures.normal.clone(),
            planet_albedo: moon_textures.planet_albedo.clone(),
            ring_profiles: moon_textures.ring_profiles.clone(),
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
    pub planet_albedo: AssetId<Image>,
    pub ring_profiles: AssetId<Image>,
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
                planet_albedo: material.planet_albedo.id(),
                ring_profiles: material.ring_profiles.id(),
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
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
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
    fn bundled_planet_textures_load_into_mipmapped_arrays() {
        let mut images = Assets::<Image>::default();
        let handles = load_moon_textures(&mut images);
        assert!(
            handles.has_planet_textures,
            "planet textures must be bundled"
        );
        let planets = images.get(&handles.planet_albedo).expect("planet array");
        assert_eq!(
            planets.texture_descriptor.size.depth_or_array_layers as usize,
            TEXTURED_PLANET_COUNT
        );
        assert_eq!(planets.texture_descriptor.mip_level_count, 12);
        // Each layer stores its full mip chain: 2048x1024 down to 1x1.
        let per_layer: usize = (0..12)
            .map(|m| ((2048usize >> m).max(1)) * ((1024usize >> m).max(1)) * 4)
            .sum();
        assert_eq!(
            planets.data.as_ref().map(Vec::len),
            Some(per_layer * TEXTURED_PLANET_COUNT)
        );
        let rings = images.get(&handles.ring_profiles).expect("ring array");
        assert_eq!(rings.texture_descriptor.size.depth_or_array_layers, 2);
    }

    #[test]
    fn downsample_rgba_2x_averages_2x2_and_1d_blocks() {
        // 2x2 -> 1x1
        let src_2x2: [u8; 16] = [
            10, 20, 30, 40, // (0,0)
            50, 60, 70, 80, // (1,0)
            90, 100, 110, 120, // (0,1)
            130, 140, 150, 160, // (1,1)
        ];
        let (out, nw, nh) = downsample_rgba_2x(&src_2x2, 2, 2);
        assert_eq!((nw, nh), (1, 1));
        assert_eq!(out, vec![70, 80, 90, 100]);

        // 4x1 -> 2x1
        let src_4x1: [u8; 16] = [
            0, 100, 200, 255, // (0,0)
            100, 200, 100, 255, // (1,0)
            20, 40, 60, 80, // (2,0)
            40, 60, 80, 100, // (3,0)
        ];
        let (out, nw, nh) = downsample_rgba_2x(&src_4x1, 4, 1);
        assert_eq!((nw, nh), (2, 1));
        assert_eq!(out, vec![50, 150, 150, 255, 30, 50, 70, 90]);

        // 1x2 -> 1x1
        let src_1x2: [u8; 8] = [10, 20, 30, 40, 30, 40, 50, 60];
        let (out, nw, nh) = downsample_rgba_2x(&src_1x2, 1, 2);
        assert_eq!((nw, nh), (1, 1));
        assert_eq!(out, vec![20, 30, 40, 50]);
    }

    #[test]
    fn fallback_textures_are_instant_and_async_loader_populates_handles() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let t0 = std::time::Instant::now();
        let fallback = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            fallback_moon_textures(&mut images)
        };
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(50),
            "fallback texture init must be instant"
        );
        assert!(!fallback.has_textures);
        assert!(!fallback.has_planet_textures);

        let bridge = BevyBridge::new(|| {});
        app.insert_resource(fallback);
        app.insert_resource(SolarSystemTextureLoader::spawn(bridge));
        app.add_systems(Update, poll_solar_system_textures_system);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            let handles = app.world().resource::<MoonTextureHandles>();
            if handles.has_textures && handles.has_planet_textures {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for async solar-system textures"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let handles = app.world().resource::<MoonTextureHandles>().clone();
        let images = app.world().resource::<Assets<Image>>();
        let planets = images.get(&handles.planet_albedo).expect("planet array");
        assert_eq!(
            planets.texture_descriptor.size.depth_or_array_layers as usize,
            TEXTURED_PLANET_COUNT
        );
        assert_eq!(planets.texture_descriptor.mip_level_count, 12);
    }

    #[test]
    fn ring_specs_cover_saturn_and_uranus_only() {
        let saturn = ring_spec(SolarSystemBody::Saturn).unwrap();
        assert!((saturn.inner_radii - 1.236).abs() < 0.01);
        assert!((saturn.outer_radii - 2.327).abs() < 0.01);
        assert!(ring_spec(SolarSystemBody::Uranus).is_some());
        for body in SolarSystemBody::ALL {
            if !matches!(body, SolarSystemBody::Saturn | SolarSystemBody::Uranus) {
                assert!(ring_spec(body).is_none(), "{body:?}");
            }
        }
        let strip = generate_uranus_ring_strip();
        assert!(strip.pixels().any(|p| p.0[3] > 100));
        assert!(strip.pixels().any(|p| p.0[3] == 0));
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
