//! The HUD (COMPANIONS.md 6): text and bars in pixels, drawn over the frame in one call.
//! The font is drawn here, five by seven, capitals only: nothing to load and nothing to
//! license. Everything is a quad into one small atlas whose first cell is solid.

use wgpu::util::DeviceExt;

use crate::render::{DEPTH_FORMAT, Gpu};

/// A glyph cell in the atlas: five by seven dots with a dot of space right and below.
const CELL_W: u32 = 6;
const CELL_H: u32 = 8;
const ATLAS_COLS: u32 = 16;
/// Glyph size and advance in pixels at scale 1.
pub const GLYPH_W: f32 = 5.0;
pub const GLYPH_H: f32 = 7.0;
pub const ADVANCE: f32 = 6.0;

/// Rows top to bottom, the five low bits left to right.
const GLYPHS: &[(char, [u8; 7])] = &[
    (
        'A',
        [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'B',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'C',
        [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
    ),
    (
        'D',
        [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'E',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'F',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
    ),
    (
        'G',
        [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'H',
        [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'I',
        [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
    ),
    (
        'J',
        [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
    ),
    (
        'K',
        [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
    ),
    (
        'L',
        [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'M',
        [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'N',
        [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'O',
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'P',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
    ),
    (
        'Q',
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
    ),
    (
        'R',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
    ),
    (
        'S',
        [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
    ),
    (
        'T',
        [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
    ),
    (
        'U',
        [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'V',
        [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
    ),
    (
        'W',
        [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
    ),
    (
        'X',
        [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
    ),
    (
        'Y',
        [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
    ),
    (
        'Z',
        [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
    ),
    (
        '0',
        [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
    ),
    (
        '1',
        [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
    ),
    (
        '2',
        [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
    ),
    (
        '3',
        [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
    ),
    (
        '4',
        [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
    ),
    (
        '5',
        [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
    ),
    (
        '6',
        [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        '7',
        [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
    ),
    (
        '8',
        [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        '9',
        [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
    ),
    (
        '.',
        [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
    ),
    (
        ',',
        [
            0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000,
        ],
    ),
    (
        ':',
        [
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000,
        ],
    ),
    (
        ';',
        [
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b00100, 0b01000,
        ],
    ),
    (
        '!',
        [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
    ),
    (
        '?',
        [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
    ),
    (
        '\'',
        [
            0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
    ),
    (
        '"',
        [
            0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
    ),
    (
        '-',
        [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
    ),
    (
        '+',
        [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
    ),
    (
        '/',
        [
            0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000,
        ],
    ),
    (
        '(',
        [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
    ),
    (
        ')',
        [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
    ),
    (
        '%',
        [
            0b11001, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b10011,
        ],
    ),
    (
        '#',
        [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
    ),
    (
        '<',
        [
            0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010,
        ],
    ),
    (
        '>',
        [
            0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000,
        ],
    ),
    (
        '=',
        [
            0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000,
        ],
    ),
    (
        '*',
        [
            0b00000, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0b00000,
        ],
    ),
    (
        '[',
        [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
    ),
    (
        ']',
        [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
    ),
    (
        '_',
        [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
    ),
    (
        '`',
        [
            0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
    ),
];

/// The atlas cell of a character: capitals stand for small letters, `?` for the unknown.
/// Cell 0 is solid and a space draws nothing.
fn cell_of(c: char) -> Option<u32> {
    if c == ' ' {
        return None;
    }
    let c = c.to_ascii_uppercase();
    let find = |c: char| GLYPHS.iter().position(|g| g.0 == c).map(|i| i as u32 + 1);
    find(c).or_else(|| find('?'))
}

/// The atlas: one byte per texel, 255 where a dot is.
fn atlas() -> (Vec<u8>, u32, u32) {
    let cells = GLYPHS.len() as u32 + 1;
    let rows = cells.div_ceil(ATLAS_COLS);
    let (w, h) = (ATLAS_COLS * CELL_W, rows * CELL_H);
    let mut px = vec![0u8; (w * h) as usize];
    for y in 0..CELL_H {
        for x in 0..CELL_W {
            px[(y * w + x) as usize] = 255;
        }
    }
    for (i, (_, rows)) in GLYPHS.iter().enumerate() {
        let cell = i as u32 + 1;
        let (cx, cy) = ((cell % ATLAS_COLS) * CELL_W, (cell / ATLAS_COLS) * CELL_H);
        for (y, row) in rows.iter().enumerate() {
            for x in 0..5 {
                if row & (0b10000 >> x) != 0 {
                    px[((cy + y as u32) * w + cx + x) as usize] = 255;
                }
            }
        }
    }
    (px, w, h)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HudVertex {
    /// Pixels from the top left.
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

const SHADER: &str = r#"
struct Screen { size: vec4<f32> };
@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var font: texture_2d<f32>;
@group(0) @binding(2) var font_sampler: sampler;
struct VsIn { @location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };
@vertex fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos.x / screen.size.x * 2.0 - 1.0, 1.0 - in.pos.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dot = textureSample(font, font_sampler, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * dot);
}
"#;

pub const WHITE: [f32; 4] = [0.92, 0.92, 0.88, 1.0];
pub const DIM: [f32; 4] = [0.62, 0.62, 0.60, 1.0];
pub const SHADE: [f32; 4] = [0.0, 0.0, 0.0, 0.55];
pub const RED: [f32; 4] = [0.80, 0.16, 0.12, 1.0];
pub const GREEN: [f32; 4] = [0.25, 0.75, 0.30, 1.0];
pub const YELLOW: [f32; 4] = [0.90, 0.75, 0.20, 1.0];
pub const BLUE: [f32; 4] = [0.25, 0.50, 0.95, 1.0];
pub const ORANGE: [f32; 4] = [0.95, 0.55, 0.15, 1.0];

pub struct Hud {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    screen_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    capacity: usize,
    vertices: Vec<HudVertex>,
    atlas_size: (f32, f32),
    /// The frame's size in pixels: what the layout is made for.
    pub size: (f32, f32),
}

impl Hud {
    pub fn new(gpu: &Gpu, color_format: wgpu::TextureFormat) -> Hud {
        let device = &gpu.device;
        let (pixels, w, h) = atlas();
        let texture = device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("hud font"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud font"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let screen_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: screen_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // Over everything: the pass has a depth buffer, the HUD ignores it.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let capacity = 6 * 1024;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud vertices"),
            size: (capacity * std::mem::size_of::<HudVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Hud {
            pipeline,
            bind_group,
            screen_buf,
            vertex_buf,
            capacity,
            vertices: Vec::new(),
            atlas_size: (w as f32, h as f32),
            size: (1.0, 1.0),
        }
    }

    /// Start a frame's HUD for a target of `size` pixels.
    pub fn begin(&mut self, size: (u32, u32)) {
        self.vertices.clear();
        self.size = (size.0.max(1) as f32, size.1.max(1) as f32);
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    fn quad(&mut self, x: f32, y: f32, w: f32, h: f32, cell: u32, color: [f32; 4]) {
        // On whole pixels: a glyph that straddles them loses dots to nearest sampling.
        let (x, y, w, h) = (x.round(), y.round(), w.round(), h.round());
        let (aw, ah) = self.atlas_size;
        // Half a texel in from the cell's edges: nearest sampling never bleeds a neighbour.
        let u0 = ((cell % ATLAS_COLS) * CELL_W) as f32;
        let v0 = ((cell / ATLAS_COLS) * CELL_H) as f32;
        let (u1, v1) = (u0 + GLYPH_W, v0 + GLYPH_H);
        let (u0, v0, u1, v1) = (u0 / aw, v0 / ah, u1 / aw, v1 / ah);
        let v = |px: f32, py: f32, u: f32, v: f32| HudVertex {
            pos: [px, py],
            uv: [u, v],
            color,
        };
        self.vertices.extend_from_slice(&[
            v(x, y, u0, v0),
            v(x + w, y, u1, v0),
            v(x + w, y + h, u1, v1),
            v(x, y, u0, v0),
            v(x + w, y + h, u1, v1),
            v(x, y + h, u0, v1),
        ]);
    }

    /// A filled rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        if w > 0.0 && h > 0.0 {
            self.quad(x, y, w, h, 0, color);
        }
    }

    pub fn text_width(scale: f32, text: &str) -> f32 {
        let n = text.chars().count() as f32;
        (n * ADVANCE - (ADVANCE - GLYPH_W)).max(0.0) * scale
    }

    /// Text with its top left at `(x, y)`; returns where it ends.
    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) -> f32 {
        let mut at = x.round();
        for c in text.chars() {
            if let Some(cell) = cell_of(c) {
                self.quad(at, y, GLYPH_W * scale, GLYPH_H * scale, cell, color);
            }
            at += ADVANCE * scale;
        }
        at
    }

    /// Text with a dark plate behind it: readable over any wall.
    pub fn label(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) -> f32 {
        let w = Hud::text_width(scale, text);
        self.rect(
            x - 2.0 * scale,
            y - 2.0 * scale,
            w + 4.0 * scale,
            (GLYPH_H + 4.0) * scale,
            SHADE,
        );
        self.text(x, y, scale, color, text)
    }

    /// A bar filled to `frac` of its width.
    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, frac: f32, fill: [f32; 4]) {
        self.rect(x - 1.0, y - 1.0, w + 2.0, h + 2.0, SHADE);
        self.rect(x, y, w * frac.clamp(0.0, 1.0), h, fill);
    }

    /// Upload the frame's quads.
    pub fn prepare(&mut self, gpu: &Gpu) {
        gpu.queue.write_buffer(
            &self.screen_buf,
            0,
            bytemuck::cast_slice(&[self.size.0, self.size.1, 0.0, 0.0]),
        );
        if self.vertices.len() > self.capacity {
            self.capacity = self.vertices.len().next_power_of_two();
            self.vertex_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hud vertices"),
                size: (self.capacity * std::mem::size_of::<HudVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.vertices.is_empty() {
            gpu.queue
                .write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&self.vertices));
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.vertices.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(0..self.vertices.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_five_dots_wide_and_the_table_has_no_duplicates() {
        for (i, (c, rows)) in GLYPHS.iter().enumerate() {
            assert!(rows.iter().all(|r| *r < 32), "{c}");
            assert!(rows.iter().any(|r| *r != 0), "{c} is blank");
            assert!(
                GLYPHS[..i].iter().all(|g| g.0 != *c),
                "{c} is in the table twice"
            );
        }
        // Small letters share the capitals; an unknown character is a question mark.
        assert_eq!(cell_of('a'), cell_of('A'));
        assert_eq!(cell_of('~'), cell_of('?'));
        assert_eq!(cell_of(' '), None);
    }

    #[test]
    fn the_atlas_has_a_solid_cell_and_the_glyphs_do_not_touch() {
        let (px, w, h) = atlas();
        assert_eq!(px.len(), (w * h) as usize);
        // Cell 0 is solid.
        assert!((0..CELL_H).all(|y| (0..CELL_W).all(|x| px[(y * w + x) as usize] == 255)));
        // Every other cell keeps its sixth column and eighth row empty.
        for cell in 1..=GLYPHS.len() as u32 {
            let (cx, cy) = ((cell % ATLAS_COLS) * CELL_W, (cell / ATLAS_COLS) * CELL_H);
            for y in 0..CELL_H {
                assert_eq!(px[((cy + y) * w + cx + 5) as usize], 0);
            }
            for x in 0..CELL_W {
                assert_eq!(px[((cy + 7) * w + cx + x) as usize], 0);
            }
        }
        assert_eq!(Hud::text_width(2.0, "AB"), 22.0);
        assert_eq!(Hud::text_width(1.0, ""), 0.0);
    }
}
