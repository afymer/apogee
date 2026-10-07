use glam::DVec3;
use std::f64::consts::{FRAC_PI_2, PI};

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
        if fov_y_rad >= (120.0_f64).to_radians() {
            return config.min_depth;
        }

        let h = (viewport_height_px as f64).max(1.0);
        let fov = fov_y_rad.clamp(1e-6, PI);
        let tile_size = config.tile_px_size as f64;

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

        target_d.clamp(config.min_depth, config.max_depth)
    }

    #[inline]
    pub fn vector_to_spherical(v: DVec3) -> (f64, f64) {
        let norm = v.normalize();
        let lat = norm.z.clamp(-1.0, 1.0).asin();
        let lon = norm.y.atan2(norm.x).rem_euclid(2.0 * PI);
        (lon, lat)
    }

    #[inline]
    pub fn spherical_to_vector(lon: f64, lat: f64) -> DVec3 {
        let (sin_lat, cos_lat) = lat.sin_cos();
        let (sin_lon, cos_lon) = lon.sin_cos();
        DVec3::new(cos_lat * cos_lon, cos_lat * sin_lon, sin_lat)
    }

    pub fn query_visible_cells(center_icrs: DVec3, radius_rad: f64, depth: u8) -> Vec<u64> {
        let total_cells = cdshealpix::nested::n_hash(depth);
        if radius_rad >= FRAC_PI_2 {
            return (0..total_cells).collect();
        }

        let (lon, lat) = Self::vector_to_spherical(center_icrs);
        let layer = cdshealpix::nested::get(depth);
        let bmoc = layer.cone_coverage_approx(lon, lat, radius_rad);
        let mut cells: Vec<u64> = bmoc.flat_iter().collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    #[inline]
    pub fn default_segments_per_edge(depth: u8) -> usize {
        match depth {
            0 => 8,
            1 | 2 => 4,
            3 => 2,
            _ => 1,
        }
    }

    #[inline]
    fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
        let dot = a.dot(b).clamp(-1.0, 1.0);
        let omega = dot.acos();
        let sin_omega = omega.sin();

        if sin_omega.abs() < 1e-12 {
            a.lerp(b, t).normalize()
        } else {
            let scale_a = ((1.0 - t) * omega).sin() / sin_omega;
            let scale_b = (t * omega).sin() / sin_omega;
            (scale_a * a + scale_b * b).normalize()
        }
    }

    pub fn cell_boundary_segments(
        depth: u8,
        hash: u64,
        segments_per_edge: usize,
    ) -> Vec<(DVec3, DVec3)> {
        let layer = cdshealpix::nested::get(depth);
        // vertices: [South, East, North, West]
        let vertices = layer.vertices(hash);
        let corners = [
            Self::spherical_to_vector(vertices[0].0, vertices[0].1),
            Self::spherical_to_vector(vertices[1].0, vertices[1].1),
            Self::spherical_to_vector(vertices[2].0, vertices[2].1),
            Self::spherical_to_vector(vertices[3].0, vertices[3].1),
        ];

        let n = segments_per_edge.max(1);
        let mut segments = Vec::with_capacity(4 * n);

        for edge_idx in 0..4 {
            let p_start = corners[edge_idx];
            let p_end = corners[(edge_idx + 1) % 4];

            let mut prev = p_start;
            for i in 1..=n {
                let t = i as f64 / n as f64;
                let next = Self::slerp(p_start, p_end, t);
                segments.push((prev, next));
                prev = next;
            }
        }

        segments
    }
}
