//! Procedural 3D mesh builder for shaded geometry, translucent limit widgets, and lines.

use std::f32::consts::TAU;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct MeshBuilder {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn push_vertex(&mut self, pos: Vec3, normal: Vec3, color: [f32; 4]) -> u32 {
        let idx = self.vertices.len() as u32;
        self.vertices.push(Vertex {
            position: pos.to_array(),
            normal: normal.normalize_or_zero().to_array(),
            color,
        });
        idx
    }

    pub fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.push(a);
        self.indices.push(b);
        self.indices.push(c);
    }

    pub fn push_quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.push_tri(a, b, c);
        self.push_tri(a, c, d);
    }

    /// Adds a box centered at the origin of `transform` with half-extents `size * 0.5`.
    pub fn add_box(&mut self, transform: Mat4, size: Vec3, color: [f32; 4]) {
        let hx = size.x * 0.5;
        let hy = size.y * 0.5;
        let hz = size.z * 0.5;

        let faces: [([Vec3; 4], Vec3); 6] = [
            (
                [
                    Vec3::new(hx, -hy, hz),
                    Vec3::new(hx, -hy, -hz),
                    Vec3::new(hx, hy, -hz),
                    Vec3::new(hx, hy, hz),
                ],
                Vec3::X,
            ),
            (
                [
                    Vec3::new(-hx, -hy, -hz),
                    Vec3::new(-hx, -hy, hz),
                    Vec3::new(-hx, hy, hz),
                    Vec3::new(-hx, hy, -hz),
                ],
                Vec3::NEG_X,
            ),
            (
                [
                    Vec3::new(-hx, hy, hz),
                    Vec3::new(hx, hy, hz),
                    Vec3::new(hx, hy, -hz),
                    Vec3::new(-hx, hy, -hz),
                ],
                Vec3::Y,
            ),
            (
                [
                    Vec3::new(-hx, -hy, -hz),
                    Vec3::new(hx, -hy, -hz),
                    Vec3::new(hx, -hy, hz),
                    Vec3::new(-hx, -hy, hz),
                ],
                Vec3::NEG_Y,
            ),
            (
                [
                    Vec3::new(-hx, -hy, hz),
                    Vec3::new(hx, -hy, hz),
                    Vec3::new(hx, hy, hz),
                    Vec3::new(-hx, hy, hz),
                ],
                Vec3::Z,
            ),
            (
                [
                    Vec3::new(hx, -hy, -hz),
                    Vec3::new(-hx, -hy, -hz),
                    Vec3::new(-hx, hy, -hz),
                    Vec3::new(hx, hy, -hz),
                ],
                Vec3::NEG_Z,
            ),
        ];

        for (corners, local_n) in faces {
            let n = transform.transform_vector3(local_n).normalize_or_zero();
            let v0 = self.push_vertex(transform.transform_point3(corners[0]), n, color);
            let v1 = self.push_vertex(transform.transform_point3(corners[1]), n, color);
            let v2 = self.push_vertex(transform.transform_point3(corners[2]), n, color);
            let v3 = self.push_vertex(transform.transform_point3(corners[3]), n, color);
            self.push_quad(v0, v1, v2, v3);
        }
    }

    /// Adds a capped cylinder/frustum aligned along local `+Y` between `y0` and `y1`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_frustum(
        &mut self,
        transform: Mat4,
        r0: f32,
        r1: f32,
        y0: f32,
        y1: f32,
        segments: u32,
        color: [f32; 4],
    ) {
        let (r0, r1, y0, y1) = if y0 <= y1 {
            (r0, r1, y0, y1)
        } else {
            (r1, r0, y1, y0)
        };
        let segs = segments.max(6);
        let slope_y = (r0 - r1) / (y1 - y0).max(1e-4);

        // Side surface with shared ring vertices (no duplicate seam vertices)
        let ring0_base = self.vertices.len() as u32;
        for i in 0..segs {
            let a = (i as f32 / segs as f32) * TAU;
            let (s, c) = a.sin_cos();
            let n = transform
                .transform_vector3(Vec3::new(c, slope_y, s))
                .normalize_or_zero();
            self.push_vertex(
                transform.transform_point3(Vec3::new(c * r0, y0, s * r0)),
                n,
                color,
            );
            self.push_vertex(
                transform.transform_point3(Vec3::new(c * r1, y1, s * r1)),
                n,
                color,
            );
        }
        for i in 0..segs {
            let next = (i + 1) % segs;
            let v00 = ring0_base + i * 2;
            let v01 = v00 + 1;
            let v10 = ring0_base + next * 2;
            let v11 = v10 + 1;
            self.push_quad(v00, v01, v11, v10);
        }

        // Bottom cap (shared ring vertices)
        if r0 > 1e-4 {
            let n_bot = transform.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
            let center = self.push_vertex(
                transform.transform_point3(Vec3::new(0.0, y0, 0.0)),
                n_bot,
                color,
            );
            let ring_base = self.vertices.len() as u32;
            for i in 0..segs {
                let a = (i as f32 / segs as f32) * TAU;
                self.push_vertex(
                    transform.transform_point3(Vec3::new(a.cos() * r0, y0, a.sin() * r0)),
                    n_bot,
                    color,
                );
            }
            for i in 0..segs {
                let next = (i + 1) % segs;
                self.push_tri(center, ring_base + i, ring_base + next);
            }
        }

        // Top cap (shared ring vertices)
        if r1 > 1e-4 {
            let n_top = transform.transform_vector3(Vec3::Y).normalize_or_zero();
            let center = self.push_vertex(
                transform.transform_point3(Vec3::new(0.0, y1, 0.0)),
                n_top,
                color,
            );
            let ring_base = self.vertices.len() as u32;
            for i in 0..segs {
                let a = (i as f32 / segs as f32) * TAU;
                self.push_vertex(
                    transform.transform_point3(Vec3::new(a.cos() * r1, y1, a.sin() * r1)),
                    n_top,
                    color,
                );
            }
            for i in 0..segs {
                let next = (i + 1) % segs;
                self.push_tri(center, ring_base + next, ring_base + i);
            }
        }
    }

    /// Adds a cylinder between world points `p0` and `p1`.
    pub fn add_strut(&mut self, p0: Vec3, p1: Vec3, radius: f32, segments: u32, color: [f32; 4]) {
        let axis = p1 - p0;
        let len = axis.length();
        if len < 1e-5 {
            return;
        }
        let dir = axis / len;
        let transform = frame_from_y_axis(p0, dir);
        self.add_frustum(transform, radius, radius, 0.0, len, segments, color);
    }

    /// Adds a spherical band (shared vertex grid) centered at `center`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_spherical_band(
        &mut self,
        center: Vec3,
        radius: f32,
        alt_min_rad: f32,
        alt_max_rad: f32,
        az_start_rad: f32,
        az_sweep_rad: f32,
        az_steps: u32,
        alt_steps: u32,
        color: [f32; 4],
    ) {
        let az_n = az_steps.max(4);
        let alt_n = alt_steps.max(1);
        let stride = az_n + 1;
        let base = self.vertices.len() as u32;

        for ia in 0..=alt_n {
            let t = ia as f32 / alt_n as f32;
            let alt = alt_min_rad + (alt_max_rad - alt_min_rad) * t;
            for iz in 0..=az_n {
                let u = iz as f32 / az_n as f32;
                let az = az_start_rad + az_sweep_rad * u;
                let d = horizontal_dir(az, alt);
                self.push_vertex(center + d * radius, d, color);
            }
        }

        for ia in 0..alt_n {
            for iz in 0..az_n {
                let v00 = base + ia * stride + iz;
                let v10 = v00 + 1;
                let v01 = v00 + stride;
                let v11 = v01 + 1;
                self.push_quad(v00, v10, v11, v01);
            }
        }
    }
}

