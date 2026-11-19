use glam::DVec3;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HipsGridConfig {
    pub min_depth: u8,
    pub max_depth: u8,
    pub tile_px_size: u32,
    pub hysteresis_factor: f64,
}

impl Default for HipsGridConfig {
    fn default() -> Self {
        Self {
            min_depth: 0,
            max_depth: 8,
            tile_px_size: 512,
            hysteresis_factor: 0.10,
        }
    }
}

pub struct HipsGrid;

impl HipsGrid {
    pub fn target_depth(
        fov_y_rad: f64,
        viewport_height_px: u32,
        current_depth: Option<u8>,
        config: &HipsGridConfig,
    ) -> u8 {
        let min_d = config.min_depth.min(config.max_depth);
        let max_d = config.min_depth.max(config.max_depth);
        if fov_y_rad >= (120.0_f64).to_radians() {
            return min_d;
        }

        let h = (viewport_height_px as f64).max(1.0);
        let fov = fov_y_rad.clamp(1e-6, PI);
        let tile_size = config.tile_px_size.max(1) as f64;

        let sqrt_pi_over_3 = (PI / 3.0).sqrt();
        let target_tile_angle = tile_size * (fov / h);
        let continuous_depth = (sqrt_pi_over_3 / target_tile_angle).log2();

        let target_d = if let Some(curr) = current_depth {
            let curr_f = curr as f64;
            let threshold = 0.5 + config.hysteresis_factor;
            if (continuous_depth - curr_f).abs() < threshold {
                curr
            } else {
                continuous_depth.round().max(0.0) as u8
            }
        } else {
            continuous_depth.round().max(0.0) as u8
        };

        target_d.clamp(min_d, max_d)
    }

    #[inline]
    pub fn vector_to_spherical(v: DVec3) -> (f64, f64) {
        let (lon, lat) = celestial::coords::cartesian_to_canonical_spherical(v);
        (lon.radians(), lat.radians())
    }

    #[inline]
    pub fn spherical_to_vector(lon: f64, lat: f64) -> DVec3 {
        celestial::coords::canonical_spherical_to_cartesian(
            celestial::coords::Angle::from_radians(lon),
            celestial::coords::Angle::from_radians(lat),
        )
    }

    pub fn query_visible_cells(center_icrs: DVec3, radius_rad: f64, depth: u8) -> Vec<u64> {
        let (lon, lat) = Self::vector_to_spherical(center_icrs);
        let layer = cdshealpix::nested::get(depth.min(29));
        let radius = radius_rad.clamp(1e-6, PI);
        let bmoc = layer.cone_coverage_approx(lon, lat, radius);
        let mut cells: Vec<u64> = bmoc.flat_iter().collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    #[inline]
    pub fn default_segments_per_edge(depth: u8) -> usize {
        Self::default_tile_subdivisions(depth)
    }

    pub fn cell_boundary_segments(
        depth: u8,
        hash: u64,
        segments_per_edge: usize,
    ) -> Vec<(DVec3, DVec3)> {
        let layer = cdshealpix::nested::get(depth);
        let n = segments_per_edge.max(1) as u32;
        let points =
            layer.path_along_cell_edge(hash, &cdshealpix::compass_point::Cardinal::S, true, n);
        let len = points.len();
        let mut segments = Vec::with_capacity(len);

        for i in 0..len {
            let (lon1, lat1) = points[i];
            let (lon2, lat2) = points[(i + 1) % len];
            let p1 = Self::spherical_to_vector(lon1, lat1);
            let p2 = Self::spherical_to_vector(lon2, lat2);
            segments.push((p1, p2));
        }

        segments
    }
}
