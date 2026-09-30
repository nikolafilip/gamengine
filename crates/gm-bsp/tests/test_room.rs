//! Integration tests against the committed test map (assets/maps/built/test_room.bsp). Rebuild it
//! with `cargo run -p gm-tools -- map build assets/maps/src/test_room.map` after editing the map.

use std::path::PathBuf;

use glam::Vec3;
use gm_bsp::{Bsp, Version};
use gm_core::movement::{MoveInput, MoveVars, PlayerState, player_move};
use gm_core::trace::{CollisionWorld, Contents, Hull};

fn map_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/built/test_room.bsp")
}

fn load() -> Bsp {
    Bsp::load(&map_path()).expect("test_room.bsp must exist; run gm-tools map build")
}

const REST_Z: f32 = 24.0;

#[test]
fn loads_geometry_textures_and_entities() {
    let bsp = load();
    assert_eq!(bsp.version, Version::Bsp29);
    assert!(bsp.faces.len() >= 60, "faces = {}", bsp.faces.len());
    assert!(bsp.leaves.len() >= 8 && bsp.nodes.len() >= 8);
    assert!(!bsp.clipnodes.is_empty());
    assert_eq!(bsp.models.len(), 1);

    let names: Vec<&str> = bsp.textures.iter().map(|t| t.name.as_str()).collect();
    for expected in [
        "floor_stone",
        "wall_brick",
        "ceil_plaster",
        "ramp_wood",
        "trim_dark",
        "metal_panel",
    ] {
        assert!(
            names.contains(&expected),
            "missing texture {expected}: {names:?}"
        );
    }
    for t in bsp.textures.iter().filter(|t| !t.name.is_empty()) {
        assert_eq!((t.width, t.height), (64, 64), "{}", t.name);
        assert_eq!(
            t.pixels.as_ref().map(|p| p.len()),
            Some(64 * 64),
            "{} has no embedded pixels",
            t.name
        );
    }

    let world = bsp.world();
    assert!(world.mins.x <= -256.0 && world.maxs.x >= 256.0);
    assert!(world.mins.z <= 0.0 && world.maxs.z >= 192.0);

    assert_eq!(
        bsp.entities
            .iter()
            .filter(|e| e.classname() == "light")
            .count(),
        5
    );
    let (spawn, angle) = bsp.player_start().expect("info_player_start");
    assert_eq!(spawn, Vec3::new(-176.0, 96.0, 40.0));
    assert_eq!(angle, 0.0);
    assert_eq!(bsp.entities[0].get("wad"), Some("base.wad"));
}

#[test]
fn every_face_has_a_lightmap_inside_the_lump() {
    let bsp = load();
    assert!(
        bsp.lit.is_some(),
        "colored .lit must be loaded next to the bsp"
    );
    let stats = bsp.lightmap_stats();
    assert_eq!(stats.lit_faces, bsp.faces.len());
    assert!(stats.luxels > 1000);
    assert!(stats.max_width <= 18 && stats.max_height <= 18, "{stats:?}");
    let mut total = 0usize;
    for i in 0..bsp.faces.len() {
        let lm = bsp.face_lightmap(i).expect("lightmap");
        assert_eq!(lm.gray.len(), (lm.width * lm.height) as usize);
        assert_eq!(
            lm.rgb.map(|r| r.len()),
            Some((lm.width * lm.height * 3) as usize)
        );
        total += lm.gray.len();
        // Face vertices must sit inside the lightmap extents.
        let ti = &bsp.texinfo[bsp.faces[i].texinfo as usize];
        for v in bsp.face_vertices(i) {
            let (s, t) = Bsp::texcoord(ti, v);
            let (ls, lt) = (
                (s - lm.mins[0] as f32) / 16.0,
                (t - lm.mins[1] as f32) / 16.0,
            );
            assert!(
                ls >= -0.01 && ls <= lm.width as f32 - 1.0 + 0.01,
                "face {i}: s = {ls}"
            );
            assert!(
                lt >= -0.01 && lt <= lm.height as f32 - 1.0 + 0.01,
                "face {i}: t = {lt}"
            );
        }
    }
    assert!(total <= bsp.lighting.len());
    // Something is actually lit, and the red light shows up in the color data.
    assert!(bsp.lighting.iter().any(|&b| b > 64));
    let lit = bsp.lit.as_ref().unwrap();
    assert!(
        lit.chunks(3).any(|c| c[0] > c[1] + 40 && c[0] > c[2] + 40),
        "no reddish luxel found"
    );
}

