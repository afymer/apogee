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
use celestial::{
    catalog::{StarCatalog, StarMetadata, StarRecord, read_star_catalog_from_zst},
    coords::{cartesian_to_canonical_spherical, topocentric_enu_to_spherical},
};
use glam::DVec3;
use tracing::{error, info, warn};

use crate::{
    camera::PlanetariumViewInfo,
    material::{
        CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
    },
};

const PICK_HEALPIX_DEPTH: u8 = 4;
const PICK_HEALPIX_CELLS: usize = 12 * (1 << (2 * PICK_HEALPIX_DEPTH as usize));

/// Rendering parameters governing limiting magnitude and screen-space star disc scaling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StarRenderParams {
    pub limiting_mag: f32,
    pub fade_range: f32,
    pub base_radius: f32,
    pub min_radius: f32,
    pub max_radius: f32,
}

impl Default for StarRenderParams {
    fn default() -> Self {
        Self {
            limiting_mag: 10.5,
            fade_range: 0.6,
            base_radius: 2.2,
            min_radius: 0.8,
            max_radius: 24.0,
        }
    }
}

impl StarRenderParams {
    /// Mirrors the rendered star disc radius (in physical pixels) computed in `celestial_star.wgsl`.
    pub fn rendered_radius_px(&self, mag: f32, fov_y_rad: f32) -> f32 {
        let fov = fov_y_rad.max(0.005);
        let zoom_factor = (1.8_f32 / fov).powf(0.28).clamp(0.85, 4.5);
        let mag_factor = 10.0_f32.powf(-0.075 * (mag - 3.5));
        let base_r = self.base_radius * zoom_factor;
        let min_r = self.min_radius * (zoom_factor * 0.9).clamp(0.75, 2.5);
        let max_r = self.max_radius * (1.0 + (zoom_factor - 1.0) * 0.6);
        (base_r * mag_factor).clamp(min_r, max_r)
    }

    fn max_rendered_radius_px(&self, fov_y_rad: f32) -> f32 {
        let fov = fov_y_rad.max(0.005);
        let zoom_factor = (1.8_f32 / fov).powf(0.28).clamp(0.85, 4.5);
        self.max_radius * (1.0 + (zoom_factor - 1.0) * 0.6)
    }
}

#[inline]
fn lerp_rgb(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

/// Maps Gaia `BP - RP` color index to approximate sRGB star tint, mirroring `celestial_star.wgsl`.
pub fn bp_rp_to_srgb(bp_rp: f32) -> [f32; 3] {
    let t = ((bp_rp + 0.3) / 2.5).clamp(0.0, 1.0);
    if t < 0.15 {
        lerp_rgb([0.65, 0.78, 1.0], [0.85, 0.90, 1.0], t / 0.15)
    } else if t < 0.35 {
        lerp_rgb([0.85, 0.90, 1.0], [1.0, 1.0, 0.98], (t - 0.15) / 0.20)
    } else if t < 0.55 {
        lerp_rgb([1.0, 1.0, 0.98], [1.0, 0.92, 0.75], (t - 0.35) / 0.20)
    } else if t < 0.75 {
        lerp_rgb([1.0, 0.92, 0.75], [1.0, 0.75, 0.45], (t - 0.55) / 0.20)
    } else {
        lerp_rgb([1.0, 0.75, 0.45], [1.0, 0.50, 0.30], (t - 0.75) / 0.25)
    }
}

/// Maps Gaia `BP - RP` color index to an approximate Morgan-Keenan spectral class letter.
pub fn spectral_class_from_bp_rp(bp_rp: f32) -> &'static str {
    if bp_rp < -0.25 {
        "O"
    } else if bp_rp < 0.0 {
        "B"
    } else if bp_rp < 0.35 {
        "A"
    } else if bp_rp < 0.75 {
        "F"
    } else if bp_rp < 1.05 {
        "G"
    } else if bp_rp < 1.80 {
        "K"
    } else {
        "M"
    }
}

