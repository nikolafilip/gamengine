//! The HUD (COMPANIONS.md 6, LOOK.md 2): text, bars and pictures in pixels, drawn over the
//! frame as quads into one RGBA atlas, one draw per layer. The atlas is the bundle's
//! (`ui.gma`: the skin, the icons, three faces) once it has loaded, and until then one made
//! at start from the five-by-seven font alone, so that everything draws either way.
//!
//! The bundle has an atlas per density (texels a dot, LOOK.md 2.2) and the client draws
//! with the one of its UI scale: a texel is then a pixel. Sizes are asked for in pixels
//! and metrics are in dots, so drawing with an atlas of another density (the dense one
//! not yet here, a window too small for the scale chosen) is the same layout, coarser.

use glam::Vec2;
use gm_model::atlas::{Atlas, Cell, Face, Glyph};
use wgpu::util::DeviceExt;

pub use crate::font::{ADVANCE, GLYPH_H, GLYPH_W};
use crate::font::{ATLAS_COLS, CELL_H, CELL_W, GLYPH_ROWS, GLYPHS, atlas as small_atlas};
use crate::render::{DEPTH_FORMAT, Gpu};

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
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
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
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    return vec4<f32>(in.color.rgb * texel.rgb, in.color.a * texel.a);
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
/// A picture as painted.
pub const PLAIN: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The faces (LOOK.md 2.3): the small five-by-seven, the text face, the title face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FaceId {
    #[default]
    Small = 0,
    Text = 1,
    Title = 2,
}

/// The layers a frame's quads go to (LOOK.md 2.1), drawn in this order.
pub const LAYERS: usize = 5;

pub struct Hud {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    screen_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    capacity: usize,
    /// Quads by layer.
    layers: [Vec<HudVertex>; LAYERS],
    layer: usize,
    /// Where each layer's quads start and end in the uploaded buffer.
    ranges: [(u32, u32); LAYERS],
    /// What the texture holds: pieces, faces, icons, and its size.
    pub atlas: Atlas,
    /// The frame's size in pixels: what the layout is made for.
    pub size: (f32, f32),
    /// The bundle's atlas is the texture (not the built-in one).
    pub skinned: bool,
}

/// The atlas made of the five-by-seven font alone: face 0, no pieces, no icons; the old
/// cell grid, white with the dot as alpha. The first cell is solid, as before.
pub fn fallback_atlas() -> Atlas {
    let (px, w, h) = small_atlas();
    let mut texels = Vec::with_capacity(px.len() * 4);
    for a in &px {
        texels.extend_from_slice(&[255, 255, 255, *a]);
    }
    let advance = (ADVANCE * gm_model::atlas::ADVANCE_PARTS) as u8;
    let mut face = Face {
        line_height: GLYPH_H as u8 + 2,
        ascent: GLYPH_H as u8,
        glyphs: std::collections::HashMap::new(),
    };
    for (i, (c, _)) in GLYPHS.iter().enumerate() {
        let cell = (i + 1) as u32;
        face.glyphs.insert(
            *c,
            Glyph {
                cell: Cell {
                    x: ((cell % ATLAS_COLS) * CELL_W) as u16,
                    y: ((cell / ATLAS_COLS) * CELL_H) as u16,
                    w: GLYPH_W as u16,
                    h: GLYPH_ROWS as u16,
                },
                bearing: [0, 0],
                advance,
            },
        );
    }
    face.glyphs.insert(
        ' ',
        Glyph {
            cell: Cell::default(),
            bearing: [0, 0],
            advance,
        },
    );
    let mut a = Atlas {
        density: 1,
        w: w as u16,
        h: h as u16,
        texels,
        ..Default::default()
    };
    a.faces.insert(0, face);
    a
}

/// The solid texel every rectangle is drawn with: the first cell of the small font's grid
/// in the built-in atlas; in a bundle's atlas, the middle of the `well` piece's... no: a
/// bundle's atlas has no solid cell of its own, so one texel of it is made solid when it
/// is uploaded (`set_atlas`): the last texel, which no picture reaches (the packer leaves a
/// dot of space).
fn solid_uv(atlas: &Atlas, skinned: bool) -> [f32; 2] {
    if skinned {
        let (w, h) = (atlas.w as f32, atlas.h as f32);
        [(w - 0.5) / w, (h - 0.5) / h]
    } else {
        [2.5 / atlas.w as f32, 2.5 / atlas.h as f32]
    }
}

