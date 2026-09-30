use criterion::{Criterion, criterion_group, criterion_main};
use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::trace::{CollisionWorld, Hull};
use std::hint::black_box;
use std::path::Path;

const MAP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/test_room.bsp"
);

fn bench(c: &mut Criterion) {
    let bsp = Bsp::load(Path::new(MAP)).expect("test map built");
    let start = Vec3::new(-176.0, 96.0, 40.0);
    c.bench_function("player_hull_trace_across_room", |b| {
        b.iter(|| black_box(bsp.trace(Hull::Player, start, Vec3::new(200.0, -150.0, 40.0))))
    });
    c.bench_function("point_trace_los", |b| {
        b.iter(|| black_box(bsp.trace(Hull::Point, start, Vec3::new(180.0, -120.0, 60.0))))
    });
    c.bench_function("leaf_for_point", |b| {
        b.iter(|| black_box(bsp.leaf_for_point(start)))
    });
    let leaf = bsp.leaf_for_point(start);
    c.bench_function("decompress_pvs", |b| {
        b.iter(|| black_box(bsp.decompress_pvs(leaf)))
    });
    c.bench_function("visible_faces", |b| {
        b.iter(|| black_box(bsp.visible_faces(leaf)))
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
