use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use std::sync::Arc;
use tracing::info;

use crate::{
    raw_image::RawImage,
    types::{DisplayRange, ViewTransform},
};

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShaderUniforms {
    image_size: [u32; 2],
    demosaic_enabled: u32,
    _pad0: u32,
    cfa_pattern: [u32; 4],
    black_levels: [f32; 4],
    white_levels: [f32; 4],
    wb_coeffs: [f32; 4],
    screen_size: [f32; 2],
    range_min_max: [f32; 2],
    pan: [f32; 2],
    zoom: f32,
    _pad1: f32,
}

pub struct RawViewerRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    raw_texture: Option<(wgpu::Texture, u32, u32)>,
    target_texture: Option<(wgpu::Texture, Arc<wgpu::TextureView>, u32, u32)>,
    current_image: Option<Arc<RawImage>>,
    demosaic_enabled: bool,
    display_range: DisplayRange,
    view_transform: ViewTransform,
}

impl RawViewerRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader_code = include_str!("shader.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Raw Viewer Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_code.into()),
        });

        let uniform_size = std::mem::size_of::<ShaderUniforms>() as u64;
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Raw Viewer Uniform Buffer"),
            size: uniform_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Raw Viewer Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // dummy
        let dummy_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy Raw Texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Raw Viewer Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Raw Viewer Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Raw Viewer Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            device,
            queue,
            render_pipeline,
            bind_group_layout,
            uniform_buffer,
            bind_group,
            raw_texture: None,
            target_texture: None,
            current_image: None,
            demosaic_enabled: true,
            display_range: DisplayRange::DEFAULT,
            view_transform: ViewTransform::DEFAULT,
        }
    }

    pub fn set_image(&mut self, image: Arc<RawImage>) {
        let width = image.width;
        let height = image.height;

        let needs_realloc = match &self.raw_texture {
            Some((_, w, h)) => *w != width || *h != height,
            None => true,
        };

        if needs_realloc {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Raw Sensor Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Raw Sensor Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                ],
            });
            self.raw_texture = Some((texture, width, height));
        }

        if let Some((ref texture, _, _)) = self.raw_texture {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&image.raw_pixels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 2),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.current_image = Some(image);
        info!("Updated RawViewerRenderer with image {}x{}", width, height);
    }

    pub fn set_demosaic(&mut self, enabled: bool) {
        self.demosaic_enabled = enabled;
    }

    pub fn demosaic(&self) -> bool {
        self.demosaic_enabled
    }

    pub fn set_display_range(&mut self, min: f32, max: f32) {
        self.display_range = DisplayRange::new(min, max);
    }

    pub fn set_range(&mut self, range: DisplayRange) {
        self.display_range = range;
    }

    pub fn display_range(&self) -> (f32, f32) {
        (self.display_range.min, self.display_range.max)
    }

    pub fn range(&self) -> DisplayRange {
        self.display_range
    }

    pub fn set_pan_zoom(&mut self, pan: Vec2, zoom: f32) {
        self.view_transform = ViewTransform::new(pan, zoom);
    }

    pub fn set_view_transform(&mut self, transform: ViewTransform) {
        self.view_transform = transform;
    }

    pub fn pan_zoom(&self) -> (Vec2, f32) {
        (self.view_transform.pan, self.view_transform.zoom)
    }

    pub fn view_transform(&self) -> ViewTransform {
        self.view_transform
    }

    pub fn render(&mut self, width: u32, height: u32) -> Arc<wgpu::TextureView> {
        let width = width.max(1);
        let height = height.max(1);

        let needs_target_realloc = match &self.target_texture {
            Some((_, _, w, h)) => *w != width || *h != height,
            None => true,
        };

        if needs_target_realloc {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Raw Viewer Render Target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.target_texture = Some((texture, view, width, height));
        }

        let target_view = self.target_texture.as_ref().unwrap().1.clone();

        let uniforms = if let Some(raw) = &self.current_image {
            ShaderUniforms {
                image_size: [raw.width, raw.height],
                demosaic_enabled: u32::from(self.demosaic_enabled),
                _pad0: 0,
                cfa_pattern: raw.cfa_pattern,
                black_levels: raw.black_levels,
                white_levels: raw.white_levels,
                wb_coeffs: raw.wb_coeffs,
                screen_size: [width as f32, height as f32],
                range_min_max: self.display_range.as_array(),
                pan: self.view_transform.pan.to_array(),
                zoom: self.view_transform.zoom,
                _pad1: 0.0,
            }
        } else {
            ShaderUniforms {
                image_size: [0, 0],
                demosaic_enabled: 0,
                _pad0: 0,
                cfa_pattern: [0, 1, 1, 2],
                black_levels: [0.0; 4],
                white_levels: [16383.0; 4],
                wb_coeffs: [1.0; 4],
                screen_size: [width as f32, height as f32],
                range_min_max: [0.0, 1.0],
                pan: [0.0, 0.0],
                zoom: 1.0,
                _pad1: 0.0,
            }
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Raw Viewer Encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Raw Viewer Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.04,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            if self.current_image.is_some() {
                rpass.set_pipeline(&self.render_pipeline);
                rpass.set_bind_group(0, &self.bind_group, &[]);
                rpass.draw(0..3, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        target_view
    }
}