impl Hud {
    pub fn new(gpu: &Gpu, color_format: wgpu::TextureFormat) -> Hud {
        let device = &gpu.device;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud atlas"),
            // A texel is a pixel at the atlas's own scale, whichever filter; stretched
            // (a nine-slice's middle, a thinner atlas standing in) it stays sharp, and
            // drawn smaller (small print, a panel that had to give way) it is averaged.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
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
        let atlas = fallback_atlas();
        let bind_group = Self::upload(gpu, &layout, &sampler, &screen_buf, &atlas, false);
        Hud {
            pipeline,
            layout,
            sampler,
            bind_group,
            screen_buf,
            vertex_buf,
            capacity,
            layers: Default::default(),
            layer: 0,
            ranges: [(0, 0); LAYERS],
            atlas,
            size: (1.0, 1.0),
            skinned: false,
        }
    }

    fn upload(
        gpu: &Gpu,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        screen_buf: &wgpu::Buffer,
        atlas: &Atlas,
        skinned: bool,
    ) -> wgpu::BindGroup {
        let mut texels = atlas.texels.clone();
        if skinned {
            // The solid texel (see `solid_uv`).
            let n = texels.len();
            texels[n - 4..].copy_from_slice(&[255, 255, 255, 255]);
        }
        let texture = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("hud atlas"),
                size: wgpu::Extent3d {
                    width: atlas.w as u32,
                    height: atlas.h as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &texels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud"),
            layout,
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
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Texels per dot of the atlas in use.
    pub fn density(&self) -> u8 {
        self.atlas.density.max(1)
    }

    /// The bundle's atlas replaces the one in use (LOOK.md 2.2): the built-in one at
    /// start, a thinner or denser one when the scale changes.
    pub fn set_atlas(&mut self, gpu: &Gpu, atlas: Atlas) {
        if !atlas.faces.contains_key(&0) {
            log::warn!("the bundle's atlas has no face 0; the built-in one stays");
            return;
        }
        self.bind_group = Self::upload(
            gpu,
            &self.layout,
            &self.sampler,
            &self.screen_buf,
            &atlas,
            true,
        );
        self.atlas = atlas;
        self.skinned = true;
    }

    /// Start a frame's HUD for a target of `size` pixels.
    pub fn begin(&mut self, size: (u32, u32)) {
        for l in &mut self.layers {
            l.clear();
        }
        self.layer = 0;
        self.size = (size.0.max(1) as f32, size.1.max(1) as f32);
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(Vec::is_empty)
    }

    /// Everything after this goes to layer `n` (LOOK.md 2.1).
    pub fn set_layer(&mut self, n: usize) {
        self.layer = n.min(LAYERS - 1);
    }

    /// A quad at `at` (x, y, width, height) showing the texels `cell` of the atlas.
    fn quad_cell(&mut self, at: [f32; 4], cell: Cell, color: [f32; 4]) {
        // On whole pixels: a glyph that straddles them loses dots to nearest sampling.
        let [x, y, w, h] = at.map(f32::round);
        let (aw, ah) = (self.atlas.w as f32, self.atlas.h as f32);
        let (u0, v0) = (cell.x as f32 / aw, cell.y as f32 / ah);
        let (u1, v1) = ((cell.x + cell.w) as f32 / aw, (cell.y + cell.h) as f32 / ah);
        self.quad_uv([x, y, w, h], [u0, v0, u1, v1], color);
    }

    fn quad_uv(&mut self, [x, y, w, h]: [f32; 4], [u0, v0, u1, v1]: [f32; 4], color: [f32; 4]) {
        let v = |px: f32, py: f32, u: f32, v: f32| HudVertex {
            pos: [px, py],
            uv: [u, v],
            color,
        };
        self.layers[self.layer].extend_from_slice(&[
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
            let [u, v] = solid_uv(&self.atlas, self.skinned);
            let [x, y, w, h] = [x, y, w, h].map(f32::round);
            self.quad_uv([x, y, w, h], [u, v, u, v], color);
        }
    }

    /// A filled triangle (the cooldown wedge is a fan of them).
    pub fn triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4]) {
        let [u, v] = solid_uv(&self.atlas, self.skinned);
        let vx = |p: Vec2| HudVertex {
            pos: [p.x, p.y],
            uv: [u, v],
            color,
        };
        self.layers[self.layer].extend_from_slice(&[vx(a), vx(b), vx(c)]);
    }

    /// A sector of a disc, `from` to `to` in turns clockwise from twelve o'clock, as a fan
    /// of a triangle per sixteenth of a turn (LOOK.md 2.1).
    pub fn wedge(&mut self, centre: Vec2, radius: f32, from: f32, to: f32, color: [f32; 4]) {
        let (from, to) = (from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
        if to <= from {
            return;
        }
        let steps = ((to - from) * 16.0).ceil().max(1.0) as usize;
        let at = |t: f32| {
            let a = t * std::f32::consts::TAU;
            centre + Vec2::new(a.sin(), -a.cos()) * radius
        };
        for i in 0..steps {
            let t0 = from + (to - from) * i as f32 / steps as f32;
            let t1 = from + (to - from) * (i + 1) as f32 / steps as f32;
            self.triangle(centre, at(t0), at(t1), color);
        }
    }

    /// A piece of the skin stretched to `(x, y, w, h)`; nothing when the atlas lacks it.
    /// Returns whether it was drawn.
    pub fn image(&mut self, x: f32, y: f32, w: f32, h: f32, piece: &str, color: [f32; 4]) -> bool {
        match self.atlas.piece(piece) {
            Some(p) => {
                let cell = p.cell;
                self.quad_cell([x, y, w, h], cell, color);
                true
            }
            None => false,
        }
    }

    /// An icon by key, `side` pixels square; nothing when the atlas lacks it.
    pub fn icon(&mut self, x: f32, y: f32, side: f32, key: &str, color: [f32; 4]) -> bool {
        match self.atlas.icon(key) {
            Some(cell) => {
                self.quad_cell([x, y, side, side], cell, color);
                true
            }
            None => false,
        }
    }

    /// A nine-slice: the piece's corners at `scale`, its edges stretched one way, its
    /// middle both ways (LOOK.md 2.1). Returns whether it was drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        piece: &str,
        scale: f32,
        color: [f32; 4],
    ) -> bool {
        let Some(p) = self.atlas.piece(piece).copied() else {
            return false;
        };
        let c = p.cell;
        let [l, t, r, b] = p.inset.map(|v| v as f32);
        if l + r == 0.0 && t + b == 0.0 {
            self.quad_cell([x, y, w, h], c, color);
            return true;
        }
        // The insets are texels; `scale` is pixels a dot.
        let k = scale / self.atlas.dots();
        let (sl, st, sr, sb) = (l * k, t * k, r * k, b * k);
        // Three columns and three rows of texels, and of pixels.
        let cols = [
            (c.x as f32, l, x, sl),
            (c.x as f32 + l, c.w as f32 - l - r, x + sl, w - sl - sr),
            (c.x as f32 + c.w as f32 - r, r, x + w - sr, sr),
        ];
        let rows = [
            (c.y as f32, t, y, st),
            (c.y as f32 + t, c.h as f32 - t - b, y + st, h - st - sb),
            (c.y as f32 + c.h as f32 - b, b, y + h - sb, sb),
        ];
        let (aw, ah) = (self.atlas.w as f32, self.atlas.h as f32);
        for (ty, th, py, ph) in rows {
            for (tx, tw, px, pw) in cols {
                if tw <= 0.0 || th <= 0.0 || pw <= 0.0 || ph <= 0.0 {
                    continue;
                }
                self.quad_uv(
                    [px.round(), py.round(), pw.round(), ph.round()],
                    [tx / aw, ty / ah, (tx + tw) / aw, (ty + th) / ah],
                    color,
                );
            }
        }
        true
    }

    fn face(&self, face: FaceId) -> &Face {
        self.atlas
            .faces
            .get(&(face as u8))
            .or_else(|| self.atlas.faces.get(&0))
            .expect("face 0 is always there")
    }

    pub fn has_face(&self, face: FaceId) -> bool {
        self.atlas.faces.contains_key(&(face as u8))
    }

    /// A face's line height and ascent in dots (at scale 1).
    pub fn metrics(&self, face: FaceId) -> (f32, f32) {
        let f = self.face(face);
        (f.line_height as f32, f.ascent as f32)
    }

    /// The width of `text` in `face` at `scale`: the same at every density.
    pub fn width_in(&self, face: FaceId, scale: f32, text: &str) -> f32 {
        let f = self.face(face);
        let unknown = f.glyphs.get(&'?');
        let parts: u32 = text
            .chars()
            .map(|c| f.glyphs.get(&c).or(unknown).map_or(0, |g| g.advance as u32))
            .sum();
        parts as f32 * scale / gm_model::atlas::ADVANCE_PARTS
    }

    /// The face the HUD's own words are in: the text face when the atlas has it, else
    /// the small one.
    pub fn words(&self) -> FaceId {
        if self.has_face(FaceId::Text) {
            FaceId::Text
        } else {
            FaceId::Small
        }
    }

    /// The height of the capitals of the HUD's words, in dots.
    pub fn cap(&self) -> f32 {
        self.metrics(self.words()).1
    }

    /// The width of `text` in the HUD's words at `scale`.
    pub fn width(&self, scale: f32, text: &str) -> f32 {
        self.width_in(self.words(), scale, text)
    }

    /// Text in the HUD's words with the top of its capitals at `(x, y)`.
    pub fn print(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) -> f32 {
        self.text_in(self.words(), x, y, scale, color, text)
    }

    /// Text with its top left at `(x, y)` in the small face; returns where it ends.
    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) -> f32 {
        self.text_in(FaceId::Small, x, y, scale, color, text)
    }

