//! Lightweight wgpu 3D overlay renderer for the NEQ6-Pro mount & infinity limit widgets.
//!
//! Renders directly onto the Planetarium's render target (`LoadOp::Load`) inside the same
//! `wgpu::CommandEncoder`, so the sky is 100% shared with `Explore` with zero frame lag.

use bytemuck::{Pod, Zeroable};
use celestial::projection::CameraProjection;
use glam::{Mat4, Vec3};

use crate::mesh::Vertex;
use crate::scene::{MountSceneState, SceneGeometry, build_scene};

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const BASE_FOV_Y_DEG: f32 = 42.0;
pub const MAX_FOV_Y_DEG: f32 = 155.0;
pub const MIN_ORBIT_DISTANCE: f32 = 1.8;
pub const MAX_ORBIT_DISTANCE: f32 = 6.5;

const SHADER_CODE: &str = include_str!("shader/mount.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniforms {
    view: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    proj_params: [f32; 4],
    sun_dir_intensity: [f32; 4],
    sun_color_night_w: [f32; 4],
    twilight_dir_weight: [f32; 4],
    sky_ambient: [f32; 4],
    gnd_ambient: [f32; 4],
    rim_color: [f32; 4],
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let denom = (edge1 - edge0).max(1e-6);
    let t = ((x - edge0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(b, t)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub distance: f32,
    pub fov_y_deg: f32,
    pub target: Vec3,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw_rad: 0.68,
            pitch_rad: 0.36,
            distance: 4.3,
            fov_y_deg: BASE_FOV_Y_DEG,
            target: Vec3::new(0.0, 1.08, 0.0),
        }
    }
}

impl OrbitCamera {
    pub fn preset_three_quarter() -> Self {
        Self::default()
    }

    pub fn preset_north() -> Self {
        Self {
            yaw_rad: 0.0,
            pitch_rad: 0.22,
            distance: 4.2,
            fov_y_deg: BASE_FOV_Y_DEG,
            target: Vec3::new(0.0, 1.10, 0.0),
        }
    }

    pub fn preset_east() -> Self {
        Self {
            yaw_rad: std::f32::consts::FRAC_PI_2,
            pitch_rad: 0.22,
            distance: 4.2,
            fov_y_deg: BASE_FOV_Y_DEG,
            target: Vec3::new(0.0, 1.10, 0.0),
        }
    }

    pub fn preset_top() -> Self {
        Self {
            yaw_rad: 0.0,
            pitch_rad: 1.48,
            distance: 4.8,
            fov_y_deg: BASE_FOV_Y_DEG,
            target: Vec3::new(0.0, 1.10, 0.0),
        }
    }

    /// Positions the camera behind the mount looking up along the optical axis into the sky.
    pub fn preset_along_ota(ota_dir: Vec3) -> Self {
        let dir = ota_dir.normalize_or_zero();
        let offset = -dir;
        let yaw_rad = offset.x.atan2(offset.z);
        let pitch_rad = offset.y.clamp(-0.98, 0.98).asin().clamp(-1.15, 1.45);
        Self {
            yaw_rad,
            pitch_rad,
            distance: 3.1,
            fov_y_deg: BASE_FOV_Y_DEG,
            target: Vec3::new(0.0, 1.22, 0.0) + dir * 0.45,
        }
    }

    /// Zooms the camera smoothly: adjusts orbit distance in `[MIN_ORBIT_DISTANCE, MAX_ORBIT_DISTANCE]`,
    /// and seamlessly widens vertical FOV up to `MAX_FOV_Y_DEG` when zooming out past `MAX_ORBIT_DISTANCE`.
    pub fn zoom(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }

        if factor > 1.0 {
            let next_dist = self.distance * factor;
            if next_dist <= MAX_ORBIT_DISTANCE {
                self.distance = next_dist;
            } else {
                let overflow = next_dist / MAX_ORBIT_DISTANCE;
                self.distance = MAX_ORBIT_DISTANCE;
                self.fov_y_deg = (self.fov_y_deg * overflow).clamp(BASE_FOV_Y_DEG, MAX_FOV_Y_DEG);
            }
        } else if self.fov_y_deg > BASE_FOV_Y_DEG {
            let next_fov = self.fov_y_deg * factor;
            if next_fov >= BASE_FOV_Y_DEG {
                self.fov_y_deg = next_fov;
            } else {
                let underflow = next_fov / BASE_FOV_Y_DEG;
                self.fov_y_deg = BASE_FOV_Y_DEG;
                self.distance =
                    (self.distance * underflow).clamp(MIN_ORBIT_DISTANCE, MAX_ORBIT_DISTANCE);
            }
        } else {
            self.distance = (self.distance * factor).clamp(MIN_ORBIT_DISTANCE, MAX_ORBIT_DISTANCE);
        }
    }

    /// Returns `(az_rad, alt_rad, fov_y_rad)` matching the Planetarium's topocentric camera.
    pub fn to_topocentric_angles(&self) -> (f64, f64, f64) {
        let clamped_pitch = self.pitch_rad.clamp(-1.52, 1.52);
        (
            (-f64::from(self.yaw_rad)).rem_euclid(std::f64::consts::TAU),
            -f64::from(clamped_pitch),
            f64::from(self.fov_y_deg).to_radians(),
        )
    }

    pub fn eye_position(&self) -> Vec3 {
        let clamped_pitch = self.pitch_rad.clamp(-1.52, 1.52);
        let (sin_p, cos_p) = clamped_pitch.sin_cos();
        let (sin_y, cos_y) = self.yaw_rad.sin_cos();
        self.target
            + Vec3::new(
                self.distance * cos_p * sin_y,
                self.distance * sin_p,
                self.distance * cos_p * cos_y,
            )
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye_position(), self.target, Vec3::Y)
    }
}

