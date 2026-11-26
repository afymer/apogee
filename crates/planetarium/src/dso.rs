use std::{fs::File, path::PathBuf};

use bevy::{
    ecs::query::QueryItem,
    mesh::MeshVertexBufferLayoutRef,
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
    HorizonProfile,
    coords::{cartesian_to_canonical_spherical, topocentric_enu_to_spherical},
    dso::{DsoCatalog, DsoRecord, read_dso_catalog_from_zst},
};
use glam::DVec3;
use tracing::{info, warn};

use crate::{
    ViewportDimensions,
    atmosphere::{AtmosphereSettings, compute_hips_atmosphere_visibility},
    camera::{PlanetariumCamera, PlanetariumViewInfo},
    material::{
        CelestialPipelineId, CelestialPipelineKey, CelestialProjectionUniforms, pipeline_descriptor,
    },
};

const PICK_HEALPIX_DEPTH: u8 = 4;
const PICK_HEALPIX_CELLS: usize = 12 * (1 << (2 * PICK_HEALPIX_DEPTH as usize));
const ARCMIN_TO_RAD: f32 = std::f32::consts::PI / (180.0 * 60.0);

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 32-byte GPU record matching `celestial_dso.wgsl::DsoGpuRecord`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DsoGpuRecord {
    pub pos: [f32; 3],
    pub eff_mag: f32,
    pub semi_major_rad: f32,
    pub semi_minor_rad: f32,
    pub pos_angle_rad: f32,
    pub packed_flags: u32,
}

impl DsoGpuRecord {
    pub fn from_record(rec: &DsoRecord) -> Self {
        let dir = rec.icrs_dir();
        let maj = rec
            .major_arcmin
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.0);
        let semi_major_rad = 0.5 * maj * ARCMIN_TO_RAD;
        let semi_minor_rad = if maj > 0.0 {
            let min_val = rec
                .minor_arcmin
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(maj)
                .min(maj);
            0.5 * min_val * ARCMIN_TO_RAD
        } else {
            0.0
        };
        let pos_angle_rad = rec
            .position_angle_deg
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
            .to_radians();

        let mut flags =
            (u32::from(rec.kind.symbol_code()) & 0xFF) | ((u32::from(rec.kind.color_id()) & 0xFF) << 8);
        if rec.is_messier() {
            flags |= 1 << 16;
        }
        if rec.is_ngc() {
            flags |= 1 << 17;
        }
        if maj > 0.0 {
            flags |= 1 << 18;
        }

        Self {
            pos: [dir.x as f32, dir.y as f32, dir.z as f32],
            eff_mag: rec.effective_display_mag(),
            semi_major_rad,
            semi_minor_rad,
            pos_angle_rad,
            packed_flags: flags,
        }
    }
}

/// Renderer-side deep-sky catalog settings updated via [`crate::ViewportCommand`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct DsoSettings {
    pub opacity: f32,
    pub show_messier: bool,
    pub show_ngc: bool,
}

impl Default for DsoSettings {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            show_messier: true,
            show_ngc: true,
        }
    }
}

impl DsoSettings {
    pub fn catalog_mask(&self) -> u32 {
        u32::from(self.show_messier) | (u32::from(self.show_ngc) << 1)
    }
}

/// CPU/GPU-mirrored visibility and sizing parameters for deep-sky objects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DsoRenderParams {
    pub ngc_limiting_mag: f32,
    pub messier_boost: f32,
    pub fade_range: f32,
    pub atmo_visibility: f32,
    pub show_messier: bool,
    pub show_ngc: bool,
}

impl DsoRenderParams {
    pub fn for_sky(
        sun_alt_deg: f32,
        fov_y_rad: f32,
        atmosphere_strength: f32,
        show_messier: bool,
        show_ngc: bool,
    ) -> Self {
        let fov_deg = fov_y_rad.to_degrees().max(0.05);
        let log_ratio = (180.0 / fov_deg).log2().max(0.0);
        Self {
            ngc_limiting_mag: 6.2 + 1.75 * log_ratio,
            messier_boost: 2.2,
            fade_range: 1.2,
            atmo_visibility: compute_hips_atmosphere_visibility(sun_alt_deg, atmosphere_strength),
            show_messier,
            show_ngc,
        }
    }

    pub fn record_enabled(&self, rec: &DsoRecord) -> bool {
        (rec.is_messier() && self.show_messier) || (rec.is_ngc() && self.show_ngc)
    }

