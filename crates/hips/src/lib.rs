pub mod grid;
pub mod survey;

pub use grid::{HipsGrid, HipsGridConfig};
pub use survey::{HipsSurvey, HipsTileFetcher, TileFetchError, TileFormat, TileMeshGeometry};

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use std::path::PathBuf;

    #[test]
    fn test_target_depth_clamping_wide_fov_and_hysteresis() {
        let config = HipsGridConfig {
            min_depth: 1,
            max_depth: 6,
            tile_px_size: 512,
            hysteresis_factor: 0.15,
        };

        // FOV >= 120 degrees always returns min_depth.
        assert_eq!(
            HipsGrid::target_depth(120.0_f64.to_radians(), 1024, None, &config),
            config.min_depth
        );
        assert_eq!(
            HipsGrid::target_depth(150.0_f64.to_radians(), 1024, Some(5), &config),
            config.min_depth
        );

        // Very narrow FOV clamps to max_depth.
        assert_eq!(
            HipsGrid::target_depth(1e-5, 1024, None, &config),
            config.max_depth
        );

        // Choose fov such that continuous_depth = 3.4:
        // target_tile_angle = (PI/3).sqrt() / 2^3.4
        let height_px = 1024u32;
        let sqrt_pi_over_3 = (PI / 3.0).sqrt();
        let target_tile_angle = sqrt_pi_over_3 / 2.0_f64.powf(3.4);
        let fov = (target_tile_angle * height_px as f64) / config.tile_px_size as f64;

        // Without current depth, rounds to 3.
        assert_eq!(HipsGrid::target_depth(fov, height_px, None, &config), 3);
        // |3.4 - 4.0| = 0.6 < 0.5 + 0.15 = 0.65 -> hysteresis holds depth 4.
        assert_eq!(HipsGrid::target_depth(fov, height_px, Some(4), &config), 4);
        // |3.4 - 2.0| = 1.4 >= 0.65 -> switches to 3.
        assert_eq!(HipsGrid::target_depth(fov, height_px, Some(2), &config), 3);
    }

    #[test]
    fn test_survey_tile_url_and_relative_path() {
        let jpg_survey = HipsSurvey {
            name: "CDS/P/2MASS/color".into(),
            base_url: "https://example.org/hips/".into(),
            max_depth: 9,
            format: TileFormat::Jpg,
        };
        assert_eq!(
            jpg_survey.tile_url(3, 12_345),
            "https://example.org/hips/Norder3/Dir10000/Npix12345.jpg"
        );
        assert_eq!(
            jpg_survey.tile_url(0, 5),
            "https://example.org/hips/Norder0/Dir0/Npix5.jpg"
        );
        assert_eq!(
            jpg_survey.tile_relative_path(4, 29_999),
            PathBuf::from("Norder4/Dir20000/Npix29999.jpg")
        );

        let png_survey = HipsSurvey {
            name: "Test PNG".into(),
            base_url: "https://example.org/png".into(),
            max_depth: 5,
            format: TileFormat::Png,
        };
        assert_eq!(
            png_survey.tile_url(2, 10_000),
            "https://example.org/png/Norder2/Dir10000/Npix10000.png"
        );
    }

    #[test]
    fn test_tile_fetcher_cache_path_single_slug() {
        let survey = HipsSurvey::dss_color();
        let fetcher = HipsTileFetcher::new(Some(PathBuf::from("/tmp/apogee-hips")));
        let path = fetcher
            .tile_cache_path(&survey, 3, 12_345)
            .expect("cache path present");

        assert_eq!(
            path,
            PathBuf::from("/tmp/apogee-hips/CDS_DSS_Color/Norder3/Dir10000/Npix12345.jpg")
        );
        // Ensure slug appears exactly once.
        let path_str = path.to_string_lossy();
        assert_eq!(path_str.matches(&survey.slug()).count(), 1);

        let no_cache = HipsTileFetcher::new(None);
        assert!(no_cache.tile_cache_path(&survey, 0, 0).is_none());
    }

    #[test]
    fn test_tile_surface_mesh_geometry() {
        let subdivisions = 4usize;
        let geom = HipsGrid::tile_surface_mesh(2, 42, subdivisions);

        let expected_vertices = (subdivisions + 1) * (subdivisions + 1);
        let expected_indices = 6 * subdivisions * subdivisions;
        assert_eq!(geom.positions.len(), expected_vertices);
        assert_eq!(geom.uvs.len(), expected_vertices);
        assert_eq!(geom.indices.len(), expected_indices);

        for pos in &geom.positions {
            assert!(pos.iter().all(|c| c.is_finite()));
            let len = (pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "non-unit vertex length: {len}");
        }
        for uv in &geom.uvs {
            assert!((0.0..=1.0).contains(&uv[0]));
            assert!((0.0..=1.0).contains(&uv[1]));
        }
    }

    #[test]
    fn test_cell_center_and_max_radius_bounds() {
        for depth in 0..=6_u8 {
            let nside_cells = 12 * 4_u64.pow(u32::from(depth));
            let layer = cdshealpix::nested::get(depth);
            let max_r = HipsGrid::max_cell_radius_rad(depth);

            for hash in [0_u64, nside_cells / 3, nside_cells - 1] {
                let center = HipsGrid::cell_center_icrs(depth, hash);
                assert!(
                    (center.length() - 1.0).abs() < 1e-12,
                    "center must be unit length at depth {depth}"
                );
                let (lon, lat) = HipsGrid::vector_to_spherical(center);
                assert_eq!(
                    layer.hash(lon, lat),
                    hash,
                    "center must map back to its own cell at depth {depth}"
                );

                for (p1, _) in HipsGrid::cell_boundary_segments(depth, hash, 4) {
                    let angle = center.dot(p1).clamp(-1.0, 1.0).acos();
                    assert!(
                        angle <= max_r,
                        "boundary vertex angle {angle} exceeds max_cell_radius_rad {max_r} at depth {depth}"
                    );
                }
            }
        }
    }
}