/// Reusable GPU buffer that grows on demand and updates in place via `queue.write_buffer`,
/// eliminating per-frame GPU buffer allocations while the mount is slewing.
struct GrowableBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: u64,
    label: &'static str,
    usage: wgpu::BufferUsages,
}

impl GrowableBuffer {
    fn write_slice<T: Pod>(
        slot: &mut Option<Self>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        usage: wgpu::BufferUsages,
        data: &[T],
    ) -> u32 {
        if data.is_empty() {
            return 0;
        }
        let bytes = bytemuck::cast_slice(data);
        let needed = (bytes.len() as u64).max(4);
        let needs_alloc = slot
            .as_ref()
            .is_none_or(|buf| buf.capacity_bytes < needed);
        if needs_alloc {
            let cap = needed.next_power_of_two().max(1024);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: cap,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            *slot = Some(Self {
                buffer,
                capacity_bytes: cap,
                label,
                usage,
            });
        }
        if let Some(buf) = slot.as_ref() {
            let _ = (buf.label, buf.usage);
            queue.write_buffer(&buf.buffer, 0, bytes);
        }
        data.len() as u32
    }
}

#[derive(Default)]
struct DynamicGpuMesh {
    vb: Option<GrowableBuffer>,
    ib: Option<GrowableBuffer>,
    index_count: u32,
}

impl DynamicGpuMesh {
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vb_label: &'static str,
        ib_label: &'static str,
        vertices: &[Vertex],
        indices: &[u32],
    ) {
        if indices.is_empty() {
            self.index_count = 0;
            return;
        }
        GrowableBuffer::write_slice(
            &mut self.vb,
            device,
            queue,
            vb_label,
            wgpu::BufferUsages::VERTEX,
            vertices,
        );
        self.index_count = GrowableBuffer::write_slice(
            &mut self.ib,
            device,
            queue,
            ib_label,
            wgpu::BufferUsages::INDEX,
            indices,
        );
    }
}

#[derive(Default)]
struct DynamicGpuLines {
    vb: Option<GrowableBuffer>,
    vertex_count: u32,
}

impl DynamicGpuLines {
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        vertices: &[Vertex],
    ) {
        self.vertex_count = GrowableBuffer::write_slice(
            &mut self.vb,
            device,
            queue,
            label,
            wgpu::BufferUsages::VERTEX,
            vertices,
        );
    }
}

pub struct MountOverlayRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    infinity_mesh_pipeline: wgpu::RenderPipeline,
    infinity_line_pipeline: wgpu::RenderPipeline,
    opaque_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    camera_uniform_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_texture: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    sky_translucent_mesh: DynamicGpuMesh,
    sky_lines: DynamicGpuLines,
    opaque_mesh: DynamicGpuMesh,
    lines: DynamicGpuLines,
    scene_state: MountSceneState,
    geometry_dirty: bool,
}