    pub fn visibility_alpha(&self, rec: &DsoRecord, true_a_px: f32, viewport_min_dim: f32) -> f32 {
        if !self.record_enabled(rec) || self.atmo_visibility <= 1e-3 {
            return 0.0;
        }
        let limit = self.ngc_limiting_mag + if rec.is_messier() { self.messier_boost } else { 0.0 };
        let fade_range = self.fade_range.max(0.1);
        let mag_vis =
            ((limit + fade_range - rec.effective_display_mag()) / fade_range).clamp(0.0, 1.0);
        let size_vis = smoothstep(8.0, 18.0, true_a_px);
        let base_vis = mag_vis.max(size_vis);

        let min_dim = viewport_min_dim.max(1.0);
        let oversize_t = smoothstep(0.45 * min_dim, 1.8 * min_dim, true_a_px);
        let giant_atten = 1.0 + (0.32 - 1.0) * oversize_t;
        let base_alpha = if rec.is_messier() { 0.90 } else { 0.76 };
        base_vis * giant_atten * base_alpha * self.atmo_visibility.clamp(0.0, 1.0)
    }

    pub fn displayed_radius_px(&self, rec: &DsoRecord, true_a_px: f32) -> f32 {
        let min_icon_r = if rec.is_messier() { 7.5 } else { 6.0 };
        true_a_px.max(min_icon_r)
    }
}

/// Result of picking a deep-sky object in the planetarium viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct PickedDso {
    pub catalog_index: u32,
    pub record: DsoRecord,
    pub az_deg: f64,
    pub alt_deg: f64,
    pub screen_ndc: [f64; 2],
    pub rendered_radius_px: f32,
    pub center_dist_logical: f64,
    pub hit_on_perimeter: bool,
}

/// On-screen label descriptor for a prominent deep-sky object.
#[derive(Clone, Debug, PartialEq)]
pub struct DsoLabel {
    pub catalog_index: u32,
    pub text: String,
    pub srgb: [f32; 3],
    pub screen_ndc: [f64; 2],
    pub rendered_radius_px: f32,
    pub alpha: f32,
}

/// Depth-4 HEALPix spatial index over Messier / NGC deep-sky records.
pub struct DsoSpatialIndex {
    records: Vec<DsoRecord>,
    icrs_dirs: Vec<DVec3>,
    offsets: Vec<u32>,
    entries: Vec<u32>,
    large_indices: Vec<u32>,
    prominent_order: Vec<u32>,
}

impl DsoSpatialIndex {
    pub fn build(records: Vec<DsoRecord>) -> Self {
        let layer = cdshealpix::nested::get(PICK_HEALPIX_DEPTH);
        let mut icrs_dirs = Vec::with_capacity(records.len());
        let mut cell_of = Vec::with_capacity(records.len());
        let mut counts = vec![0_u32; PICK_HEALPIX_CELLS];
        let mut large_indices = Vec::new();
        let mut prominent_order: Vec<u32> = (0..records.len() as u32).collect();

        for (idx, rec) in records.iter().enumerate() {
            let dir = rec.icrs_dir();
            let (lon, lat) = cartesian_to_canonical_spherical(dir);
            let lon_rad = lon.normalize().radians();
            let lat_rad = lat
                .radians()
                .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            let cell = (layer.hash(lon_rad, lat_rad) as usize).min(PICK_HEALPIX_CELLS - 1);
            counts[cell] += 1;
            cell_of.push(cell);
            icrs_dirs.push(dir);
            if rec.major_arcmin.is_some_and(|maj| maj >= 30.0) {
                large_indices.push(idx as u32);
            }
        }

        let mut offsets = Vec::with_capacity(PICK_HEALPIX_CELLS + 1);
        let mut running = 0_u32;
        offsets.push(0);
        for c in &counts {
            running += *c;
            offsets.push(running);
        }

        let mut entries = vec![0_u32; records.len()];
        let mut write_pos = offsets[..PICK_HEALPIX_CELLS].to_vec();
        for (idx, &cell) in cell_of.iter().enumerate() {
            let pos = write_pos[cell] as usize;
            entries[pos] = idx as u32;
            write_pos[cell] += 1;
        }

        let cmp_prominence = |&a: &u32, &b: &u32| {
            let ra = &records[a as usize];
            let rb = &records[b as usize];
            (!ra.is_messier())
                .cmp(&(!rb.is_messier()))
                .then_with(|| {
                    ra.effective_display_mag()
                        .total_cmp(&rb.effective_display_mag())
                })
                .then_with(|| a.cmp(&b))
        };

        for cell in 0..PICK_HEALPIX_CELLS {
            let start = offsets[cell] as usize;
            let end = offsets[cell + 1] as usize;
            entries[start..end].sort_unstable_by(cmp_prominence);
        }
        prominent_order.sort_unstable_by(cmp_prominence);

        Self {
            records,
            icrs_dirs,
            offsets,
            entries,
            large_indices,
            prominent_order,
        }
    }