#[derive(Default)]
pub struct LineBuilder {
    pub vertices: Vec<Vertex>,
}

impl LineBuilder {
    pub fn add_segment(&mut self, a: Vec3, b: Vec3, color: [f32; 4]) {
        self.vertices.push(Vertex {
            position: a.to_array(),
            normal: [0.0, 1.0, 0.0],
            color,
        });
        self.vertices.push(Vertex {
            position: b.to_array(),
            normal: [0.0, 1.0, 0.0],
            color,
        });
    }

    pub fn add_polyline(&mut self, points: &[Vec3], color: [f32; 4]) {
        for pair in points.windows(2) {
            self.add_segment(pair[0], pair[1], color);
        }
    }

    pub fn add_circle(
        &mut self,
        center: Vec3,
        u: Vec3,
        v: Vec3,
        radius: f32,
        segments: u32,
        color: [f32; 4],
    ) {
        let segs = segments.max(12);
        let mut prev = center + u * radius;
        for i in 1..=segs {
            let a = (i as f32 / segs as f32) * TAU;
            let next = center + (u * a.cos() + v * a.sin()) * radius;
            self.add_segment(prev, next, color);
            prev = next;
        }
    }
}

/// Converts horizontal `(az_rad, alt_rad)` into a right-handed Y-up unit vector
/// (`+X = East`, `+Y = Up`, `-Z = North`, `+Z = South`).
pub fn horizontal_dir(az_rad: f32, alt_rad: f32) -> Vec3 {
    let (sin_alt, cos_alt) = alt_rad.sin_cos();
    let (sin_az, cos_az) = az_rad.sin_cos();
    Vec3::new(cos_alt * sin_az, sin_alt, -cos_alt * cos_az)
}