impl MountOverlayRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::new_with_format(device, queue, TARGET_FORMAT)
    }

    pub fn new_with_format(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mount 3D Overlay Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_CODE.into()),
        });

        let camera_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mount Camera Uniform Buffer"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mount Camera Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Mount Camera Bind Group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mount Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };

        let make_pipeline = |label: &'static str,
                             vs_entry: &'static str,
                             fs_entry: &'static str,
                             blend: wgpu::BlendState,
                             topology: wgpu::PrimitiveTopology,
                             cull_mode: Option<wgpu::Face>,
                             depth_write: bool,
                             depth_compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs_entry),
                    buffers: std::slice::from_ref(&vertex_layout),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let infinity_mesh_pipeline = make_pipeline(
            "Mount Infinity Zone Pipeline",
            "vs_infinity",
            "fs_unlit",
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::TriangleList,
            None,
            false,
            wgpu::CompareFunction::Always,
        );
        let infinity_line_pipeline = make_pipeline(
            "Mount Infinity Line Pipeline",
            "vs_infinity",
            "fs_unlit",
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::LineList,
            None,
            false,
            wgpu::CompareFunction::Always,
        );
        let opaque_pipeline = make_pipeline(
            "Mount Opaque Pipeline",
            "vs_lit",
            "fs_lit",
            wgpu::BlendState::REPLACE,
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Back),
            true,
            wgpu::CompareFunction::Less,
        );
        let line_pipeline = make_pipeline(
            "Mount Line Pipeline",
            "vs_line",
            "fs_unlit",
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::LineList,
            None,
            false,
            wgpu::CompareFunction::LessEqual,
        );

        Self {
            device,
            queue,
            infinity_mesh_pipeline,
            infinity_line_pipeline,
            opaque_pipeline,
            line_pipeline,
            camera_uniform_buffer,
            camera_bind_group,
            depth_texture: None,
            sky_translucent_mesh: DynamicGpuMesh::default(),
            sky_lines: DynamicGpuLines::default(),
            opaque_mesh: DynamicGpuMesh::default(),
            lines: DynamicGpuLines::default(),
            scene_state: MountSceneState::default(),
            geometry_dirty: true,
        }
    }

    fn upload_geometry(&mut self, geo: &SceneGeometry) {
        self.opaque_mesh.upload(
            &self.device,
            &self.queue,
            "Mount Opaque VB",
            "Mount Opaque IB",
            &geo.opaque.vertices,
            &geo.opaque.indices,
        );
        self.sky_translucent_mesh.upload(
            &self.device,
            &self.queue,
            "Mount Sky Translucent VB",
            "Mount Sky Translucent IB",
            &geo.sky_translucent.vertices,
            &geo.sky_translucent.indices,
        );
        self.sky_lines.upload(
            &self.device,
            &self.queue,
            "Mount Sky Line VB",
            &geo.sky_lines.vertices,
        );
        self.lines.upload(
            &self.device,
            &self.queue,
            "Mount Line VB",
            &geo.lines.vertices,
        );
    }

    fn ensure_depth_target(&mut self, width: u32, height: u32) {
        let needs_depth = self
            .depth_texture
            .as_ref()
            .is_none_or(|(_, _, w, h)| *w != width || *h != height);
        if needs_depth {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Mount 3D Depth Target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.depth_texture = Some((texture, view, width, height));
        }
    }

    /// Composites the at-infinity limit widgets and 3D NEQ6-Pro mount directly onto
    /// `color_view` (`LoadOp::Load`), preserving the Planetarium sky rendered underneath.
    #[allow(clippy::too_many_arguments)]
    pub fn render_into(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: OrbitCamera,
        scene_state: &MountSceneState,
        sun_dir_alt: [f32; 4],
    ) {
        let w = width.max(1);
        let h = height.max(1);
        self.ensure_depth_target(w, h);

        if self.geometry_dirty || &self.scene_state != scene_state {
            self.scene_state = scene_state.clone();
            let geo = build_scene(&self.scene_state);
            self.upload_geometry(&geo);
            self.geometry_dirty = false;
        }

        let aspect = (w as f32 / h as f32).max(0.1);
        let view = camera.view_matrix();
        let eye = camera.eye_position();
        let proj = CameraProjection::for_fov(f64::from(camera.fov_y_deg).to_radians(), aspect);

        // Precompute solar phase lighting once per frame on CPU instead of per fragment.
        let sun_dir = Vec3::new(sun_dir_alt[0], sun_dir_alt[1], sun_dir_alt[2])
            .try_normalize()
            .unwrap_or(Vec3::Y);
        let sun_alt = sun_dir_alt[3];
        let tw_t = smoothstep(-14.0, -2.0, sun_alt);
        let day_t = smoothstep(-2.0, 12.0, sun_alt);
        let sun_intensity = smoothstep(-1.5, 6.0, sun_alt);
        let sun_warmth = smoothstep(0.0, 22.0, sun_alt);

        let sun_color = lerp3(Vec3::new(1.0, 0.56, 0.30), Vec3::new(1.0, 0.96, 0.90), sun_warmth);
        let horiz_sun_len = sun_dir.x.hypot(sun_dir.z);
        let twilight_dir = if horiz_sun_len > 1e-4 {
            Vec3::new(sun_dir.x, 0.22 * horiz_sun_len, sun_dir.z).normalize()
        } else {
            Vec3::Y
        };
        let twilight_weight = tw_t * (1.0 - day_t * 0.85);
        let sky_ambient = lerp3(
            lerp3(Vec3::new(0.17, 0.22, 0.32), Vec3::new(0.24, 0.22, 0.32), tw_t),
            Vec3::new(0.30, 0.39, 0.54),
            day_t,
        );
        let gnd_ambient = lerp3(
            lerp3(Vec3::new(0.07, 0.09, 0.13), Vec3::new(0.09, 0.09, 0.12), tw_t),
            Vec3::new(0.16, 0.15, 0.14),
            day_t,
        );
        let rim_color = lerp3(Vec3::new(0.18, 0.24, 0.36), sun_color * 0.45, sun_intensity);

        let uniforms = CameraUniforms {
            view: view.to_cols_array_2d(),
            camera_pos: [eye.x, eye.y, eye.z, 1.0],
            proj_params: [
                aspect,
                proj.blend_factor() as f32,
                proj.scale_factor() as f32,
                proj.fov_y.radians() as f32,
            ],
            sun_dir_intensity: [sun_dir.x, sun_dir.y, sun_dir.z, sun_intensity],
            sun_color_night_w: [sun_color.x, sun_color.y, sun_color.z, 1.0 - day_t],
            twilight_dir_weight: [twilight_dir.x, twilight_dir.y, twilight_dir.z, twilight_weight],
            sky_ambient: [sky_ambient.x, sky_ambient.y, sky_ambient.z, 0.0],
            gnd_ambient: [gnd_ambient.x, gnd_ambient.y, gnd_ambient.z, 0.0],
            rim_color: [rim_color.x, rim_color.y, rim_color.z, 0.0],
        };
        self.queue.write_buffer(
            &self.camera_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        let depth_view = &self.depth_texture.as_ref().unwrap().1;

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Mount 3D Overlay Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // Bind camera uniform group once for all 4 pipelines (identical pipeline layout).
        pass.set_bind_group(0, &self.camera_bind_group, &[]);

        // 1. Translucent safety & meridian limit zones (at infinity, over Explore sky)
        if self.sky_translucent_mesh.index_count > 0 {
            if let (Some(vb), Some(ib)) = (
                &self.sky_translucent_mesh.vb,
                &self.sky_translucent_mesh.ib,
            ) {
                pass.set_pipeline(&self.infinity_mesh_pipeline);
                pass.set_vertex_buffer(0, vb.buffer.slice(..));
                pass.set_index_buffer(ib.buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.sky_translucent_mesh.index_count, 0, 0..1);
            }
        }

        // 2. Limit rims, meridian arc & optical-axis sky reticle (at infinity)
        if self.sky_lines.vertex_count > 0 {
            if let Some(vb) = &self.sky_lines.vb {
                pass.set_pipeline(&self.infinity_line_pipeline);
                pass.set_vertex_buffer(0, vb.buffer.slice(..));
                pass.draw(0..self.sky_lines.vertex_count, 0..1);
            }
        }

        // 3. Opaque foreground NEQ6-Pro mount & optical train
        if self.opaque_mesh.index_count > 0 {
            if let (Some(vb), Some(ib)) = (&self.opaque_mesh.vb, &self.opaque_mesh.ib) {
                pass.set_pipeline(&self.opaque_pipeline);
                pass.set_vertex_buffer(0, vb.buffer.slice(..));
                pass.set_index_buffer(ib.buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.opaque_mesh.index_count, 0, 0..1);
            }
        }

        // 4. Foreground world-space lines (ground compass & local OTA beam)
        if self.lines.vertex_count > 0 {
            if let Some(vb) = &self.lines.vb {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, vb.buffer.slice(..));
                pass.draw(0..self.lines.vertex_count, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_widens_fov_past_max_distance_and_reverses_cleanly() {
        let mut cam = OrbitCamera::default();
        assert!((cam.fov_y_deg - BASE_FOV_Y_DEG).abs() < 1e-4);

        for _ in 0..30 {
            cam.zoom(1.15);
        }
        assert!((cam.distance - MAX_ORBIT_DISTANCE).abs() < 1e-4);
        assert!((cam.fov_y_deg - MAX_FOV_Y_DEG).abs() < 1e-3);
        let (_, _, fov_rad) = cam.to_topocentric_angles();
        assert!((fov_rad.to_degrees() - f64::from(MAX_FOV_Y_DEG)).abs() < 1e-3);

        for _ in 0..40 {
            cam.zoom(0.85);
        }
        assert!((cam.fov_y_deg - BASE_FOV_Y_DEG).abs() < 1e-4);
        assert!((cam.distance - MIN_ORBIT_DISTANCE).abs() < 1e-4);

        // Non-finite / non-positive zoom factors are ignored
        let before = cam;
        cam.zoom(f32::NAN);
        cam.zoom(0.0);
        cam.zoom(-1.5);
        assert_eq!(cam, before);
    }
}
