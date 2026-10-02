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
    let mut tactical = crate::tactical::Tactical::new();
    if opts.tactical {
        tactical.enter(sim.yaw);
    }
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
        let eye = sim.eye();
        // The tactical viewport: the camera above the body, the world drawn from the
        // body's leaf (the camera itself hangs in the rock over the ceiling).
        let (camera, cam_yaw, cam_pitch) = if opts.tactical {
            tactical.steer(0.0, 0.0, 0.2, 0.0, dt);
            (
                tactical.camera(sim.origin()),
                tactical.yaw,
                crate::tactical::PITCH,
            )
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
        boxes.clear();
        avatars.begin_frame();
        avatars.push_crowd(time, dt, camera, &bsp, &renderer.characters, &mut boxes);
        let vp = view_proj(camera, cam_yaw, cam_pitch, w as f32 / h as f32);
        renderer.hud.begin((w, h));
        if opts.tactical {
            crate::app::build_hud(&mut renderer.hud, None, &tactical, vp, &[], &[], None);
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