#[test]
fn leaves_and_contents() {
    let bsp = load();
    let spawn = bsp.player_start().unwrap().0;
    let leaf = bsp.leaf_for_point(spawn);
    assert_ne!(leaf, 0);
    assert_eq!(
        Contents::from_i32(bsp.leaves[leaf].contents),
        Some(Contents::Empty)
    );
    assert_eq!(bsp.point_contents(Hull::Point, spawn), Contents::Empty);
    // Inside the +x wall, and far outside the map.
    assert_eq!(
        bsp.point_contents(Hull::Point, Vec3::new(264.0, 0.0, 96.0)),
        Contents::Solid
    );
    assert_eq!(
        bsp.point_contents(Hull::Point, Vec3::new(2000.0, 0.0, 0.0)),
        Contents::Solid
    );
    // The player hull is solid when its box overlaps the floor even though its origin is in the open.
    assert_eq!(
        bsp.point_contents(Hull::Player, Vec3::new(0.0, 0.0, 10.0)),
        Contents::Solid
    );
    assert_eq!(
        bsp.point_contents(Hull::Player, Vec3::new(0.0, 0.0, 40.0)),
        Contents::Empty
    );
}

#[test]
fn pvs_rows_are_well_formed() {
    let bsp = load();
    let row_len = bsp.pvs_row_bytes();
    assert!(!bsp.visdata.is_empty(), "vis must have run");
    for leaf in 1..bsp.leaves.len() {
        if Contents::from_i32(bsp.leaves[leaf].contents) == Some(Contents::Solid) {
            continue;
        }
        let row = bsp.decompress_pvs(leaf);
        assert_eq!(row.len(), row_len);
        assert!(
            Bsp::leaf_in_pvs(&row, leaf),
            "leaf {leaf} does not see itself"
        );
        assert!(!Bsp::leaf_in_pvs(&row, 0), "solid leaf marked visible");
    }
    let spawn_leaf = bsp.leaf_for_point(bsp.player_start().unwrap().0);
    let visible = bsp.visible_faces(spawn_leaf);
    assert!(!visible.is_empty() && visible.len() <= bsp.faces.len());
    let mut sorted = visible.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        visible.len(),
        "visible_faces must not repeat faces"
    );
}

#[test]
fn point_traces_hit_floor_walls_and_ramp() {
    let bsp = load();
    let eps = 0.1;
    // Straight down onto the floor (z = 0).
    let t = bsp.trace(
        Hull::Point,
        Vec3::new(-176.0, 96.0, 40.0),
        Vec3::new(-176.0, 96.0, -40.0),
    );
    assert!(t.hit() && !t.start_solid);
    assert!((t.end.z - 0.0).abs() < eps, "end = {:?}", t.end);
    assert!(
        (t.plane_normal - Vec3::Z).length() < 1e-3,
        "normal = {:?}",
        t.plane_normal
    );
    assert!((t.fraction - 0.5).abs() < 0.01);
    // Into the +x wall (inner face at x = 256).
    let t = bsp.trace(
        Hull::Point,
        Vec3::new(0.0, 0.0, 96.0),
        Vec3::new(400.0, 0.0, 96.0),
    );
    assert!(t.hit());
    assert!((t.end.x - 256.0).abs() < eps, "end = {:?}", t.end);
    assert!(
        (t.plane_normal - Vec3::NEG_X).length() < 1e-3,
        "normal = {:?}",
        t.plane_normal
    );
    // Onto the ramp: z = (x + 32) / 2 at x = 32 is 32.
    let t = bsp.trace(
        Hull::Point,
        Vec3::new(32.0, -128.0, 100.0),
        Vec3::new(32.0, -128.0, 0.0),
    );
    assert!(t.hit());
    assert!((t.end.z - 32.0).abs() < 0.2, "end = {:?}", t.end);
    let expected = Vec3::new(-1.0, 0.0, 2.0).normalize();
    assert!(
        (t.plane_normal - expected).length() < 1e-2,
        "normal = {:?}",
        t.plane_normal
    );
    // Free path across the room.
    let t = bsp.trace(
        Hull::Point,
        Vec3::new(-200.0, 150.0, 100.0),
        Vec3::new(200.0, 150.0, 100.0),
    );
    assert!(!t.hit(), "{t:?}");
    assert_eq!(t.fraction, 1.0);
}