/// Estimates effective surface temperature (K), rounded to the nearest 50 K,
/// via piecewise-linear interpolation over Gaia DR3 empirical `(BP - RP, Teff)` anchors.
pub fn estimate_teff_kelvin(bp_rp: f32) -> u32 {
    const ANCHORS: [(f32, f32); 11] = [
        (-0.40, 28000.0),
        (-0.25, 18000.0),
        (0.00, 9700.0),
        (0.35, 7400.0),
        (0.65, 6250.0),
        (0.82, 5770.0),
        (1.15, 5000.0),
        (1.50, 4400.0),
        (1.85, 3850.0),
        (2.50, 3400.0),
        (3.50, 2800.0),
    ];

    let x = if bp_rp.is_finite() { bp_rp } else { 0.82 };
    let teff = if x <= ANCHORS[0].0 {
        ANCHORS[0].1
    } else if x >= ANCHORS[ANCHORS.len() - 1].0 {
        ANCHORS[ANCHORS.len() - 1].1
    } else {
        let mut out = ANCHORS[0].1;
        for w in ANCHORS.windows(2) {
            let (x0, y0) = w[0];
            let (x1, y1) = w[1];
            if x <= x1 {
                let u = (x - x0) / (x1 - x0);
                out = y0 + (y1 - y0) * u;
                break;
            }
        }
        out
    };
    ((teff / 50.0).round() * 50.0).max(50.0) as u32
}

/// Star picking result containing catalog photometry, coordinates, screen projection, and optional names.
#[derive(Clone, Debug, PartialEq)]
pub struct PickedStar {
    pub catalog_index: u32,
    pub mag: f32,
    pub bp_rp: f32,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub screen_ndc: [f64; 2],
    pub rendered_radius_px: f32,
    pub srgb: [f32; 3],
    pub spectral_class: &'static str,
    pub estimated_teff_k: u32,
    pub metadata: Option<StarMetadata>,
}

