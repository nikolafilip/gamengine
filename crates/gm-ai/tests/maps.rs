//! The navigation grid on the shipped maps (COMPANIONS.md 7, 14): built from collision alone,
//! inside the build-time budget, and connected the way the maps are.

use std::path::Path;
use std::time::Instant;

use glam::Vec3;
use gm_ai::nav::NavGrid;
use gm_bsp::Bsp;

const MAPS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");

fn load(name: &str) -> (Bsp, Vec<Vec3>) {
    let bsp = Bsp::load(&Path::new(MAPS).join(format!("{name}.bsp"))).expect("map built");
    let seeds = bsp
        .entities
        .iter()
        .filter(|e| {
            matches!(
                e.classname(),
                "gm_spawn" | "info_player_start" | "gm_creature"
            )
        })
        .filter_map(|e| e.origin())
        .collect();
    (bsp, seeds)
}

fn length(path: &[Vec3]) -> f32 {
    path.windows(2).map(|w| (w[1] - w[0]).length()).sum()
}

#[test]
fn the_arena_is_one_connected_floor_with_walkways() {
    let (bsp, seeds) = load("arena");
    let t0 = Instant::now();
    let grid = NavGrid::build(&bsp, &seeds);
    let took = t0.elapsed();
    println!("arena: {} nodes in {took:?}", grid.len());
    assert!(grid.len() > 4000, "{} nodes", grid.len());
    // From a team 1 spawn to a team 2 spawn: around the base cover, not through it.
    let a = Vec3::new(-1216.0, -96.0, 24.0);
    let b = Vec3::new(1216.0, 96.0, 24.0);
    let path = grid.path_between(a, b).expect("the bases are connected");
    let len = length(&path);
    assert!((2400.0..3400.0).contains(&len), "path of {len} u");
    // Up onto the north walkway (96 u high) by a ramp.
    let ledge = Vec3::new(0.0, 880.0, 120.0);
    let up = grid
        .path_between(a, ledge)
        .expect("the walkway is reachable");
    assert!(up.last().unwrap().z > 100.0);
    assert!(
        up.iter()
            .any(|p| p.x.abs() > 640.0 && p.z > 40.0 && p.z < 110.0),
        "by a ramp"
    );
    // The centre platform by its ramps.
    let centre = Vec3::new(120.0, 120.0, 72.0);
    assert!(grid.reachable(a, centre));
    #[cfg(not(debug_assertions))]
    assert!(took.as_millis() < 500, "nav build took {took:?}");
}

#[test]
fn the_town_and_the_test_room_flood_too() {
    for (name, min_nodes) in [("town", 2500), ("test_room", 150)] {
        let (bsp, seeds) = load(name);
        let t0 = Instant::now();
        let grid = NavGrid::build(&bsp, &seeds);
        println!("{name}: {} nodes in {:?}", grid.len(), t0.elapsed());
        assert!(grid.len() >= min_nodes, "{name}: {} nodes", grid.len());
    }
}

#[test]
fn the_dungeon_is_walkable_from_the_entry_to_the_warden() {
    let (bsp, seeds) = load("dungeon");
    let t0 = Instant::now();
    let grid = NavGrid::build(&bsp, &seeds);
    let took = t0.elapsed();
    println!("dungeon: {} nodes in {took:?}", grid.len());
    let entry = Vec3::new(-1900.0, 0.0, 24.0);
    let gate = Vec3::new(96.0, 480.0, 24.0);
    let warden = Vec3::new(1600.0, 352.0, 88.0);
    let to_gate = grid.path_between(entry, gate).expect("the passage");
    let to_warden = grid.path_between(entry, warden).expect("the stair");
    println!(
        "entry to gate {:.0} u, entry to the Warden {:.0} u",
        length(&to_gate),
        length(&to_warden)
    );
    // Through the passage with its two turns, not through rock: well over the straight line.
    assert!(length(&to_gate) > 2300.0, "{}", length(&to_gate));
    assert!(
        (3800.0..5200.0).contains(&length(&to_warden)),
        "{}",
        length(&to_warden)
    );
    // The stair is climbed: the path ends on the hall's raised floor.
    assert!(to_warden.last().unwrap().z > 80.0);
    assert!(
        to_warden.iter().any(|p| p.z > 30.0 && p.z < 80.0),
        "by the steps"
    );
    // The PVS cuts: the entry hall does not see the Warden's hall.
    let a = bsp.leaf_for_point(entry + Vec3::Z * 22.0);
    let b = bsp.leaf_for_point(warden + Vec3::Z * 22.0);
    let row = bsp.decompress_pvs(a);
    assert!(!Bsp::leaf_in_pvs(&row, b), "the entry hall sees the Warden");
    #[cfg(not(debug_assertions))]
    assert!(took.as_millis() < 500, "nav build took {took:?}");
}