#[test]
fn player_hull_traces_respect_the_box() {
    let bsp = load();
    // Down: the hull bottom (origin - 24) stops on the floor, so the origin stops at 24.
    let t = bsp.trace(
        Hull::Player,
        Vec3::new(-176.0, 96.0, 100.0),
        Vec3::new(-176.0, 96.0, -100.0),
    );
    assert!(t.hit());
    assert!((t.end.z - REST_Z).abs() < 0.1, "end = {:?}", t.end);
    assert!(t.plane_normal.z > 0.99);
    // Toward the +x wall: the hull's +x side (origin + 16) stops at 256.
    let t = bsp.trace(
        Hull::Player,
        Vec3::new(0.0, 0.0, 96.0),
        Vec3::new(400.0, 0.0, 96.0),
    );
    assert!(t.hit());
    assert!((t.end.x - 240.0).abs() < 0.1, "end = {:?}", t.end);
    // Start inside the wall: start_solid, no movement.
    let t = bsp.trace(
        Hull::Player,
        Vec3::new(250.0, 0.0, 96.0),
        Vec3::new(200.0, 0.0, 96.0),
    );
    assert!(t.start_solid);
}

fn settle(bsp: &Bsp, origin: Vec3) -> PlayerState {
    let mut st = PlayerState::new(origin);
    let dt = 1.0 / 64.0;
    for _ in 0..128 {
        player_move(bsp, &MoveVars::QUAKE, &mut st, &MoveInput::default(), dt);
    }
    st
}

#[test]
fn player_movement_in_the_room() {
    let bsp = load();
    let dt = 1.0 / 64.0;
    let spawn = bsp.player_start().unwrap().0;

    // Spawn, fall, rest.
    let mut st = settle(&bsp, spawn);
    assert!(st.on_ground);
    assert!(
        (st.origin.z - REST_Z).abs() < 0.1,
        "origin = {:?}",
        st.origin
    );

    // Walk +x from the spawn: the second pillar (x 64..96, y 96..128) stops the 32-wide hull at x = 48.
    let input = MoveInput {
        yaw: 0.0,
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..256 {
        player_move(&bsp, &MoveVars::QUAKE, &mut st, &input, dt);
    }
    assert!(
        st.origin.x > 47.0 && st.origin.x <= 48.05,
        "origin = {:?}",
        st.origin
    );
    assert!(
        (st.origin.y - 96.0).abs() < 0.5,
        "drifted sideways: {:?}",
        st.origin
    );
    assert!(st.on_ground);

    // Along y = 160 nothing is in the way until the +x wall stops us at x = 240.
    let mut st = settle(&bsp, Vec3::new(-176.0, 160.0, 40.0));
    for _ in 0..256 {
        player_move(&bsp, &MoveVars::QUAKE, &mut st, &input, dt);
    }
    assert!(
        st.origin.x > 239.0 && st.origin.x <= 240.05,
        "origin = {:?}",
        st.origin
    );
    assert!(
        (st.origin.y - 160.0).abs() < 0.5,
        "drifted sideways: {:?}",
        st.origin
    );
    assert!(st.on_ground);

    // Walk up the ramp onto the 64-unit platform.
    let mut st = settle(&bsp, Vec3::new(-100.0, -128.0, 40.0));
    for _ in 0..192 {
        player_move(&bsp, &MoveVars::QUAKE, &mut st, &input, dt);
    }
    assert!(st.on_ground, "{st:?}");
    assert!(
        (st.origin.z - (REST_Z + 64.0)).abs() < 0.2,
        "not on the platform: {:?}",
        st.origin
    );
    assert!(st.origin.x > 200.0, "{:?}", st.origin);

    // The 40-unit low wall (top at z = 40, higher than the 18-unit step) blocks a walker.
    // Approach along x = -48 so the hull (x -64..-32) stays clear of the ramp (x >= -32).
    let mut st = settle(&bsp, Vec3::new(-48.0, -100.0, 40.0));
    let north = MoveInput {
        yaw: 90.0,
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..128 {
        player_move(&bsp, &MoveVars::QUAKE, &mut st, &north, dt);
    }
    assert!(
        st.origin.y <= -32.0 - 16.0 + 0.05,
        "walked through the low wall: {:?}",
        st.origin
    );
    assert!(
        (st.origin.z - REST_Z).abs() < 0.1,
        "not on the floor: {st:?}"
    );
}