    /// Text in `face` with the top of its line at `(x, y)`; returns where it ends.
    pub fn text_in(
        &mut self,
        face: FaceId,
        x: f32,
        y: f32,
        scale: f32,
        color: [f32; 4],
        text: &str,
    ) -> f32 {
        let f = self.face(face);
        let unknown = f.glyphs.get(&'?').copied();
        let glyphs: Vec<Glyph> = text
            .chars()
            .filter_map(|c| f.glyphs.get(&c).copied().or(unknown))
            .collect();
        // Cells and bearings are texels: `k` pixels each (one, at the atlas's own scale).
        let k = scale / self.atlas.dots();
        let y = y.round();
        let mut at = x.round();
        for g in glyphs {
            if g.cell.w > 0 && g.cell.h > 0 {
                self.quad_cell(
                    [
                        at + g.bearing[0] as f32 * k,
                        y + g.bearing[1] as f32 * k,
                        g.cell.w as f32 * k,
                        g.cell.h as f32 * k,
                    ],
                    g.cell,
                    color,
                );
            }
            at += g.advance as f32 * scale / gm_model::atlas::ADVANCE_PARTS;
        }
        at
    }

    /// The HUD's words with a dark plate behind them: readable over any wall.
    pub fn label(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) -> f32 {
        let w = self.width(scale, text);
        let (line, cap) = self.metrics(self.words());
        self.rect(
            x - 3.0 * scale,
            y - 2.0 * scale,
            w + 6.0 * scale,
            (cap + 2.0 + (line - cap).max(2.0)) * scale,
            SHADE,
        );
        self.print(x, y, scale, color, text)
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
        let total: usize = self.layers.iter().map(Vec::len).sum();
        if total > self.capacity {
            self.capacity = total.next_power_of_two();
            self.vertex_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hud vertices"),
                size: (self.capacity * std::mem::size_of::<HudVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut all: Vec<HudVertex> = Vec::with_capacity(total);
        for (i, l) in self.layers.iter().enumerate() {
            let start = all.len() as u32;
            all.extend_from_slice(l);
            self.ranges[i] = (start, all.len() as u32);
        }
        if !all.is_empty() {
            gpu.queue
                .write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&all));
        }
    }

