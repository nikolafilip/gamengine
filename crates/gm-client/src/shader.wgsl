// World shader: diffuse texture array modulated by the lightmap atlas. No dynamic lighting yet.

struct Globals {
    view_proj: mat4x4<f32>,
    // x = lightmap scale; y, z, w reserved.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var diffuse: texture_2d_array<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var lightmap_sampler: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) layer: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.layer = in.layer;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(diffuse, diffuse_sampler, in.uv, i32(in.layer));
    let light = textureSample(lightmap, lightmap_sampler, in.lm_uv).rgb * globals.params.x;
    return vec4<f32>(albedo.rgb * light, 1.0);
}