    pub fn records(&self) -> &[DsoRecord] {
        &self.records
    }

    pub fn iter_dsos(&self) -> impl Iterator<Item = (u32, &DsoRecord)> + '_ {
        self.records
            .iter()
            .enumerate()
            .map(|(i, r)| (i as u32, r))
    }

    /// Computes approximate physical pixels per radian at `v_icrs` around the projection center.
    fn local_px_per_rad(
        view_info: &PlanetariumViewInfo,
        v_icrs: DVec3,
        viewport_physical: (f64, f64),
    ) -> f64 {
        let fov_y = view_info.projection.fov_y.radians().max(1e-5);
        let focal_px = viewport_physical.1 / (2.0 * (0.5 * fov_y).tan().max(1e-6));
        let v_cam = view_info.camera_to_icrs.transpose() * v_icrs;
        let nz = v_cam.z.clamp(-0.95, 1.0);
        // Stereographic expands slightly away from center; perspective stays ~1/cos(theta)^2.
        let stereo_scale = 2.0 / (1.0 + nz).max(0.05);
        focal_px * stereo_scale.clamp(0.8, 3.0) * 0.5_f64.max(1.0)
            / (if view_info.projection.blend_factor() > 0.5 {
                nz.max(0.25)
            } else {
                1.0
            })
    }

    fn true_semi_major_px(
        view_info: &PlanetariumViewInfo,
        v_icrs: DVec3,
        rec: &DsoRecord,
        viewport_physical: (f64, f64),
    ) -> f32 {
        let Some(maj) = rec.major_arcmin.filter(|v| v.is_finite() && *v > 0.0) else {
            return 0.0;
        };
        let semi_major_rad = 0.5 * f64::from(maj) * f64::from(ARCMIN_TO_RAD);
        let px_per_rad = Self::local_px_per_rad(view_info, v_icrs, viewport_physical);
        (semi_major_rad * px_per_rad) as f32
    }

    #[allow(clippy::too_many_arguments)]
    pub fn pick(
        &self,
        view_info: &PlanetariumViewInfo,
        cursor_ndc: (f64, f64),
        viewport_logical: (f64, f64),
        scale_factor: f64,
        params: &DsoRenderParams,
        occlude_below_horizon: bool,
        occlude_horizon: Option<&HorizonProfile>,
    ) -> Option<PickedDso> {
        let vw = viewport_logical.0;
        let vh = viewport_logical.1;
        if vw <= 1.0 || vh <= 1.0 || params.atmo_visibility <= 1e-3 {
            return None;
        }
        if !params.show_messier && !params.show_ngc {
            return None;
        }

        let safe_scale = scale_factor.max(0.25);
        let viewport_physical = (vw * safe_scale, vh * safe_scale);
        let min_dim_phys = viewport_physical.0.min(viewport_physical.1) as f32;
        let max_dim_phys = viewport_physical.0.max(viewport_physical.1) as f32;

        let v_cam0 = view_info
            .projection
            .unproject_ndc(cursor_ndc.0, cursor_ndc.1)
            .normalize_or_zero();
        if v_cam0 == DVec3::ZERO {
            return None;
        }

        let max_hit_logical = 32.0;
        let dndc_x = (2.0 * max_hit_logical / vw).min(0.5);
        let dndc_y = (2.0 * max_hit_logical / vh).min(0.5);
        let mut search_rad = 1e-4_f64;
        for (px, py) in [
            (cursor_ndc.0 + dndc_x, cursor_ndc.1),
            (cursor_ndc.0 - dndc_x, cursor_ndc.1),
            (cursor_ndc.0, cursor_ndc.1 + dndc_y),
            (cursor_ndc.0, cursor_ndc.1 - dndc_y),
        ] {
            let probe = view_info
                .projection
                .unproject_ndc(px, py)
                .normalize_or_zero();
            if probe != DVec3::ZERO {
                let ang = v_cam0.dot(probe).clamp(-1.0, 1.0).acos();
                search_rad = search_rad.max(ang);
            }
        }
        // Include up to ~0.7° around the cursor for medium DSOs; >= 30' objects are checked via `large_indices`.
        let search_rad = (search_rad * 1.35 + 0.012).clamp(1e-4, 0.40);

        let cursor_icrs = (view_info.camera_to_icrs * v_cam0).normalize_or_zero();
        let (lon, lat) = cartesian_to_canonical_spherical(cursor_icrs);
        let lon_rad = lon.normalize().radians();
        let lat_rad = lat
            .radians()
            .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);

        let layer = cdshealpix::nested::get(PICK_HEALPIX_DEPTH);
        let bmoc = layer.cone_coverage_approx(lon_rad, lat_rad, search_rad);
        let mut candidate_indices: Vec<u32> = Vec::new();
        for cell_u64 in bmoc.flat_iter() {
            let cell = cell_u64 as usize;
            if cell + 1 >= self.offsets.len() {
                continue;
            }
            let start = self.offsets[cell] as usize;
            let end = self.offsets[cell + 1] as usize;
            candidate_indices.extend_from_slice(&self.entries[start..end]);
        }
        candidate_indices.extend_from_slice(&self.large_indices);
        candidate_indices.sort_unstable();
        candidate_indices.dedup();

        struct Candidate {
            score: f64,
            catalog_index: u32,
            az_deg: f64,
            alt_deg: f64,
            screen_ndc: [f64; 2],
            rendered_radius_px: f32,
            center_dist_logical: f64,
            hit_on_perimeter: bool,
        }

        let mut best: Option<Candidate> = None;

        for idx in candidate_indices {
            let rec = &self.records[idx as usize];
            if !params.record_enabled(rec) {
                continue;
            }
            let dso_icrs = self.icrs_dirs[idx as usize];
            let Some((nx, ny)) = view_info.project_icrs_to_ndc(dso_icrs) else {
                continue;
            };
            if nx.abs() > 1.25 || ny.abs() > 1.25 {
                continue;
            }

            let true_a_px = Self::true_semi_major_px(view_info, dso_icrs, rec, viewport_physical);
            if true_a_px > 4.0 * max_dim_phys {
                continue;
            }
            let alpha = params.visibility_alpha(rec, true_a_px, min_dim_phys);
            if alpha <= 0.05 {
                continue;
            }

            let v_topo =
                view_info.camera_to_topo * (view_info.camera_to_icrs.transpose() * dso_icrs);
            let (az, alt) = topocentric_enu_to_spherical(v_topo);
            let alt_deg = alt.degrees();
            if occlude_below_horizon && alt_deg < 0.0 {
                continue;
            }
            if let Some(horizon) = occlude_horizon
                && !horizon.is_visible(az.degrees(), alt_deg)
            {
                continue;
            }

            let dx_px = (nx - cursor_ndc.0) * 0.5 * vw;
            let dy_px = (ny - cursor_ndc.1) * 0.5 * vh;
            let dist_logical = (dx_px * dx_px + dy_px * dy_px).sqrt();

            let rendered_px = params.displayed_radius_px(rec, true_a_px);
            let rendered_logical = f64::from(rendered_px) / safe_scale;
            let center_hit_r = rendered_logical.clamp(10.0, 26.0);
            let center_hit = dist_logical <= center_hit_r;
            let perimeter_hit =
                !center_hit && rendered_logical > 26.0 && (dist_logical - rendered_logical).abs() <= 6.0;
            if !center_hit && !perimeter_hit {
                continue;
            }

            let messier_bonus = if rec.is_messier() { -2.5 } else { 0.0 };
            let perimeter_penalty = if perimeter_hit { 3.0 } else { 0.0 };
            let dist_norm = if center_hit {
                dist_logical / center_hit_r
            } else {
                (dist_logical - rendered_logical).abs() / 6.0
            };
            let score =
                f64::from(rec.effective_display_mag()) + messier_bonus + perimeter_penalty + 2.0 * dist_norm;

            let is_better = best.as_ref().is_none_or(|prev| {
                if (score - prev.score).abs() > 1e-9 {
                    score < prev.score
                } else {
                    dist_logical < prev.center_dist_logical
                }
            });

            if is_better {
                best = Some(Candidate {
                    score,
                    catalog_index: idx,
                    az_deg: az.degrees(),
                    alt_deg,
                    screen_ndc: [nx, ny],
                    rendered_radius_px: rendered_px,
                    center_dist_logical: dist_logical,
                    hit_on_perimeter: perimeter_hit,
                });
            }
        }

        best.map(|c| PickedDso {
            catalog_index: c.catalog_index,
            record: self.records[c.catalog_index as usize].clone(),
            az_deg: c.az_deg,
            alt_deg: c.alt_deg,
            screen_ndc: c.screen_ndc,
            rendered_radius_px: c.rendered_radius_px,
            center_dist_logical: c.center_dist_logical,
            hit_on_perimeter: c.hit_on_perimeter,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn visible_labels(
        &self,
        view_info: &PlanetariumViewInfo,
        viewport_logical: (f64, f64),
        scale_factor: f64,
        params: &DsoRenderParams,
        occlude_below_horizon: bool,
        occlude_horizon: Option<&HorizonProfile>,
        max_labels: usize,
    ) -> Vec<DsoLabel> {
        let vw = viewport_logical.0;
        let vh = viewport_logical.1;
        if max_labels == 0 || vw <= 20.0 || vh <= 20.0 || params.atmo_visibility <= 1e-3 {
            return Vec::new();
        }
        if !params.show_messier && !params.show_ngc {
            return Vec::new();
        }

        let safe_scale = scale_factor.max(0.25);
        let viewport_physical = (vw * safe_scale, vh * safe_scale);
        let min_dim_phys = viewport_physical.0.min(viewport_physical.1) as f32;
        let max_dim_phys = viewport_physical.0.max(viewport_physical.1) as f32;

        let mut occupied: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(max_labels * 2);
        let mut labels = Vec::with_capacity(max_labels);

        for &idx in &self.prominent_order {
            if labels.len() >= max_labels {
                break;
            }
            let rec = &self.records[idx as usize];
            if !params.record_enabled(rec) {
                continue;
            }

            let dso_icrs = self.icrs_dirs[idx as usize];
            let Some((nx, ny)) = view_info.project_icrs_to_ndc(dso_icrs) else {
                continue;
            };
            if nx.abs() > 0.96 || ny.abs() > 0.96 {
                continue;
            }

            let true_a_px = Self::true_semi_major_px(view_info, dso_icrs, rec, viewport_physical);
            if true_a_px > 4.0 * max_dim_phys {
                continue;
            }
            let alpha = params.visibility_alpha(rec, true_a_px, min_dim_phys);
            if alpha < 0.28 {
                continue;
            }

            let v_topo =
                view_info.camera_to_topo * (view_info.camera_to_icrs.transpose() * dso_icrs);
            let (az, alt) = topocentric_enu_to_spherical(v_topo);
            let alt_deg = alt.degrees();
            if occlude_below_horizon && alt_deg < 0.0 {
                continue;
            }
            if let Some(horizon) = occlude_horizon
                && !horizon.is_visible(az.degrees(), alt_deg)
            {
                continue;
            }

            let rendered_px = params.displayed_radius_px(rec, true_a_px);
            let r_logical = (f64::from(rendered_px) / safe_scale).min(48.0);
            let sx = (nx * 0.5 + 0.5) * vw;
            let sy = (0.5 - ny * 0.5) * vh;

            let text = rec.primary_designation();
            let box_w = text.len() as f64 * 6.5 + 10.0;
            let box_h = 15.0;
            let lx = sx + r_logical + 5.0;
            let ly = sy - box_h * 0.5;
            if lx < 8.0 || lx + box_w > vw - 8.0 || ly < 8.0 || ly + box_h > vh - 8.0 {
                continue;
            }

            let label_rect = (lx, ly, lx + box_w, ly + box_h);
            let collides = occupied.iter().any(|&(ax0, ay0, ax1, ay1)| {
                label_rect.0 < ax1
                    && label_rect.2 > ax0
                    && label_rect.1 < ay1
                    && label_rect.3 > ay0
            });
            if collides {
                continue;
            }

            occupied.push(label_rect);
            // Also reserve the marker core around the DSO center so later labels avoid overlapping it.
            let core_r = r_logical.min(14.0);
            occupied.push((sx - core_r, sy - core_r, sx + core_r, sy + core_r));

            labels.push(DsoLabel {
                catalog_index: idx,
                text,
                srgb: rec.kind.srgb(),
                screen_ndc: [nx, ny],
                rendered_radius_px: rendered_px,
                alpha: alpha.clamp(0.0, 1.0),
            });
        }

        labels
    }
}

#[derive(Clone, Copy, ShaderType, Debug, Component)]
pub struct CelestialDsoUniforms {
    pub transform: Mat4,
    pub projection: CelestialProjectionUniforms,
    pub viewport_size: Vec2,
    pub ngc_limiting_mag: f32,
    pub messier_boost: f32,
    pub fade_range: f32,
    pub opacity: f32,
    pub catalog_mask: u32,
    pub atmo_visibility: f32,
}

#[derive(Component, Debug, Clone)]
pub struct CelestialDsoMaterial {
    pub uniforms: CelestialDsoUniforms,
    pub dsos: Handle<ShaderBuffer>,
    pub dso_count: u32,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct CelestialDsoDraw {
    pub mesh: AssetId<Mesh>,
    pub dsos: AssetId<ShaderBuffer>,
    pub dso_count: u32,
}

impl SyncComponent for CelestialDsoMaterial {
    type Target = (
        CelestialDsoUniforms,
        CelestialDsoDraw,
        CelestialPipelineId,
    );
}

impl ExtractComponent for CelestialDsoMaterial {
    type QueryData = (&'static Self, &'static Mesh3d);
    type QueryFilter = ();
    type Out = (CelestialDsoUniforms, CelestialDsoDraw);

    fn extract_component(
        (material, mesh): QueryItem<'_, '_, Self::QueryData>,
    ) -> Option<Self::Out> {
        if material.uniforms.opacity <= 1e-4
            || material.uniforms.catalog_mask == 0
            || material.uniforms.atmo_visibility <= 1e-4
        {
            return None;
        }
        Some((
            material.uniforms,
            CelestialDsoDraw {
                mesh: mesh.id(),
                dsos: material.dsos.id(),
                dso_count: material.dso_count,
            },
        ))
    }
}

#[derive(Resource)]
pub struct CelestialDsoPipeline {
    pub layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl CelestialDsoPipeline {
    pub fn new(shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "celestial_dso_bind_group_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer::<CelestialDsoUniforms>(true),
                    storage_buffer_read_only_sized(false, None),
                ),
            ),
        );
        Self { layout, shader }
    }
}