    /// Draw layers `from..=to` of what `prepare` uploaded.
    pub fn draw_layers(&self, pass: &mut wgpu::RenderPass<'_>, from: usize, to: usize) {
        let (a, b) = (
            self.ranges[from.min(LAYERS - 1)].0,
            self.ranges[to.min(LAYERS - 1)].1,
        );
        if b <= a {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(a..b, 0..1);
    }

    /// Draw every layer.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_layers(pass, 0, LAYERS - 1);
    }
}

impl crate::ui::Canvas for Hud {
    fn size(&self) -> (f32, f32) {
        self.size
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        Hud::rect(self, x, y, w, h, color);
    }

    fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) {
        Hud::text(self, x, y, scale, color, text);
    }

    fn text_in(&mut self, face: FaceId, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) {
        Hud::text_in(self, face, x, y, scale, color, text);
    }

    fn width_in(&self, face: FaceId, scale: f32, text: &str) -> f32 {
        Hud::width_in(self, face, scale, text)
    }

    fn metrics(&self, face: FaceId) -> (f32, f32) {
        Hud::metrics(self, face)
    }

    fn has_face(&self, face: FaceId) -> bool {
        Hud::has_face(self, face)
    }

    fn image(&mut self, x: f32, y: f32, w: f32, h: f32, piece: &str, color: [f32; 4]) -> bool {
        Hud::image(self, x, y, w, h, piece, color)
    }

    fn frame(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        piece: &str,
        scale: f32,
        color: [f32; 4],
    ) -> bool {
        Hud::frame(self, x, y, w, h, piece, scale, color)
    }

    fn icon(&mut self, x: f32, y: f32, side: f32, key: &str, color: [f32; 4]) -> bool {
        Hud::icon(self, x, y, side, key, color)
    }

    fn layer(&mut self, n: usize) {
        Hud::set_layer(self, n);
    }
}