impl PickedStar {
    /// Returns the primary display title and optional designation/spectral-type subtitle for hover UI.
    pub fn display_title_and_subtitle(&self) -> (String, Option<String>) {
        match self.metadata.as_ref() {
            Some(meta) => meta.display_title_and_subtitle(self.spectral_class),
            None => (format!("{}-type Star", self.spectral_class), None),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct IndexedStar {
    record: StarRecord,
    catalog_index: u32,
}

/// Depth-4 nested HEALPix spatial index over star records, sorted brightest-first per cell.
pub struct StarSpatialIndex {
    offsets: Vec<u32>,
    entries: Vec<IndexedStar>,
    metadata: Vec<(u32, StarMetadata)>,
}

impl StarSpatialIndex {
    /// Builds a spatial index without metadata, grouped into HEALPix depth-4 buckets and sorted by magnitude ascending.
    pub fn build(records: &[StarRecord]) -> Self {
        Self::build_with_metadata(records, Vec::new())
    }

    /// Builds a spatial index with sparse star naming/designation metadata keyed by catalog index.
    pub fn build_with_metadata(
        records: &[StarRecord],
        mut metadata: Vec<(u32, StarMetadata)>,
    ) -> Self {
        metadata.retain(|(idx, meta)| (*idx as usize) < records.len() && !meta.is_empty());
        metadata.sort_unstable_by_key(|(idx, _)| *idx);
        metadata.dedup_by_key(|(idx, _)| *idx);

        let layer = cdshealpix::nested::get(PICK_HEALPIX_DEPTH);
        let mut cell_of = Vec::with_capacity(records.len());
        let mut counts = vec![0_u32; PICK_HEALPIX_CELLS];

        for rec in records {
            let dir = DVec3::new(f64::from(rec.pos[0]), f64::from(rec.pos[1]), f64::from(rec.pos[2]));
            let (lon, lat) = cartesian_to_canonical_spherical(dir);
            let lon_rad = lon.normalize().radians();
            let lat_rad = lat
                .radians()
                .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            let cell = (layer.hash(lon_rad, lat_rad) as usize).min(PICK_HEALPIX_CELLS - 1);
            counts[cell] += 1;
            cell_of.push(cell);
        }

        let mut offsets = Vec::with_capacity(PICK_HEALPIX_CELLS + 1);
        let mut running = 0_u32;
        offsets.push(0);
        for c in &counts {
            running += *c;
            offsets.push(running);
        }

        let mut entries = vec![
            IndexedStar {
                record: StarRecord::new([0.0, 0.0, 1.0], 0.0, 0.0),
                catalog_index: 0,
            };
            records.len()
        ];
        let mut write_pos = offsets[..PICK_HEALPIX_CELLS].to_vec();

        for (idx, (&rec, &cell)) in records.iter().zip(cell_of.iter()).enumerate() {
            let pos = write_pos[cell] as usize;
            entries[pos] = IndexedStar {
                record: rec,
                catalog_index: idx as u32,
            };
            write_pos[cell] += 1;
        }

        for cell in 0..PICK_HEALPIX_CELLS {
            let start = offsets[cell] as usize;
            let end = offsets[cell + 1] as usize;
            entries[start..end].sort_unstable_by_key(|e| (e.record.mag_scaled, e.catalog_index));
        }

        Self {
            offsets,
            entries,
            metadata,
        }
    }

    /// Looks up optional naming metadata for `catalog_index` in `O(log N)` time.
    pub fn metadata_for(&self, catalog_index: u32) -> Option<&StarMetadata> {
        self.metadata
            .binary_search_by_key(&catalog_index, |(idx, _)| *idx)
            .ok()
            .map(|pos| &self.metadata[pos].1)
    }

    /// Iterates over every star in the index as `(catalog_index, StarRecord, Option<&StarMetadata>)`.
    pub fn iter_stars(
        &self,
    ) -> impl Iterator<Item = (u32, StarRecord, Option<&StarMetadata>)> + '_ {
        self.entries.iter().map(|entry| {
            (
                entry.catalog_index,
                entry.record,
                self.metadata_for(entry.catalog_index),
            )
        })
    }

    /// Picks the highest-priority star under `cursor_ndc`, prioritizing brighter/larger stars.
    #[allow(clippy::too_many_arguments)]
    pub fn pick(
        &self,
        view_info: &PlanetariumViewInfo,
        cursor_ndc: (f64, f64),
        viewport_logical: (f64, f64),
        scale_factor: f64,
        params: &StarRenderParams,
        occlude_below_horizon: bool,
        occlude_horizon: Option<&celestial::HorizonProfile>,
    ) -> Option<PickedStar> {
        let vw = viewport_logical.0;
        let vh = viewport_logical.1;
        if vw <= 1.0 || vh <= 1.0 {
            return None;
        }

        let safe_scale = scale_factor.max(0.25);
        let fov_y_rad = view_info.projection.fov_y.radians() as f32;
        let max_rendered_logical = f64::from(params.max_rendered_radius_px(fov_y_rad)) / safe_scale;
        let max_hit_logical = (max_rendered_logical * 1.4 + 6.0).max(10.0);

        let v_cam0 = view_info
            .projection
            .unproject_ndc(cursor_ndc.0, cursor_ndc.1)
            .normalize_or_zero();
        if v_cam0 == DVec3::ZERO {
            return None;
        }

        let dndc_x = (2.0 * max_hit_logical / vw).min(0.5);
        let dndc_y = (2.0 * max_hit_logical / vh).min(0.5);
        let mut search_rad = 1e-4_f64;
        for (px, py) in [
            (cursor_ndc.0 + dndc_x, cursor_ndc.1),
            (cursor_ndc.0 - dndc_x, cursor_ndc.1),
            (cursor_ndc.0, cursor_ndc.1 + dndc_y),
            (cursor_ndc.0, cursor_ndc.1 - dndc_y),
        ] {
            let probe = view_info.projection.unproject_ndc(px, py).normalize_or_zero();
            if probe != DVec3::ZERO {
                let ang = v_cam0.dot(probe).clamp(-1.0, 1.0).acos();
                search_rad = search_rad.max(ang);
            }
        }
        let search_rad = (search_rad * 1.35).clamp(1e-5, 0.35);

        let cursor_icrs = (view_info.camera_to_icrs * v_cam0).normalize_or_zero();
        let (lon, lat) = cartesian_to_canonical_spherical(cursor_icrs);
        let lon_rad = lon.normalize().radians();
        let lat_rad = lat
            .radians()
            .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);

        let layer = cdshealpix::nested::get(PICK_HEALPIX_DEPTH);
        let bmoc = layer.cone_coverage_approx(lon_rad, lat_rad, search_rad);
        let mut cells: Vec<u64> = bmoc.flat_iter().collect();
        cells.sort_unstable();
        cells.dedup();

        let max_visible_mag = params.limiting_mag + params.fade_range;

        struct Candidate {
            score: f64,
            mag_scaled: i16,
            dist_px: f64,
            catalog_index: u32,
            mag: f32,
            bp_rp: f32,
            ra_deg: f64,
            dec_deg: f64,
            az_deg: f64,
            alt_deg: f64,
            screen_ndc: [f64; 2],
            rendered_radius_px: f32,
        }

        let mut best: Option<Candidate> = None;

        for cell_u64 in cells {
            let cell = cell_u64 as usize;
            if cell + 1 >= self.offsets.len() {
                continue;
            }
            let start = self.offsets[cell] as usize;
            let end = self.offsets[cell + 1] as usize;

            for entry in &self.entries[start..end] {
                let mag = entry.record.mag();
                if mag >= max_visible_mag {
                    break;
                }

                let star_icrs = DVec3::new(
                    f64::from(entry.record.pos[0]),
                    f64::from(entry.record.pos[1]),
                    f64::from(entry.record.pos[2]),
                );
                let Some((nx, ny)) = view_info.project_icrs_to_ndc(star_icrs) else {
                    continue;
                };
                if nx.abs() > 1.02 || ny.abs() > 1.02 {
                    continue;
                }

                let v_topo = view_info.camera_to_topo
                    * (view_info.camera_to_icrs.transpose() * star_icrs);
                let (az, alt) = topocentric_enu_to_spherical(v_topo);
                let alt_deg = alt.degrees();
                if occlude_below_horizon && alt_deg < 0.0 {
                    continue;
                }
                if let Some(horizon) = occlude_horizon {
                    if !horizon.is_visible(az.degrees(), alt_deg) {
                        continue;
                    }
                }

                let dx_px = (nx - cursor_ndc.0) * 0.5 * vw;
                let dy_px = (ny - cursor_ndc.1) * 0.5 * vh;
                let dist_px = (dx_px * dx_px + dy_px * dy_px).sqrt();

                let rendered_px = params.rendered_radius_px(mag, fov_y_rad);
                let rendered_logical = f64::from(rendered_px) / safe_scale;
                let hit_radius = (rendered_logical * 1.4 + 6.0).max(9.0);
                if dist_px > hit_radius {
                    continue;
                }

                let score = f64::from(mag) + 2.5 * (dist_px / hit_radius);
                let is_better = match best.as_ref() {
                    None => true,
                    Some(prev) => {
                        if (score - prev.score).abs() > 1e-12 {
                            score < prev.score
                        } else if entry.record.mag_scaled != prev.mag_scaled {
                            entry.record.mag_scaled < prev.mag_scaled
                        } else if (dist_px - prev.dist_px).abs() > 1e-12 {
                            dist_px < prev.dist_px
                        } else {
                            entry.catalog_index < prev.catalog_index
                        }
                    }
                };

                if is_better {
                    let bp_rp = entry.record.color_index();
                    let (ra, dec) = cartesian_to_canonical_spherical(star_icrs);
                    best = Some(Candidate {
                        score,
                        mag_scaled: entry.record.mag_scaled,
                        dist_px,
                        catalog_index: entry.catalog_index,
                        mag,
                        bp_rp,
                        ra_deg: ra.degrees(),
                        dec_deg: dec.degrees(),
                        az_deg: az.degrees(),
                        alt_deg,
                        screen_ndc: [nx, ny],
                        rendered_radius_px: rendered_px,
                    });
                }
            }
        }

        best.map(|c| PickedStar {
            catalog_index: c.catalog_index,
            mag: c.mag,
            bp_rp: c.bp_rp,
            ra_deg: c.ra_deg,
            dec_deg: c.dec_deg,
            az_deg: c.az_deg,
            alt_deg: c.alt_deg,
            screen_ndc: c.screen_ndc,
            rendered_radius_px: c.rendered_radius_px,
            srgb: bp_rp_to_srgb(c.bp_rp),
            spectral_class: spectral_class_from_bp_rp(c.bp_rp),
            estimated_teff_k: estimate_teff_kelvin(c.bp_rp),
            metadata: self.metadata_for(c.catalog_index).cloned(),
        })
    }
}

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
pub fn load_star_catalog() -> Option<StarCatalog> {
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
                        let _span = tracing::info_span!("read_star_catalog_from_zst").entered();
                        read_star_catalog_from_zst(file)
                    };
                    match parse_res {
                        Ok(catalog) => {
                            info!(
                                "Loaded {} stars (magnitudes {:.2} to {:.2}, {} named/identified) from {:?}",
                                catalog.records.len(),
                                catalog.header.min_mag,
                                catalog.header.max_mag,
                                catalog.metadata.len(),
                                path
                            );
                            return Some(catalog);
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

    fn setup_test_view_info() -> (crate::camera::PlanetariumCamera, PlanetariumViewInfo) {
        let mut cam = crate::camera::PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        cam.observer.set_epoch_unix_seconds(1_742_472_000.0);
        cam.view_mode = crate::camera::ViewMode::Equatorial;
        cam.yaw = 0.0;
        cam.pitch = 45.0_f64.to_radians();
        cam.target_yaw = cam.yaw;
        cam.target_pitch = cam.pitch;
        cam.fov_y = 60.0_f64.to_radians();
        cam.target_fov_y = cam.fov_y;
        let info = cam.view_info(16.0 / 9.0);
        (cam, info)
    }

    fn star_at_offset_px(
        info: &PlanetariumViewInfo,
        viewport: (f64, f64),
        dx_px: f64,
        dy_px: f64,
        mag: f32,
        bp_rp: f32,
    ) -> StarRecord {
        let ndc_x = (2.0 * dx_px) / viewport.0;
        let ndc_y = (2.0 * dy_px) / viewport.1;
        let v_cam = info.projection.unproject_ndc(ndc_x, ndc_y);
        let v_icrs = (info.camera_to_icrs * v_cam).normalize();
        StarRecord::new(
            [v_icrs.x as f32, v_icrs.y as f32, v_icrs.z as f32],
            mag,
            bp_rp,
        )
    }

    #[test]
    fn star_picker_prioritizes_brighter_star_in_cluster() {
        let (_cam, info) = setup_test_view_info();
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        // Bright mag 2.0 star at 4 px away vs faint mag 7.5 star right under cursor at 1.5 px
        let records = vec![
            star_at_offset_px(&info, viewport, 1.5, 0.0, 7.5, 1.2),
            star_at_offset_px(&info, viewport, 4.0, 0.0, 2.0, -0.1),
        ];
        let index = StarSpatialIndex::build(&records);
        let picked = index
            .pick(&info, (0.0, 0.0), viewport, 1.0, &params, false, None)
            .expect("should pick bright star");
        assert_eq!(picked.catalog_index, 1);
        assert!((picked.mag - 2.0).abs() < 0.02);
    }

    #[test]
    fn star_picker_selects_faint_star_outside_bright_radius() {
        let (_cam, info) = setup_test_view_info();
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        // Bright star far away (25 px), faint star directly under cursor
        let faint_ndc_x = (2.0 * 30.0) / viewport.0;
        let records = vec![
            star_at_offset_px(&info, viewport, 0.0, 0.0, 1.5, 0.0),
            star_at_offset_px(&info, viewport, 30.0, 0.0, 7.2, 1.1),
        ];
        let index = StarSpatialIndex::build(&records);
        let picked = index
            .pick(&info, (faint_ndc_x, 0.0), viewport, 1.0, &params, false, None)
            .expect("should pick isolated faint star");
        assert_eq!(picked.catalog_index, 1);
        assert!((picked.mag - 7.2).abs() < 0.02);
    }

    #[test]
    fn star_picker_picks_nearest_among_equal_magnitude_stars() {
        let (_cam, info) = setup_test_view_info();
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        let records = vec![
            star_at_offset_px(&info, viewport, 6.0, 0.0, 4.0, 0.5),
            star_at_offset_px(&info, viewport, 2.0, 0.0, 4.0, 0.5),
        ];
        let index = StarSpatialIndex::build(&records);
        let picked = index
            .pick(&info, (0.0, 0.0), viewport, 1.0, &params, false, None)
            .expect("should pick nearest star");
        assert_eq!(picked.catalog_index, 1);
    }

    #[test]
    fn star_picker_respects_below_horizon_occlusion() {
        let mut cam = crate::camera::PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        cam.observer.set_epoch_unix_seconds(1_742_472_000.0);
        cam.view_mode = crate::camera::ViewMode::Horizontal;
        cam.yaw = 0.0;
        cam.pitch = -30.0_f64.to_radians();
        cam.target_yaw = cam.yaw;
        cam.target_pitch = cam.pitch;
        let info = cam.view_info(16.0 / 9.0);
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        let records = vec![star_at_offset_px(&info, viewport, 0.0, 0.0, 2.0, 0.3)];
        let index = StarSpatialIndex::build(&records);

        assert!(
            index
                .pick(&info, (0.0, 0.0), viewport, 1.0, &params, true, None)
                .is_none()
        );
        assert!(
            index
                .pick(&info, (0.0, 0.0), viewport, 1.0, &params, false, None)
                .is_some()
        );
    }

    #[test]
    fn star_picker_respects_custom_horizon_occlusion() {
        let mut cam = crate::camera::PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        cam.observer.set_epoch_unix_seconds(1_742_472_000.0);
        cam.view_mode = crate::camera::ViewMode::Horizontal;
        cam.yaw = 0.0;
        cam.pitch = 10.0_f64.to_radians();
        cam.target_yaw = cam.yaw;
        cam.target_pitch = cam.pitch;
        let info = cam.view_info(16.0 / 9.0);
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        let records = vec![star_at_offset_px(&info, viewport, 0.0, 0.0, 2.0, 0.3)];
        let index = StarSpatialIndex::build(&records);

        let high_horizon = celestial::HorizonProfile::flat(25.0, 360);
        let low_horizon = celestial::HorizonProfile::flat(5.0, 360);

        assert!(
            index
                .pick(
                    &info,
                    (0.0, 0.0),
                    viewport,
                    1.0,
                    &params,
                    true,
                    Some(&high_horizon),
                )
                .is_none()
        );
        assert!(
            index
                .pick(
                    &info,
                    (0.0, 0.0),
                    viewport,
                    1.0,
                    &params,
                    true,
                    Some(&low_horizon),
                )
                .is_some()
        );
        assert!(
            index
                .pick(&info, (0.0, 0.0), viewport, 1.0, &params, true, None)
                .is_some()
        );
    }

    #[test]
    fn spectral_and_teff_calibration_monotonic() {
        assert_eq!(spectral_class_from_bp_rp(-0.35), "O");
        assert_eq!(spectral_class_from_bp_rp(-0.10), "B");
        assert_eq!(spectral_class_from_bp_rp(0.15), "A");
        assert_eq!(spectral_class_from_bp_rp(0.55), "F");
        assert_eq!(spectral_class_from_bp_rp(0.82), "G");
        assert_eq!(spectral_class_from_bp_rp(1.30), "K");
        assert_eq!(spectral_class_from_bp_rp(2.20), "M");

        let samples = [-0.40_f32, -0.10, 0.20, 0.82, 1.50, 2.50, 3.50];
        for w in samples.windows(2) {
            assert!(estimate_teff_kelvin(w[0]) > estimate_teff_kelvin(w[1]));
        }
        assert_eq!(estimate_teff_kelvin(0.82), 5750);
    }

    #[test]
    fn star_picker_returns_metadata_for_named_and_unnamed_stars() {
        let (_cam, info) = setup_test_view_info();
        let viewport = (1600.0, 900.0);
        let params = StarRenderParams::default();

        let records = vec![
            star_at_offset_px(&info, viewport, 0.0, 0.0, 1.8, -0.1),
            star_at_offset_px(&info, viewport, 40.0, 0.0, 6.5, 1.2),
        ];
        let metadata = vec![(
            0,
            StarMetadata {
                proper_name: Some("Mintaka".to_string()),
                bayer_flamsteed: Some("δ Ori".to_string()),
                hip: Some(25930),
                ..Default::default()
            },
        )];
        let index = StarSpatialIndex::build_with_metadata(&records, metadata);

        let named = index
            .pick(&info, (0.0, 0.0), viewport, 1.0, &params, false, None)
            .expect("pick named star");
        assert_eq!(
            named.display_title_and_subtitle(),
            (
                "Mintaka".to_string(),
                Some("δ Ori · HIP 25930 · B-type".to_string())
            )
        );

        let unnamed_ndc_x = (2.0 * 40.0) / viewport.0;
        let unnamed = index
            .pick(&info, (unnamed_ndc_x, 0.0), viewport, 1.0, &params, false, None)
            .expect("pick unnamed star");
        assert!(unnamed.metadata.is_none());
        assert_eq!(
            unnamed.display_title_and_subtitle(),
            ("K-type Star".to_string(), None)
        );
    }

    #[test]
    fn bundled_catalog_has_iconic_named_stars() {
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/catalogs/stars.bin.zst"
        ));
        let file = File::open(&path).expect("open bundled stars.bin.zst");
        let catalog = read_star_catalog_from_zst(file).expect("decode v2 star catalog");

        assert!(
            catalog.records.len() > 62_700,
            "expected > 62,700 stars, got {}",
            catalog.records.len()
        );

        let proper_count = catalog
            .metadata
            .iter()
            .filter(|(_, m)| m.proper_name.is_some())
            .count();
        let bayer_count = catalog
            .metadata
            .iter()
            .filter(|(_, m)| m.bayer_flamsteed.is_some())
            .count();
        assert!(
            proper_count >= 400,
            "expected >= 400 proper-named stars, got {proper_count}"
        );
        assert!(
            bayer_count >= 3_000,
            "expected >= 3,000 Bayer/Flamsteed stars, got {bayer_count}"
        );

        for iconic in [
            "Sirius",
            "Vega",
            "Polaris",
            "Betelgeuse",
            "Rigel",
            "Alioth",
            "Alkaid",
            "Dubhe",
            "Mintaka",
            "Alnilam",
            "Alnitak",
            "Saiph",
            "Albireo",
            "Schedar",
        ] {
            let entry = catalog
                .metadata
                .iter()
                .find(|(_, m)| m.proper_name.as_deref() == Some(iconic));
            let (_, meta) = entry.unwrap_or_else(|| panic!("Missing iconic star {iconic}"));
            assert!(
                meta.bayer_flamsteed
                    .as_deref()
                    .is_some_and(|b| !b.is_empty()),
                "Missing Bayer/Flamsteed designation for {iconic}: {meta:?}"
            );
            assert!(
                meta.hip.is_some(),
                "Missing HIP identifier for {iconic}: {meta:?}"
            );
        }

        assert!(
            catalog
                .metadata
                .iter()
                .any(|(_, m)| m.bayer_flamsteed.as_deref() == Some("γ Cas")),
            "Missing Bayer designation for γ Cas"
        );
    }
}
