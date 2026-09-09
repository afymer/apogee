use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use std::sync::Arc;
use tracing::info;

use crate::{
    histogram::HistogramData,
    raw_image::RawImage,
    types::{DisplayRange, ViewTransform},
};

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

const VIEWER_SHADER_CODE: &str = concat!(
    include_str!("shader/common.wgsl"),
    include_str!("shader/viewer.wgsl")
);

const HISTOGRAM_SHADER_CODE: &str = concat!(
    include_str!("shader/common.wgsl"),
    include_str!("shader/histogram.wgsl")
);

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ImageUniforms {
    pub image_size: [u32; 2],
    pub demosaic_enabled: u32,
    pub pad0: u32,
    pub cfa_pattern: [u32; 4],
    pub black_levels: [f32; 4],
    pub white_levels: [f32; 4],
    pub wb_coeffs: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ViewUniforms {
    pub screen_size: [f32; 2],
    pub pan: [f32; 2],
    pub zoom: f32,
    _pad0: f32,
    _pad1: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct StretchUniforms {
    pub range_min_max: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct HistogramParams {
    pub x_log: u32,
    pub y_log: u32,
    pub pad0: u32,
    pub pad1: u32,
}

pub struct RawViewerRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    render_pipeline: wgpu::RenderPipeline,
    image_bind_group_layout: wgpu::BindGroupLayout,
    image_uniform_buffer: wgpu::Buffer,
    view_uniform_buffer: wgpu::Buffer,
    stretch_uniform_buffer: wgpu::Buffer,
    image_bind_group: wgpu::BindGroup,
    view_bind_group: wgpu::BindGroup,
    raw_texture: Option<(wgpu::Texture, u32, u32)>,
    target_texture: Option<(wgpu::Texture, Arc<wgpu::TextureView>, u32, u32)>,
    current_image: Option<Arc<RawImage>>,
    demosaic_enabled: bool,
    display_range: DisplayRange,
    view_transform: ViewTransform,
    image_uniforms_dirty: bool,
    stretch_uniforms_dirty: bool,
    histogram_accum_pipeline: wgpu::ComputePipeline,
    histogram_postprocess_pipeline: wgpu::ComputePipeline,
    histogram_bind_group_layout: wgpu::BindGroupLayout,
    histogram_params_buffer: wgpu::Buffer,
    histogram_accum_buffer: wgpu::Buffer,
    histogram_output_buffer: wgpu::Buffer,
    histogram_staging_buffer: wgpu::Buffer,
    histogram_bind_group: Option<wgpu::BindGroup>,
}

impl RawViewerRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Raw Viewer Shader"),
            source: wgpu::ShaderSource::Wgsl(VIEWER_SHADER_CODE.into()),
        });

        let image_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Raw Viewer Image Uniform Buffer"),
            size: std::mem::size_of::<ImageUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let view_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Raw Viewer View Uniform Buffer"),
            size: std::mem::size_of::<ViewUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let stretch_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Raw Viewer Stretch Uniform Buffer"),
            size: std::mem::size_of::<StretchUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let default_image_uniforms = ImageUniforms {
            image_size: [0, 0],
            demosaic_enabled: 0,
            pad0: 0,
            cfa_pattern: [0, 1, 1, 2],
            black_levels: [0.0; 4],
            white_levels: [16383.0; 4],
            wb_coeffs: [1.0; 4],
        };
        queue.write_buffer(
            &image_uniform_buffer,
            0,
            bytemuck::cast_slice(&[default_image_uniforms]),
        );

        let default_view_uniforms = ViewUniforms {
            screen_size: [1.0, 1.0],
            pan: [0.0, 0.0],
            zoom: 1.0,
            _pad0: 0.0,
            _pad1: [0.0, 0.0],
        };
        queue.write_buffer(
            &view_uniform_buffer,
            0,
            bytemuck::cast_slice(&[default_view_uniforms]),
        );

        let default_stretch_uniforms = StretchUniforms {
            range_min_max: DisplayRange::DEFAULT.as_array(),
            _pad: [0.0, 0.0],
        };
        queue.write_buffer(
            &stretch_uniform_buffer,
            0,
            bytemuck::cast_slice(&[default_stretch_uniforms]),
        );

        let image_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Raw Viewer Image Bind Group Layout"),
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
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let view_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Raw Viewer View Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
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

        let image_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Raw Viewer Image Bind Group"),
            layout: &image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: image_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Raw Viewer View Bind Group"),
            layout: &view_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: stretch_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Raw Viewer Pipeline Layout"),
            bind_group_layouts: &[
                Some(&image_bind_group_layout),
                Some(&view_bind_group_layout),
            ],
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

        let histogram_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Histogram Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(HISTOGRAM_SHADER_CODE.into()),
        });

        let histogram_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Params Buffer"),
            size: std::mem::size_of::<HistogramParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let histogram_accum_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Accum Buffer"),
            size: (1024 * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let histogram_output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Output Buffer"),
            size: (1024 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let histogram_staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Histogram Staging Buffer"),
            size: (1024 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let histogram_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Histogram Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Uint,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let histogram_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Histogram Pipeline Layout"),
                bind_group_layouts: &[Some(&histogram_bind_group_layout)],
                immediate_size: 0,
            });

        let histogram_accum_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Histogram Accum Compute Pipeline"),
                layout: Some(&histogram_pipeline_layout),
                module: &histogram_shader_module,
                entry_point: Some("cs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        let histogram_postprocess_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Histogram Postprocess Compute Pipeline"),
                layout: Some(&histogram_pipeline_layout),
                module: &histogram_shader_module,
                entry_point: Some("cs_postprocess"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        Self {
            device,
            queue,
            render_pipeline,
            image_bind_group_layout,
            image_uniform_buffer,
            view_uniform_buffer,
            stretch_uniform_buffer,
            image_bind_group,
            view_bind_group,
            raw_texture: None,
            target_texture: None,
            current_image: None,
            demosaic_enabled: true,
            display_range: DisplayRange::DEFAULT,
            view_transform: ViewTransform::DEFAULT,
            image_uniforms_dirty: false,
            stretch_uniforms_dirty: false,
            histogram_accum_pipeline,
            histogram_postprocess_pipeline,
            histogram_bind_group_layout,
            histogram_params_buffer,
            histogram_accum_buffer,
            histogram_output_buffer,
            histogram_staging_buffer,
            histogram_bind_group: None,
        }
    }

    pub fn set_image(
        &mut self,
        image: Arc<RawImage>,
        x_log: bool,
        y_log: bool,
    ) -> Option<HistogramData> {
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
            self.image_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Raw Sensor Bind Group"),
                layout: &self.image_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.image_uniform_buffer.as_entire_binding(),
                    },
                ],
            });

            self.histogram_bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Histogram Bind Group"),
                layout: &self.histogram_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.image_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.histogram_params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.histogram_accum_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.histogram_output_buffer.as_entire_binding(),
                    },
                ],
            }));

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
        self.image_uniforms_dirty = true;
        let hist = self.compute_histogram(x_log, y_log);
        info!("Updated RawViewerRenderer with image {}x{}", width, height);
        hist
    }

    fn flush_image_uniforms(&mut self) {
        if !self.image_uniforms_dirty {
            return;
        }
        let image_uniforms = if let Some(raw) = &self.current_image {
            ImageUniforms {
                image_size: [raw.width, raw.height],
                demosaic_enabled: u32::from(self.demosaic_enabled),
                pad0: 0,
                cfa_pattern: raw.cfa_pattern,
                black_levels: raw.black_levels,
                white_levels: raw.white_levels,
                wb_coeffs: raw.wb_coeffs,
            }
        } else {
            ImageUniforms {
                image_size: [0, 0],
                demosaic_enabled: 0,
                pad0: 0,
                cfa_pattern: [0, 1, 1, 2],
                black_levels: [0.0; 4],
                white_levels: [16383.0; 4],
                wb_coeffs: [1.0; 4],
            }
        };
        self.queue.write_buffer(
            &self.image_uniform_buffer,
            0,
            bytemuck::cast_slice(&[image_uniforms]),
        );
        self.image_uniforms_dirty = false;
    }

    pub fn compute_histogram(&mut self, x_log: bool, y_log: bool) -> Option<HistogramData> {
        self.flush_image_uniforms();

        let image = self.current_image.as_ref()?;
        let bind_group = self.histogram_bind_group.as_ref()?;

        let params = HistogramParams {
            x_log: u32::from(x_log),
            y_log: u32::from(y_log),
            pad0: 0,
            pad1: 0,
        };
        self.queue.write_buffer(
            &self.histogram_params_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        let zeros = [0u32; 1024];
        self.queue.write_buffer(
            &self.histogram_accum_buffer,
            0,
            bytemuck::cast_slice(&zeros),
        );

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Histogram Compute Encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Histogram Accumulate Compute Pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.histogram_accum_pipeline);
            cpass.set_bind_group(0, bind_group, &[]);
            let dispatch_x = image.width.div_ceil(32);
            let dispatch_y = image.height.div_ceil(32);
            cpass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Histogram Postprocess Compute Pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.histogram_postprocess_pipeline);
            cpass.set_bind_group(0, bind_group, &[]);
            cpass.dispatch_workgroups(4, 1, 1);
        }

        encoder.copy_buffer_to_buffer(
            &self.histogram_output_buffer,
            0,
            &self.histogram_staging_buffer,
            0,
            (1024 * std::mem::size_of::<f32>()) as u64,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = self.histogram_staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(Ok(())) = rx.recv() else {
            return None;
        };

        let mapped_range = slice.get_mapped_range();
        let bins_slice: &[f32] = bytemuck::cast_slice(&mapped_range);
        let mut raw_bins = [0.0f32; 1024];
        raw_bins.copy_from_slice(bins_slice);
        drop(mapped_range);
        self.histogram_staging_buffer.unmap();

        Some(HistogramData::from_raw_bins(&raw_bins))
    }

    pub fn set_demosaic(&mut self, enabled: bool) {
        if self.demosaic_enabled != enabled {
            self.demosaic_enabled = enabled;
            self.image_uniforms_dirty = true;
        }
    }

    pub fn demosaic(&self) -> bool {
        self.demosaic_enabled
    }

    pub fn set_display_range(&mut self, min: f32, max: f32) {
        self.set_range(DisplayRange::new(min, max));
    }

    pub fn set_range(&mut self, range: DisplayRange) {
        if self.display_range != range {
            self.display_range = range;
            self.stretch_uniforms_dirty = true;
        }
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

        self.flush_image_uniforms();

        if self.stretch_uniforms_dirty {
            let stretch_uniforms = StretchUniforms {
                range_min_max: self.display_range.as_array(),
                _pad: [0.0, 0.0],
            };
            self.queue.write_buffer(
                &self.stretch_uniform_buffer,
                0,
                bytemuck::cast_slice(&[stretch_uniforms]),
            );
            self.stretch_uniforms_dirty = false;
        }

        let view_uniforms = ViewUniforms {
            screen_size: [width as f32, height as f32],
            pan: self.view_transform.pan.to_array(),
            zoom: self.view_transform.zoom,
            _pad0: 0.0,
            _pad1: [0.0, 0.0],
        };
        self.queue.write_buffer(
            &self.view_uniform_buffer,
            0,
            bytemuck::cast_slice(&[view_uniforms]),
        );

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
                rpass.set_bind_group(0, &self.image_bind_group, &[]);
                rpass.set_bind_group(1, &self.view_bind_group, &[]);
                rpass.draw(0..3, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        target_view
    }
}
