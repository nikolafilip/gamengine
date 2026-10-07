//! Offscreen rendering for CI and benchmarks: no window, same renderer, optional PPM screenshot.

use std::time::{Duration, Instant};

use gm_bsp::Bsp;
use gm_core::movement::MoveInput;

use crate::app::Sim;
use crate::avatars::Avatars;
use crate::render::{EntityDraw, Gpu, Renderer, view_proj};
use crate::stats::{FrameStats, print_bench, print_bench_avatars};
use crate::world;
use crate::{Error, Options};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// How long the crowd's models may take to load before the frames are counted anyway.
const WARM_UP: Duration = Duration::from_secs(60);

pub fn run(opts: &Options) -> Result<(), Error> {
    // A replay names its own map.
    let mut opts_with_map;
    let mut playback = None;
    let opts = if opts.replay.is_some() {
        opts_with_map = clone_for_replay(opts);
        playback = crate::app::open_replay(&mut opts_with_map)?;
        &opts_with_map
    } else {
        opts
    };
    let bsp = Bsp::load(&opts.map).map_err(|e| format!("loading {}: {e}", opts.map.display()))?;
    let palette = world::load_palette(&opts.palette);
    let mesh = world::build(&bsp, &palette);
    let faces_total = mesh
        .face_ranges
        .iter()
        .filter(|r| r.index_count > 0)
        .count();

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let gpu = Gpu::new(&instance, None, opts.software)?;
    let (w, h) = (opts.width.max(1), opts.height.max(1));
    let mut renderer = Renderer::new(&gpu, FORMAT, &mesh, (w, h));
    let mut avatars = Avatars::new(&gpu, &mut renderer.characters, opts, &bsp, None)?;
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // Let the player settle on the floor, then sweep the camera like the windowed bench does.
    let mut sim = Sim::at(&bsp, opts.start);
    let idle = MoveInput {
        yaw: sim.yaw,
        ..Default::default()
    };
    sim.advance(&bsp, &idle, 2.0);
    let yaw0 = sim.yaw;
    let frames = opts.bench_frames.unwrap_or(120);
    let dt = 1.0 / 60.0;
    let mut stats = FrameStats::new();
    let mut leaf = None;
    let mut boxes: Vec<EntityDraw> = Vec::new();
    let mut time = 0.0f32;
    let warm_up = Instant::now();
    let mut counted = 0;
    while counted < frames {
        time += dt;
        if avatars.crowd_len() > 0 {
            sim.yaw = yaw0 + 22.0 * (time * 0.7).sin();
        } else {
            sim.yaw += 20.0 * dt;
        }
        let mut eye = sim.eye();
        let mut bodies: Vec<crate::avatars::Body> = Vec::new();
        boxes.clear();
        // A replay: the scene of its next moment, from the followed body's eyes or from
        // behind it.
        let mut replay_camera = None;
        if let Some(p) = &mut playback {
            p.advance(dt);
            let (at, yaw, pitch) = p.scene(opts.third_person, &mut bodies, &mut boxes);
            eye = at;
            replay_camera = Some(if opts.third_person {
                (
                    crate::app::third_person_camera(&bsp, at, yaw, pitch),
                    yaw,
                    pitch,
                )
            } else {
                (at, yaw, pitch)
            });
        }
        let (camera, cam_yaw, cam_pitch) = if let Some(cam) = replay_camera {
            cam
        } else {
            (eye, sim.yaw, sim.pitch)
        };
        let l = bsp.leaf_for_point(eye);
        if leaf != Some(l) {
            leaf = Some(l);
            if l == 0 {
                renderer.set_visible_faces(&gpu, None);
            } else {
                renderer.set_visible_faces(&gpu, Some(&bsp.visible_faces(l)));
            }
        }
        avatars.begin_frame();
        for body in &bodies {
            avatars.push(body, dt, &bsp, &renderer.characters, &mut boxes);
        }
        avatars.push_crowd(time, dt, camera, &bsp, &renderer.characters, &mut boxes);
        let vp = view_proj(camera, cam_yaw, cam_pitch, w as f32 / h as f32);
        renderer.hud.begin((w, h));
        if let Some(p) = &playback {
            p.hud(&mut renderer.hud);
        }
        renderer.render(&gpu, &view, vp, &boxes, &avatars.draws);
        avatars.end_frame(&gpu, &mut renderer.characters);
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("poll: {e}"))?;
        // Frames count once every model the crowd wears is on the GPU.
        let loading = avatars.cache.as_ref().is_some_and(|c| c.pending() > 0);
        if loading && warm_up.elapsed() < WARM_UP {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        stats.frame();
        counted += 1;
    }

    if let Some(path) = &opts.screenshot {
        write_ppm(&gpu, &target, w, h, path)?;
        log::info!("screenshot written to {}", path.display());
    }
    let report = stats.report();
    print_bench(
        &report,
        &gpu.info,
        "headless",
        renderer.faces_drawn,
        faces_total,
        renderer.draw_calls,
    );
    print_bench_avatars(&report, &avatars, &renderer);
    Ok(())
}

/// The options with their own `map`, for a replay to point at its map.
fn clone_for_replay(o: &Options) -> Options {
    Options {
        map: o.map.clone(),
        palette: o.palette.clone(),
        maps_dir: o.maps_dir.clone(),
        replay: o.replay.clone(),
        follow: o.follow.clone(),
        from: o.from,
        third_person: o.third_person,
        bench_frames: o.bench_frames,
        headless: o.headless,
        software: o.software,
        width: o.width,
        height: o.height,
        screenshot: o.screenshot.clone(),
        cache_dir: o.cache_dir.clone(),
        cache_mb: o.cache_mb,
        vram_mb: o.vram_mb,
        ..Options::default()
    }
}

fn write_ppm(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
    path: &std::path::Path,
) -> Result<(), Error> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let bytes_per_row = (w * 4).div_ceil(align) * align;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("screenshot"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| format!("poll: {e}"))?;
    rx.recv()??;
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    {
        let data = buffer
            .get_mapped_range(..)
            .map_err(|e| format!("map range: {e}"))?;
        for y in 0..h {
            let row = &data[(y * bytes_per_row) as usize..(y * bytes_per_row + w * 4) as usize];
            for px in row.chunks(4) {
                out.extend_from_slice(&px[..3]);
            }
        }
    }
    buffer.unmap();
    std::fs::write(path, out)?;
    Ok(())
}
