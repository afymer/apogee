use bevy::{
    asset::RenderAssetUsages,
    mesh::MeshVertexAttribute,
    render::{
        mesh::Mesh,
        render_resource::{PrimitiveTopology, VertexFormat},
    },
};
use celestial::coords::{Angle, SphericalRect};
pub use celestial::coords::{canonical_spherical_to_cartesian, topocentric_spherical_to_enu};
use glam::Vec3;

pub const ATTRIBUTE_LINE_TIER: MeshVertexAttribute = MeshVertexAttribute::new(
    "LineTier",
    Mesh::FIRST_AVAILABLE_CUSTOM_ATTRIBUTE,
    VertexFormat::Float32,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridLineTier {
    Major,
    Minor,
}

impl GridLineTier {
    #[inline]
    pub const fn as_f32(self) -> f32 {
        match self {
            Self::Major => 0.0,
            Self::Minor => 1.0,
        }
    }
}

pub fn create_grid_mesh(bounds: SphericalRect, start_level: u32, subdivision_count: u32) -> Mesh {
    create_grid_mesh_mapped(&bounds, start_level, subdivision_count, |lon, lat| {
        canonical_spherical_to_cartesian(lon, lat).as_vec3()
    })
}

pub fn create_grid_mesh_mapped(
    bounds: &SphericalRect,
    start_level: u32,
    subdivision_count: u32,
    map_point: impl Fn(Angle, Angle) -> Vec3,
) -> Mesh {
    let n = subdivision_count.max(1);
    let major_cuts = (n as f64).powi(start_level.min(20) as i32);
    let major_step = Angle::from_radians(Angle::TAU.radians() / major_cuts);
    create_hierarchical_grid_mesh_mapped(bounds, major_step, major_step, n, map_point)
}

pub fn create_hierarchical_grid_mesh(
    bounds: SphericalRect,
    lon_major: Angle,
    lat_major: Angle,
    subdivisions: u32,
) -> Mesh {
    create_hierarchical_grid_mesh_mapped(&bounds, lon_major, lat_major, subdivisions, |lon, lat| {
        canonical_spherical_to_cartesian(lon, lat).as_vec3()
    })
}

pub fn create_hierarchical_grid_mesh_mapped(
    bounds: &SphericalRect,
    lon_major: Angle,
    lat_major: Angle,
    subdivisions: u32,
    map_point: impl Fn(Angle, Angle) -> Vec3,
) -> Mesh {
    let subdivisions = subdivisions.max(2) as i64;
    let tau = Angle::TAU.radians();

    let major_lon_cuts = (tau / lon_major.radians().abs().max(1e-12))
        .round()
        .clamp(1.0, 1e12) as i64;
    let total_lon_cuts = major_lon_cuts.saturating_mul(subdivisions);

    let major_lat_cuts = (tau / lat_major.radians().abs().max(1e-12))
        .round()
        .clamp(1.0, 1e12) as i64;
    let total_lat_cuts = major_lat_cuts.saturating_mul(subdivisions);
    let n_pole = total_lat_cuts / 4;

    let total_lon_cuts_f = total_lon_cuts as f64;
    let total_lat_cuts_f = total_lat_cuts as f64;

    let span_cuts_f = bounds.lon.span().radians() / tau * total_lon_cuts_f;
    let is_full_circle = span_cuts_f.round() >= total_lon_cuts_f;

    let mut positions: Vec<Vec3> = Vec::new();
    let mut line_tiers: Vec<f32> = Vec::new();

    // Parallels, aka "big circles" according to Stellarium
    let lat_start_f = bounds.lat.start().radians() / tau * total_lat_cuts_f;
    let lat_end_f = bounds.lat.end().radians() / tau * total_lat_cuts_f;
    let j_min = lat_start_f.ceil() as i64;
    let j_max = lat_end_f.floor() as i64;
    let j_start = j_min.max(-n_pole + 1);
    let j_end = j_max.min(n_pole - 1);
    let mut j_stride: i64 = 1;
    let lat_lines = j_end.saturating_sub(j_start).saturating_add(1);
    while lat_lines / j_stride > 144 {
        let Some(next) = j_stride.checked_mul(subdivisions) else {
            break;
        };
        if next >= total_lat_cuts || next <= j_stride {
            break;
        }
        j_stride = next;
    }
    let j_first = if j_stride > 1 {
        j_start.div_euclid(j_stride) * j_stride
            + if j_start.rem_euclid(j_stride) == 0 {
                0
            } else {
                j_stride
            }
    } else {
        j_start
    };

    let num_lon_segs = if is_full_circle {
        128
    } else {
        ((span_cuts_f.ceil() as usize).saturating_mul(4).max(8)).min(128)
    };

    let lon_start_rad = if is_full_circle {
        0.0
    } else {
        bounds.lon.start().radians()
    };
    let lon_span_rad = if is_full_circle {
        tau
    } else {
        bounds.lon.span().radians()
    };

    for j in (j_first..=j_end).step_by(j_stride as usize) {
        let lat = Angle::from_radians(tau * j as f64 / total_lat_cuts_f);
        let tier = if j % subdivisions == 0 || j == n_pole - 1 || j == -n_pole + 1 {
            GridLineTier::Major
        } else {
            GridLineTier::Minor
        };

        for s in 0..num_lon_segs {
            let lon0 = Angle::from_radians(
                lon_start_rad + (s as f64 / num_lon_segs as f64) * lon_span_rad,
            );
            let lon1 = Angle::from_radians(
                lon_start_rad + ((s + 1) as f64 / num_lon_segs as f64) * lon_span_rad,
            );
            positions.push(map_point(lon0, lat));
            positions.push(map_point(lon1, lat));
            line_tiers.push(tier.as_f32());
            line_tiers.push(tier.as_f32());
        }
    }

    // Pole limits: the last drawn parallels near the poles
    // Idea stolen from Stellarium to prevent very dense lines
    // near the poles
    let lat_north_limit = tau * (n_pole - 1) as f64 / total_lat_cuts_f;
    let lat_south_limit = -lat_north_limit;

    let reg_lat_start = bounds.lat.start().radians().max(lat_south_limit);
    let reg_lat_end = bounds.lat.end().radians().min(lat_north_limit);
    let reg_lat_span_rad = reg_lat_end - reg_lat_start;

    if reg_lat_span_rad > 0.0 {
        let reg_lat_cuts = (((reg_lat_span_rad / tau) * total_lat_cuts_f).ceil() as i64).max(1);
        let num_reg_lat_segs = (reg_lat_cuts.saturating_mul(4).max(4)).min(90) as usize;

        if is_full_circle {
            let mut stride: i64 = 1;
            while total_lon_cuts / stride > 72 {
                let Some(next) = stride.checked_mul(subdivisions) else {
                    break;
                };
                if next >= total_lon_cuts || next <= stride {
                    break;
                }
                stride = next;
            }

            for c in (0..total_lon_cuts).step_by(stride as usize) {
                let cut_index = c.rem_euclid(total_lon_cuts);
                let lon = Angle::from_radians(tau * cut_index as f64 / total_lon_cuts_f);
                let tier = if cut_index % subdivisions == 0 {
                    GridLineTier::Major
                } else {
                    GridLineTier::Minor
                };

                for s in 0..num_reg_lat_segs {
                    let lat0 = Angle::from_radians(
                        reg_lat_start + (s as f64 / num_reg_lat_segs as f64) * reg_lat_span_rad,
                    );
                    let lat1 = Angle::from_radians(
                        reg_lat_start
                            + ((s + 1) as f64 / num_reg_lat_segs as f64) * reg_lat_span_rad,
                    );
                    positions.push(map_point(lon, lat0));
                    positions.push(map_point(lon, lat1));
                    line_tiers.push(tier.as_f32());
                    line_tiers.push(tier.as_f32());
                }
            }
        } else {
            let start_cut_f = bounds.lon.start().radians() / tau * total_lon_cuts_f;
            let end_cut_f = start_cut_f + span_cuts_f;
            let end_cut = end_cut_f.floor() as i64;
            let span_cuts = span_cuts_f.ceil() as i64;
            let mut stride: i64 = 1;
            while span_cuts / stride > 72 {
                let Some(next) = stride.checked_mul(subdivisions) else {
                    break;
                };
                if next >= total_lon_cuts || next <= stride {
                    break;
                }
                stride = next;
            }

            let c_start = (start_cut_f / stride as f64).ceil() as i64 * stride;
            for c in (c_start..=end_cut).step_by(stride as usize) {
                let cut_index = c.rem_euclid(total_lon_cuts);
                let lon = Angle::from_radians(tau * cut_index as f64 / total_lon_cuts_f);
                let tier = if cut_index % subdivisions == 0 {
                    GridLineTier::Major
                } else {
                    GridLineTier::Minor
                };

                for s in 0..num_reg_lat_segs {
                    let lat0 = Angle::from_radians(
                        reg_lat_start + (s as f64 / num_reg_lat_segs as f64) * reg_lat_span_rad,
                    );
                    let lat1 = Angle::from_radians(
                        reg_lat_start
                            + ((s + 1) as f64 / num_reg_lat_segs as f64) * reg_lat_span_rad,
                    );
                    positions.push(map_point(lon, lat0));
                    positions.push(map_point(lon, lat1));
                    line_tiers.push(tier.as_f32());
                    line_tiers.push(tier.as_f32());
                }
            }
        }
    }

    let north_cap_start = bounds.lat.start().radians().max(lat_north_limit);
    let north_cap_end = bounds.lat.end().radians().min(tau / 4.0);
    if north_cap_start < north_cap_end {
        let span = north_cap_end - north_cap_start;
        let cap_cuts = (((span / tau) * total_lat_cuts_f).ceil() as i64).max(1);
        let num_cap_segs = (cap_cuts.saturating_mul(4).max(2)).min(45) as usize;

        for k in 0..4 {
            let lon = Angle::from_radians(k as f64 * (tau / 4.0));
            if is_full_circle || bounds.lon.contains(lon) {
                for s in 0..num_cap_segs {
                    let lat0 = Angle::from_radians(
                        north_cap_start + (s as f64 / num_cap_segs as f64) * span,
                    );
                    let lat1 = Angle::from_radians(
                        north_cap_start + ((s + 1) as f64 / num_cap_segs as f64) * span,
                    );
                    positions.push(map_point(lon, lat0));
                    positions.push(map_point(lon, lat1));
                    line_tiers.push(GridLineTier::Major.as_f32());
                    line_tiers.push(GridLineTier::Major.as_f32());
                }
            }
        }
    }

    let south_cap_start = bounds.lat.start().radians().max(-tau / 4.0);
    let south_cap_end = bounds.lat.end().radians().min(lat_south_limit);
    if south_cap_start < south_cap_end {
        let span = south_cap_end - south_cap_start;
        let cap_cuts = (((span / tau) * total_lat_cuts_f).ceil() as i64).max(1);
        let num_cap_segs = (cap_cuts.saturating_mul(4).max(2)).min(45) as usize;

        for k in 0..4 {
            let lon = Angle::from_radians(k as f64 * (tau / 4.0));
            if is_full_circle || bounds.lon.contains(lon) {
                for s in 0..num_cap_segs {
                    let lat0 = Angle::from_radians(
                        south_cap_start + (s as f64 / num_cap_segs as f64) * span,
                    );
                    let lat1 = Angle::from_radians(
                        south_cap_start + ((s + 1) as f64 / num_cap_segs as f64) * span,
                    );
                    positions.push(map_point(lon, lat0));
                    positions.push(map_point(lon, lat1));
                    line_tiers.push(GridLineTier::Major.as_f32());
                    line_tiers.push(GridLineTier::Major.as_f32());
                }
            }
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(ATTRIBUTE_LINE_TIER, line_tiers);
    mesh
}

pub fn create_horizon_mesh_mapped(map_point: impl Fn(Angle, Angle) -> Vec3) -> Mesh {
    let mut positions = Vec::new();
    let mut line_tiers = Vec::new();
    let segments = 360;
    let step = Angle::TAU.radians() / segments as f64;

    for i in 0..segments {
        let az0 = Angle::from_radians(i as f64 * step);
        let az1 = Angle::from_radians((i + 1) as f64 * step);
        positions.push(map_point(az0, Angle::ZERO));
        positions.push(map_point(az1, Angle::ZERO));
        line_tiers.push(GridLineTier::Major.as_f32());
        line_tiers.push(GridLineTier::Major.as_f32());
    }

    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(ATTRIBUTE_LINE_TIER, line_tiers);
    mesh
}

pub fn create_horizon_mesh(topo_to_cam: glam::DMat3) -> Mesh {
    create_horizon_mesh_mapped(move |az, alt| {
        let v_topo = topocentric_spherical_to_enu(az, alt);
        let v_cam = topo_to_cam.mul_vec3(v_topo);
        Vec3::new(v_cam.x as f32, v_cam.y as f32, v_cam.z as f32)
    })
}

pub fn create_altaz_grid_mesh(topo_to_cam: glam::DMat3) -> Mesh {
    create_grid_mesh_mapped(&SphericalRect::full_sphere(), 1, 6, move |az, alt| {
        let v_topo = topocentric_spherical_to_enu(az, alt);
        let v_cam = topo_to_cam.mul_vec3(v_topo);
        Vec3::new(v_cam.x as f32, v_cam.y as f32, v_cam.z as f32)
    })
}

pub fn create_equatorial_grid_mesh(eq_to_cam: glam::DMat3) -> Mesh {
    create_grid_mesh_mapped(&SphericalRect::full_sphere(), 1, 6, move |lon, lat| {
        let v_eq = canonical_spherical_to_cartesian(lon, lat);
        let v_cam = eq_to_cam.mul_vec3(v_eq);
        Vec3::new(v_cam.x as f32, v_cam.y as f32, v_cam.z as f32)
    })
}
