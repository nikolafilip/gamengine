//! wgpu device setup and the world renderer: one pipeline, one draw call, PVS-culled indices.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::Error;
use crate::world::{FaceRange, TEXTURE_SIZE, Vertex, WorldMesh};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const FOV_Y_DEG: f32 = 75.0;
const NEAR: f32 = 4.0;
const FAR: f32 = 8192.0;

pub struct Gpu {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

impl Gpu {
    pub fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        software: bool,
    ) -> Result<Gpu, Error> {
        pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    force_fallback_adapter: software,
                    compatible_surface: surface,
                    ..Default::default()
                })
                .await
                .map_err(|e| {
                    format!("no compatible GPU adapter (is a Vulkan driver installed?): {e}")
                })?;
            let info = adapter.get_info();
            log::info!(
                "adapter: {} ({:?}, {:?}, driver {} {})",
                info.name,
                info.backend,
                info.device_type,
                info.driver,
                info.driver_info
            );
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("gm-client"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                })
                .await
                .map_err(|e| format!("device creation failed: {e}"))?;
            Ok(Gpu {
                adapter,
                device,
                queue,
                info,
            })
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    /// x = lightmap scale; the rest is reserved.
    params: [f32; 4],
}

/// View-projection for a Quake-convention camera (Z up, yaw counter-clockwise from +X, positive
/// pitch looks down), producing wgpu clip space.
pub fn view_proj(eye: Vec3, yaw_deg: f32, pitch_deg: f32, aspect: f32) -> Mat4 {
    let (sy, cy) = yaw_deg.to_radians().sin_cos();
    let (sp, cp) = pitch_deg.to_radians().sin_cos();
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let proj =
        glam::camera::rh::proj::directx::perspective(FOV_Y_DEG.to_radians(), aspect, NEAR, FAR);
    proj * glam::camera::rh::view::look_to_mat4(eye, forward, Vec3::Z)
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    textures_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
    face_ranges: Vec<FaceRange>,
    all_indices: Vec<u32>,
    scratch: Vec<u32>,
    pub lightmap_scale: f32,
    pub faces_drawn: usize,
}

fn make_depth(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// Box-filter mip chain for an RGBA image.
fn mip_chain(rgba: &[u8], mut w: u32, mut h: u32) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(w, h, rgba.to_vec())];
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let src = &levels.last().unwrap().2;
        let mut dst = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut acc = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(w - 1);
                        let sy = (y * 2 + dy).min(h - 1);
                        acc += src[((sy * w + sx) * 4 + c) as usize] as u32;
                    }
                    dst[((y * nw + x) * 4 + c) as usize] = (acc / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, dst));
        w = nw;
        h = nh;
    }
    levels
}

fn upload_layer(
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    layer: u32,
    mips: &[(u32, u32, Vec<u8>)],
) {
    for (level, (w, h, data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
}

impl Renderer {
    pub fn new(
        gpu: &Gpu,
        color_format: wgpu::TextureFormat,
        world: &WorldMesh,
        size: (u32, u32),
    ) -> Renderer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        // Diffuse texture array with a full mip chain.
        let layers = world.texture_layers.len().max(1) as u32;
        let mip_count = TEXTURE_SIZE.ilog2() + 1;
        let diffuse = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("diffuse array"),
            size: wgpu::Extent3d {
                width: TEXTURE_SIZE,
                height: TEXTURE_SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, layer) in world.texture_layers.iter().enumerate() {
            upload_layer(
                &gpu.queue,
                &diffuse,
                i as u32,
                &mip_chain(layer, TEXTURE_SIZE, TEXTURE_SIZE),
            );
        }
        let diffuse_view = diffuse.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let diffuse_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("diffuse"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        // Lightmap atlas, linear (not sRGB): light values multiply the albedo.
        let lm = &world.lightmap;
        let lightmap = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lightmap atlas"),
            size: wgpu::Extent3d {
                width: lm.width,
                height: lm.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        upload_layer(
            &gpu.queue,
            &lightmap,
            0,
            &[(lm.width, lm.height, lm.rgba.clone())],
        );
        let lightmap_view = lightmap.create_view(&wgpu::TextureViewDescriptor::default());
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
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
        let tex_entry = |binding, dim| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[
                tex_entry(0, wgpu::TextureViewDimension::D2Array),
                sampler_entry(1),
                tex_entry(2, wgpu::TextureViewDimension::D2),
                sampler_entry(3),
            ],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });
        let textures_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("textures"),
            layout: &textures_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&diffuse_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&diffuse_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&lightmap_sampler),
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&globals_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Uint32],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("world"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world vertices"),
            contents: bytemuck::cast_slice(&world.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world indices"),
            contents: bytemuck::cast_slice(&world.indices),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });

        Renderer {
            pipeline,
            globals_buf,
            globals_bg,
            textures_bg,
            vertex_buf,
            index_buf,
            index_count: world.indices.len() as u32,
            depth_view: make_depth(device, size),
            depth_size: size,
            face_ranges: world.face_ranges.clone(),
            all_indices: world.indices.clone(),
            scratch: Vec::with_capacity(world.indices.len()),
            lightmap_scale: 2.0,
            faces_drawn: world
                .face_ranges
                .iter()
                .filter(|r| r.index_count > 0)
                .count(),
        }
    }

    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if size != self.depth_size && size.0 > 0 && size.1 > 0 {
            self.depth_view = make_depth(&gpu.device, size);
            self.depth_size = size;
        }
    }

    /// Restrict drawing to `faces` (from the PVS), or to everything with `None`.
    pub fn set_visible_faces(&mut self, gpu: &Gpu, faces: Option<&[u32]>) {
        self.scratch.clear();
        let mut drawn = 0;
        match faces {
            None => {
                self.scratch.extend_from_slice(&self.all_indices);
                drawn = self
                    .face_ranges
                    .iter()
                    .filter(|r| r.index_count > 0)
                    .count();
            }
            Some(faces) => {
                for &f in faces {
                    let r = self.face_ranges[f as usize];
                    if r.index_count > 0 {
                        drawn += 1;
                        let start = r.first_index as usize;
                        self.scratch.extend_from_slice(
                            &self.all_indices[start..start + r.index_count as usize],
                        );
                    }
                }
            }
        }
        self.index_count = self.scratch.len() as u32;
        self.faces_drawn = drawn;
        if !self.scratch.is_empty() {
            gpu.queue
                .write_buffer(&self.index_buf, 0, bytemuck::cast_slice(&self.scratch));
        }
    }

    /// Record and submit one frame into `target`.
    pub fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, view_proj: Mat4) {
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            params: [self.lightmap_scale, 0.0, 0.0, 0.0],
        };
        gpu.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.03,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.index_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.globals_bg, &[]);
                pass.set_bind_group(1, &self.textures_bg, &[]);
                pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
                pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}