/// Constructs a rigid transform whose origin is `origin` and whose local `+Y` axis points along `y_dir`.
pub fn frame_from_y_axis(origin: Vec3, y_dir: Vec3) -> Mat4 {
    let y = y_dir.normalize_or_zero();
    let ref_v = if y.y.abs() < 0.92 { Vec3::Y } else { Vec3::X };
    let x = y.cross(ref_v).normalize_or_zero();
    let z = x.cross(y).normalize_or_zero();
    Mat4::from_cols(
        x.extend(0.0),
        y.extend(0.0),
        z.extend(0.0),
        origin.extend(1.0),
    )
}

#[cfg(test)]
pub(crate) fn assert_outward_ccw_winding(mesh: &MeshBuilder) {
    assert_ne!(mesh.indices.len(), 0);
    for tri in mesh.indices.as_chunks::<3>().0 {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let p0 = Vec3::from_array(v0.position);
        let p1 = Vec3::from_array(v1.position);
        let p2 = Vec3::from_array(v2.position);
        let geo_n = (p1 - p0).cross(p2 - p0);
        let avg_n =
            Vec3::from_array(v0.normal) + Vec3::from_array(v1.normal) + Vec3::from_array(v2.normal);
        assert!(
            geo_n.dot(avg_n) > 0.0,
            "Inverted triangle winding: geo_n={geo_n:?}, avg_n={avg_n:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_have_outward_ccw_winding_including_inverted_y() {
        let mut box_mesh = MeshBuilder::default();
        box_mesh.add_box(Mat4::IDENTITY, Vec3::new(1.0, 2.0, 3.0), [1.0; 4]);
        assert_outward_ccw_winding(&box_mesh);

        let mut frustum_mesh = MeshBuilder::default();
        frustum_mesh.add_frustum(Mat4::IDENTITY, 0.5, 0.3, -0.4, 0.8, 16, [1.0; 4]);
        frustum_mesh.add_frustum(Mat4::IDENTITY, 0.3, 0.5, 0.8, -0.4, 16, [1.0; 4]);
        frustum_mesh.add_strut(Vec3::ZERO, Vec3::new(0.4, 0.9, -0.3), 0.05, 12, [1.0; 4]);
        assert_outward_ccw_winding(&frustum_mesh);
    }
}