impl SpecializedMeshPipeline for CelestialDsoPipeline {
    type Key = CelestialPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor =
            pipeline_descriptor("celestial_dso_pipeline", &self.layout, &self.shader, key);
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(descriptor)
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn dso_system(
    camera_query: Query<&PlanetariumCamera>,
    dims: Res<ViewportDimensions>,
    dso_settings: Res<DsoSettings>,
    atmo_settings: Res<AtmosphereSettings>,
    mut dso_materials: Query<&mut CelestialDsoMaterial>,
) {
    let Ok(camera) = camera_query.single() else {
        return;
    };
    let aspect = dims.width.max(1) as f32 / dims.height.max(1) as f32;
    let proj_params = camera.projection_params(aspect);
    let sun_alt_deg = celestial::solar::solar_altitude_deg(
        camera.observer.location,
        camera.observer.epoch,
    ) as f32;
    let render_params = DsoRenderParams::for_sky(
        sun_alt_deg,
        proj_params.fov_y,
        atmo_settings.strength,
        dso_settings.show_messier,
        dso_settings.show_ngc,
    );
    let transform = glam::DMat4::from_mat3(camera.icrs_to_camera_matrix()).as_mat4();
    let viewport_size = Vec2::new(dims.width as f32, dims.height as f32);

    for mut mat in &mut dso_materials {
        mat.uniforms.transform = transform;
        mat.uniforms.projection = proj_params.into();
        mat.uniforms.viewport_size = viewport_size;
        mat.uniforms.ngc_limiting_mag = render_params.ngc_limiting_mag;
        mat.uniforms.messier_boost = render_params.messier_boost;
        mat.uniforms.fade_range = render_params.fade_range;
        mat.uniforms.opacity = dso_settings.opacity;
        mat.uniforms.catalog_mask = dso_settings.catalog_mask();
        mat.uniforms.atmo_visibility = render_params.atmo_visibility;
    }
}

/// Discovers and loads the Messier/NGC deep-sky catalog from disk, checking
/// `APOGEE_DSO_PATH` first and then common relative locations.
pub fn load_dso_catalog() -> Option<DsoCatalog> {
    let _span = tracing::info_span!("load_dso_catalog").entered();
    let mut candidates = Vec::new();

    if let Ok(env_path) = std::env::var("APOGEE_DSO_PATH") {
        candidates.push(PathBuf::from(env_path));
    }

    candidates.push(PathBuf::from("assets/catalogs/dso.bin.zst"));
    candidates.push(PathBuf::from("../assets/catalogs/dso.bin.zst"));
    candidates.push(PathBuf::from("../../assets/catalogs/dso.bin.zst"));

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("assets/catalogs/dso.bin.zst"));
        candidates.push(dir.join("dso.bin.zst"));
    }
    candidates.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/catalogs/dso.bin.zst"
    )));

    for path in &candidates {
        if path.exists() {
            info!("Loading DSO catalog from {:?}", path);
            match File::open(path) {
                Ok(file) => match read_dso_catalog_from_zst(file) {
                    Ok(catalog) => {
                        info!(
                            "Loaded {} DSOs ({} Messier, {} NGC) from {:?}",
                            catalog.records.len(),
                            catalog.header.messier_count,
                            catalog.header.ngc_count,
                            path
                        );
                        return Some(catalog);
                    }
                    Err(err) => {
                        warn!("Failed to parse DSO catalog at {:?}: {}", path, err);
                    }
                },
                Err(err) => {
                    warn!("Failed to open DSO catalog at {:?}: {}", path, err);
                }
            }
        }
    }

    warn!(
        "DSO catalog file (dso.bin.zst) not found in candidate paths: {:?}",
        candidates
    );
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use celestial::dso::DsoKind;

    use super::*;

    fn setup_test_view_info() -> (PlanetariumCamera, PlanetariumViewInfo) {
        let mut cam = PlanetariumCamera::default();
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

    fn dso_at_offset_px(
        info: &PlanetariumViewInfo,
        viewport: (f64, f64),
        offset_px: (f64, f64),
        messier: Option<u16>,
        ngc: Option<&str>,
        v_mag: f32,
        major_arcmin: Option<f32>,
    ) -> DsoRecord {
        let ndc_x = (2.0 * offset_px.0) / viewport.0;
        let ndc_y = (2.0 * offset_px.1) / viewport.1;
        let v_cam = info.projection.unproject_ndc(ndc_x, ndc_y);
        let v_icrs = (info.camera_to_icrs * v_cam).normalize();
        let (ra, dec) = cartesian_to_canonical_spherical(v_icrs);
        DsoRecord {
            ra_deg: ra.degrees(),
            dec_deg: dec.degrees(),
            kind: DsoKind::Galaxy,
            constellation: *b"And",
            messier,
            ngc: ngc.map(ToString::to_string),
            ic: None,
            alt_ngc: Vec::new(),
            common_names: Vec::new(),
            v_mag: Some(v_mag),
            b_mag: None,
            surface_brightness: None,
            major_arcmin,
            minor_arcmin: None,
            position_angle_deg: None,
            hubble_type: None,
        }
    }

    #[test]
    fn extract_skips_dso_when_opacity_or_mask_zero() {
        let mut mat = CelestialDsoMaterial {
            uniforms: CelestialDsoUniforms {
                transform: Mat4::IDENTITY,
                projection: CelestialProjectionUniforms {
                    fov_y: 1.0,
                    aspect_ratio: 1.0,
                    blend_factor: 0.0,
                    scale_factor: 1.0,
                },
                viewport_size: Vec2::new(800.0, 600.0),
                ngc_limiting_mag: 10.0,
                messier_boost: 2.2,
                fade_range: 1.2,
                opacity: 0.0,
                catalog_mask: 3,
                atmo_visibility: 1.0,
            },
            dsos: Handle::default(),
            dso_count: 10,
        };
        let mesh = Mesh3d(Handle::default());
        assert!(CelestialDsoMaterial::extract_component((&mat, &mesh)).is_none());

        mat.uniforms.opacity = 1.0;
        mat.uniforms.catalog_mask = 0;
        assert!(CelestialDsoMaterial::extract_component((&mat, &mesh)).is_none());

        mat.uniforms.catalog_mask = 1;
        mat.uniforms.atmo_visibility = 0.0;
        assert!(CelestialDsoMaterial::extract_component((&mat, &mesh)).is_none());

        mat.uniforms.atmo_visibility = 0.5;
        assert!(CelestialDsoMaterial::extract_component((&mat, &mesh)).is_some());
    }

    #[test]
    fn dso_picker_respects_catalog_filter_and_horizon() {
        let mut cam = PlanetariumCamera::default();
        cam.observer.location =
            celestial::coords::geodetic::GeodeticPosition::from_degrees(0.0, 0.0, 0.0);
        cam.observer.set_epoch_unix_seconds(1_742_472_000.0);
        cam.view_mode = crate::camera::ViewMode::Horizontal;
        cam.yaw = 0.0;
        cam.pitch = 15.0_f64.to_radians();
        cam.target_yaw = cam.yaw;
        cam.target_pitch = cam.pitch;
        cam.fov_y = 40.0_f64.to_radians();
        cam.target_fov_y = cam.fov_y;
        let info = cam.view_info(16.0 / 9.0);
        let viewport = (1600.0, 900.0);

        let records = vec![
            dso_at_offset_px(&info, viewport, (0.0, 0.0), Some(31), Some("224"), 3.44, Some(177.0)),
            dso_at_offset_px(&info, viewport, (40.0, 0.0), None, Some("7000"), 4.0, Some(120.0)),
        ];
        let index = DsoSpatialIndex::build(records);

        let params_both = DsoRenderParams::for_sky(-25.0, cam.fov_y as f32, 1.0, true, true);
        let picked_m = index
            .pick(&info, (0.0, 0.0), viewport, 1.0, &params_both, true, None)
            .expect("should pick M31");
        assert_eq!(picked_m.record.messier, Some(31));

        // Disabling Messier hides M31 (it still has NGC 224 unless NGC disabled too; test NGC-only filter!).
        let params_messier_only = DsoRenderParams::for_sky(-25.0, cam.fov_y as f32, 1.0, true, false);
        let ngc_ndc_x = (2.0 * 40.0) / viewport.0;
        assert!(
            index
                .pick(
                    &info,
                    (ngc_ndc_x, 0.0),
                    viewport,
                    1.0,
                    &params_messier_only,
                    true,
                    None
                )
                .is_none()
        );

        let high_horizon = HorizonProfile::flat(30.0, 360);
        assert!(
            index
                .pick(
                    &info,
                    (0.0, 0.0),
                    viewport,
                    1.0,
                    &params_both,
                    true,
                    Some(&high_horizon)
                )
                .is_none()
        );
    }

    #[test]
    fn visible_labels_suppresses_overlaps() {
        let (cam, info) = setup_test_view_info();
        let viewport = (1200.0, 800.0);
        let records = vec![
            dso_at_offset_px(&info, viewport, (0.0, 0.0), Some(31), Some("224"), 3.4, Some(10.0)),
            dso_at_offset_px(&info, viewport, (4.0, 0.0), Some(32), Some("221"), 8.1, Some(5.0)),
            dso_at_offset_px(&info, viewport, (150.0, 0.0), Some(110), Some("205"), 8.5, Some(10.0)),
        ];
        let index = DsoSpatialIndex::build(records);
        let params = DsoRenderParams::for_sky(-25.0, cam.fov_y as f32, 1.0, true, true);
        let labels = index.visible_labels(&info, viewport, 1.0, &params, false, None, 24);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].text, "M31");
        assert_eq!(labels[1].text, "M110");
    }

    #[test]
    fn bundled_dso_catalog_has_all_messier_and_iconic_ngc_objects() {
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/catalogs/dso.bin.zst"
        ));
        let file = File::open(&path).expect("open bundled dso.bin.zst");
        let catalog = read_dso_catalog_from_zst(file).expect("decode dso catalog");

        assert_eq!(catalog.header.messier_count, 110);
        assert!(
            catalog.header.ngc_count > 8_000,
            "expected > 8,000 NGC objects, got {}",
            catalog.header.ngc_count
        );

        let messier_set: HashSet<u16> = catalog
            .records
            .iter()
            .filter_map(|r| r.messier)
            .collect();
        for m in 1..=110_u16 {
            assert!(messier_set.contains(&m), "Missing Messier object M{m}");
        }

        for m in [1_u16, 13, 24, 25, 31, 40, 42, 45, 51, 57, 101, 102, 104] {
            assert!(
                catalog.records.iter().any(|r| r.messier == Some(m)),
                "Missing iconic Messier M{m}"
            );
        }
        let m102 = catalog
            .records
            .iter()
            .find(|r| r.messier == Some(102))
            .expect("M102 present");
        assert_eq!(m102.ngc.as_deref(), Some("5866"));

        for ngc_id in ["253", "281", "869", "1499", "2237", "2239", "5139", "6960", "7000", "7293"] {
            assert!(
                catalog
                    .records
                    .iter()
                    .any(|r| r.ngc.as_deref() == Some(ngc_id)),
                "Missing iconic NGC {ngc_id}"
            );
        }

        let ngc2239 = catalog
            .records
            .iter()
            .find(|r| r.ngc.as_deref() == Some("2239"))
            .expect("NGC 2239 present");
        assert!(
            ngc2239.alt_ngc.iter().any(|id| id == "2244"),
            "Expected NGC 2244 as cross-reference on NGC 2239, got {:?}",
            ngc2239.alt_ngc
        );
    }
}
