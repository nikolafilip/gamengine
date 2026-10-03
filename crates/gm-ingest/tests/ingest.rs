//! Ingestion end to end (MODELS.md 3–5, 11): what is accepted, what is refused and why, and
//! that nothing a file can say makes the pipeline panic.

use glam::Vec3;
use gm_core::vocab::ArchetypeFrame;
use gm_ingest::write::{GlbBuilder, container, png};
use gm_ingest::{Report, ingest, synth};
use gm_model::mannequin::{self, Shape};
use gm_model::rig::{self, BONES, bone};
use gm_model::{Model, limits};
use serde_json::{Value, json};

const STRIKER: ArchetypeFrame = ArchetypeFrame::Striker;

fn refused(upload: &[u8], frame: ArchetypeFrame) -> Report {
    let (report, ingested) = ingest(upload, frame);
    assert!(!report.ok && ingested.is_none(), "accepted: {report:?}");
    assert!(!report.violations.is_empty());
    assert!(report.id.is_empty());
    report
}

fn has(report: &Report, needle: &str) -> bool {
    report.violations.iter().any(|v| v.contains(needle))
}

/// Split a `.glb` into its JSON and its BIN chunk, let `f` edit the JSON, and wrap it again.
fn patch(glb: &[u8], f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let mut doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
    let bin = &glb[20 + json_len + 8..];
    f(&mut doc);
    container(&doc, bin)
}

/// A small avatar for tests that do not care about the texture's size.
fn small(seed: u64, frame: ArchetypeFrame) -> GlbBuilder {
    synth::avatar(seed, frame, 64)
}

#[test]
fn the_template_of_every_frame_is_accepted() {
    for frame in rig::FRAMES {
        let (report, ingested) = ingest(&synth::template(frame).build(), frame);
        assert!(report.ok, "{frame:?}: {:?}", report.violations);
        let i = ingested.unwrap();
        assert_eq!(gm_model::id_hex(&i.id), report.id);
        assert!(
            (3400..=3500).contains(&report.facts.triangles),
            "{:?}",
            report.facts
        );
        assert_eq!(report.facts.texture, [256, 256]);
        assert_eq!(report.facts.bones, BONES as u32);
        // The detailed figure is the mannequin with more sides: the same silhouette.
        assert!(
            (0.9..1.15).contains(&report.facts.coverage_front),
            "{:?}",
            report.facts
        );
        assert!(
            (0.9..1.15).contains(&report.facts.coverage_side),
            "{:?}",
            report.facts
        );
        // What comes out is what a client can read back.
        let model = Model::decode(&i.gmm).unwrap();
        assert_eq!(model, i.model);
        assert_eq!(model.frame, rig::frame_index(frame));
        assert!(!model.cutout() && !model.two_sided());
        assert!(i.gmm.len() < limits::MAX_FILE_BYTES);
        assert_eq!(&i.preview[1..4], b"PNG");
    }
}

#[test]
fn ingestion_is_deterministic_and_the_id_names_the_bytes() {
    let upload = small(7, STRIKER).build();
    let (a, ia) = ingest(&upload, STRIKER);
    let (b, ib) = ingest(&upload, STRIKER);
    assert!(a.ok, "{:?}", a.violations);
    assert_eq!(a, b);
    assert_eq!(ia.as_ref().unwrap().gmm, ib.unwrap().gmm);
    assert_eq!(
        ia.as_ref().unwrap().id,
        gm_model::model_id(&ia.as_ref().unwrap().gmm)
    );
    // Another seed is another model; so is the same file for another frame.
    let (c, _) = ingest(&small(8, STRIKER).build(), STRIKER);
    assert!(c.ok && c.id != a.id);
    let (d, _) = ingest(&upload, ArchetypeFrame::Caster);
    assert!(d.ok, "{:?}", d.violations);
    assert_ne!(d.id, a.id);
}

#[test]
fn generated_avatars_fill_the_budget_and_pass_for_every_frame() {
    let mut ids = std::collections::HashSet::new();
    for seed in 0..40u64 {
        let frame = rig::FRAMES[seed as usize % 4];
        let (report, _) = ingest(&small(seed, frame).build(), frame);
        assert!(report.ok, "seed {seed} {frame:?}: {:?}", report.violations);
        assert!(
            (3400..=3500).contains(&report.facts.triangles),
            "seed {seed}: {} triangles",
            report.facts.triangles
        );
        assert!(ids.insert(report.id), "seed {seed} repeats an id");
    }
}

#[test]
fn a_full_size_avatar_stays_under_the_payload_ceiling() {
    let (report, ingested) = ingest(&synth::avatar(3, STRIKER, 1024).build(), STRIKER);
    assert!(report.ok, "{:?}", report.violations);
    assert_eq!(report.facts.texture, [1024, 1024]);
    let i = ingested.unwrap();
    assert!(
        i.gmm.len() < limits::MAX_FILE_BYTES,
        "{} bytes",
        i.gmm.len()
    );
    assert_eq!(i.model.texture.len(), 699_048);
    println!(
        "full-size avatar: {} bytes on the wire, {} on the GPU, {} triangles, {} vertices",
        i.gmm.len(),
        i.model.gpu_bytes(),
        report.facts.triangles,
        report.facts.vertices
    );
}

#[test]
fn geometry_weights_and_pivots_survive() {
    let b = small(11, STRIKER);
    let (report, ingested) = ingest(&b.build(), STRIKER);
    assert!(report.ok, "{:?}", report.violations);
    let model = ingested.unwrap().model;
    let src = &b.mesh;
    assert_eq!(model.vertices.len(), src.positions.len());
    assert_eq!(model.triangles(), src.triangles());
    for i in (0..src.positions.len()).step_by(37) {
        assert!(
            (model.position(i) - src.positions[i]).length() < 0.02,
            "vertex {i}: {:?} vs {:?}",
            model.position(i),
            src.positions[i]
        );
        assert!(model.normal(i).dot(src.normals[i]) > 0.99);
        let uv = model.uv(i);
        assert!((uv[0] - src.uvs[i][0]).abs() < 1e-4 && (uv[1] - src.uvs[i][1]).abs() < 1e-4);
        // Every influence arrives on the same bone with the same weight, to a byte.
        let v = &model.vertices[i];
        for k in 0..4 {
            if src.weights[i][k] <= 0.0 {
                continue;
            }
            let got = (0..4)
                .find(|m| v.weights[*m] > 0 && v.joints[*m] == src.joints[i][k])
                .map(|m| v.weights[m] as f32 / 255.0);
            assert!(
                got.is_some_and(|w| (w - src.weights[i][k]).abs() < 0.01),
                "vertex {i}: {:?} {:?} vs {:?} {:?}",
                v.joints,
                v.weights,
                src.joints[i],
                src.weights[i]
            );
        }
    }
    for bone in 0..BONES {
        assert!(
            (model.pivot(bone) - src.pivots[bone]).length() < 1e-3,
            "{}",
            rig::NAMES[bone]
        );
    }
}

#[test]
fn over_budget_geometry_is_refused_with_the_count() {
    let mesh = mannequin::build(
        STRIKER,
        &Shape {
            sides: 30,
            ..Shape::detailed()
        },
    );
    assert!(mesh.triangles() > limits::MAX_TRIANGLES);
    let b = GlbBuilder::new(mesh, png(64, 64, synth::paint(1, 64).as_raw()));
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "triangles; the budget is 3500"),
        "{:?}",
        r.violations
    );
    assert!(r.facts.triangles > 3500);
}

#[test]
fn an_avatar_must_be_visible_where_it_can_be_hit() {
    // A stick figure: a third of the girth.
    let thin = mannequin::build(
        STRIKER,
        &Shape {
            girth: 0.3,
            shoulders: 0.4,
            ..Shape::detailed()
        },
    );
    let b = GlbBuilder::new(thin, png(64, 64, synth::paint(1, 64).as_raw()));
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "at least 50% is required"), "{:?}", r.violations);
    assert!(r.facts.coverage_front < 0.5);

    // A full body painted away with a cutout texture.
    let mut b = small(1, STRIKER);
    let mut clear = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 0]));
    clear.put_pixel(0, 0, image::Rgba([255, 255, 255, 255]));
    b.image = png(64, 64, clear.as_raw());
    b.alpha_mode = "MASK";
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "covers 0%"), "{:?}", r.violations);
    // The same texture on an opaque material is a white body: alpha is ignored.
    b.alpha_mode = "OPAQUE";
    assert!(ingest(&b.build(), STRIKER).0.ok);

    // One-sided cards facing forward only: nothing to see from behind.
    let mut b = small(1, STRIKER);
    let keep: Vec<u32> = b
        .mesh
        .indices
        .chunks_exact(3)
        .filter(|t| {
            let n = b.mesh.normals[t[0] as usize]
                + b.mesh.normals[t[1] as usize]
                + b.mesh.normals[t[2] as usize];
            n.x > 0.0
        })
        .flatten()
        .copied()
        .collect();
    b.mesh.indices = keep;
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "from the front the model covers 0%"),
        "{:?}",
        r.violations
    );
    // Declared two-sided, the same half shell shows from both sides.
    b.double_sided = true;
    let (r, i) = ingest(&b.build(), STRIKER);
    assert!(r.ok, "{:?}", r.violations);
    assert!(i.unwrap().model.two_sided());
}

#[test]
fn a_model_must_fit_its_frame() {
    // A striker-sized body as a colossus is too small, as an infiltrator too big.
    let upload = small(2, STRIKER).build();
    assert!(ingest(&upload, STRIKER).0.ok);
    let mut big = small(2, STRIKER);
    for p in &mut big.mesh.positions {
        *p *= 1.6;
    }
    for p in &mut big.mesh.pivots {
        *p *= 1.6;
    }
    let r = refused(&big.build(), STRIKER);
    assert!(has(&r, "the `head` pivot is at"), "{:?}", r.violations);
    assert!(has(&r, "the highest vertex is at"), "{:?}", r.violations);
    assert!(has(&r, "at most 150% is allowed"), "{:?}", r.violations);
    // Far out of the box: a prop a hundred units out.
    let mut far = small(2, STRIKER);
    far.mesh.positions[0] = Vec3::new(120.0, 0.0, 30.0);
    let r = refused(&far.build(), STRIKER);
    assert!(
        has(&r, "vertices are outside the box"),
        "{:?}",
        r.violations
    );
    // An infiltrator cannot dress as a colossus: the colossus template is too tall and too
    // broad for the infiltrator's frame.
    let colossus = synth::template(ArchetypeFrame::Colossus).build();
    let r = refused(&colossus, ArchetypeFrame::Infiltrator);
    assert!(has(&r, "the `head` pivot is at"), "{:?}", r.violations);
}

#[test]
fn the_pose_must_be_the_t_pose() {
    // Arms lowered 45°: every arm vertex and pivot rotated about the shoulder.
    let mut b = small(4, STRIKER);
    for (side, upper, chain) in [
        (
            1.0f32,
            bone::UPPER_ARM_L,
            [bone::FOREARM_L, bone::HAND_L, bone::PROP_L],
        ),
        (
            -1.0,
            bone::UPPER_ARM_R,
            [bone::FOREARM_R, bone::HAND_R, bone::PROP_R],
        ),
    ] {
        let shoulder = b.mesh.pivots[upper];
        let rot = glam::Quat::from_rotation_x(-side * std::f32::consts::FRAC_PI_4);
        let turn = |p: Vec3| shoulder + rot * (p - shoulder);
        for c in chain {
            b.mesh.pivots[c] = turn(b.mesh.pivots[c]);
        }
        for i in 0..b.mesh.positions.len() {
            let j = b.mesh.joints[i][0] as usize;
            if j == upper || chain.contains(&j) {
                b.mesh.positions[i] = turn(b.mesh.positions[i]);
            }
        }
    }
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "`upper_arm_l` points 45° away from the T-pose"),
        "{:?}",
        r.violations
    );
    assert!(has(&r, "`upper_arm_r` points 45°"), "{:?}", r.violations);

    // Mirrored: left and right swapped.
    let mut b = small(4, STRIKER);
    for p in b.mesh.positions.iter_mut().chain(b.mesh.pivots.iter_mut()) {
        p.y = -p.y;
    }
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "mirrored or faces backward"), "{:?}", r.violations);
}

#[test]
fn the_rig_is_matched_by_name_and_extra_joints_fold_into_it() {
    // Blender-style names match.
    let mut b = small(5, STRIKER);
    b.bone_names[bone::UPPER_ARM_L] = "Upper_Arm.L".into();
    b.bone_names[bone::HIPS] = "HIPS".into();
    assert!(ingest(&b.build(), STRIKER).0.ok);

    // A missing required bone is named.
    let mut b = small(5, STRIKER);
    b.include &= !(1 << bone::FOREARM_L);
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "required bones are missing: forearm_l"),
        "{:?}",
        r.violations
    );

    // Optional bones may be absent: their vertices follow the parent.
    let mut b = small(5, STRIKER);
    for optional in [
        bone::TOE_L,
        bone::TOE_R,
        bone::CLAVICLE_L,
        bone::CLAVICLE_R,
        bone::NECK,
    ] {
        b.include &= !(1 << optional);
        let parent = rig::parent(optional).unwrap() as u8;
        for j in b.mesh.joints.iter_mut().flatten() {
            if *j == optional as u8 {
                *j = parent;
            }
        }
    }
    b.mesh.bone_mask = b.include;
    let (r, i) = ingest(&b.build(), STRIKER);
    assert!(r.ok, "{:?}", r.violations);
    let model = i.unwrap().model;
    assert_eq!(model.bone_mask, b.include);
    assert_eq!(model.pivot(bone::TOE_L), model.pivot(bone::FOOT_L));

    // A finger joint under the hand: its vertices end up on the hand.
    let mut b = small(5, STRIKER);
    let wrist = b.mesh.pivots[bone::HAND_L];
    b.extra_joints
        .push(("finger_01_l".into(), bone::HAND_L, wrist + Vec3::Y * 2.0));
    let mut moved = 0;
    for j in &mut b.mesh.joints {
        if j[0] == bone::HAND_L as u8 {
            j[0] = BONES as u8;
            moved += 1;
        }
    }
    assert!(moved > 0);
    let (r, i) = ingest(&b.build(), STRIKER);
    assert!(r.ok, "{:?}", r.violations);
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("1 joints are not standard bones")),
        "{:?}",
        r.notes
    );
    assert_eq!(r.facts.source_joints, BONES as u32 + 1);
    let model = i.unwrap().model;
    assert!(
        model
            .vertices
            .iter()
            .any(|v| v.joints[0] == bone::HAND_L as u8)
    );
    assert!(
        model
            .vertices
            .iter()
            .all(|v| (v.joints[0] as usize) < BONES)
    );

    // Two joints with the same standard name, and a rig wired wrongly.
    let mut b = small(5, STRIKER);
    b.bone_names[bone::SHIN_R] = "shin_l".into();
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "two joints are named `shin_l`"),
        "{:?}",
        r.violations
    );
    let mut b = small(5, STRIKER);
    b.bone_names[bone::THIGH_L] = "shin_l".into();
    b.bone_names[bone::SHIN_L] = "thigh_l".into();
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "`shin_l` must descend from `thigh_l`"),
        "{:?}",
        r.violations
    );
}

#[test]
fn textures_are_resampled_and_checked() {
    // A JPEG atlas of an odd size.
    let mut b = small(6, STRIKER);
    let img = image::RgbImage::from_fn(300, 150, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    b.image = jpeg;
    b.mime = "image/jpeg";
    let (r, _) = ingest(&b.build(), STRIKER);
    assert!(r.ok, "{:?}", r.violations);
    assert_eq!(r.facts.source_texture, [300, 150]);
    assert_eq!(r.facts.texture, [256, 128]);

    // Not an image at all, and an image of a kind we do not take.
    let mut b = small(6, STRIKER);
    b.image = vec![0x42; 300];
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "the texture"), "{:?}", r.violations);
    let mut b = small(6, STRIKER);
    b.image = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;".to_vec();
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "must be a PNG or a JPEG"), "{:?}", r.violations);

    // A decompression bomb: a PNG header that claims 30,000 × 30,000.
    let mut bomb = png(1, 1, &[0, 0, 0, 255]);
    bomb[16..20].copy_from_slice(&30_000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&30_000u32.to_be_bytes());
    let mut b = small(6, STRIKER);
    b.image = bomb;
    let r = refused(&b.build(), STRIKER);
    assert!(has(&r, "the texture"), "{:?}", r.violations);

    // Texture coordinates that tile.
    let mut b = small(6, STRIKER);
    for uv in b.mesh.uvs.iter_mut().take(10) {
        uv[0] += 1.5;
    }
    let r = refused(&b.build(), STRIKER);
    assert!(
        has(&r, "10 vertices have texture coordinates outside [0, 1]"),
        "{:?}",
        r.violations
    );
}

#[test]
fn files_that_are_not_what_they_claim_are_refused_without_a_panic() {
    let good = small(9, STRIKER).build();
    // Not a .glb; a .gltf (JSON only); garbage with the right magic.
    assert!(has(
        &refused(b"hello", STRIKER),
        "must be a glTF 2.0 binary"
    ));
    assert!(has(
        &refused(b"{\"asset\":{\"version\":\"2.0\"}}", STRIKER),
        "must be a glTF 2.0 binary"
    ));
    let mut junk = b"glTF".to_vec();
    junk.extend((0..500u32).flat_map(|i| i.wrapping_mul(2_654_435_761).to_le_bytes()));
    assert!(has(&refused(&junk, STRIKER), "not a valid glTF"));
    // Truncated anywhere.
    for cut in [
        0,
        4,
        12,
        13,
        20,
        64,
        400,
        good.len() / 3,
        good.len() / 2,
        good.len() - 9,
        good.len() - 1,
    ] {
        let (r, i) = ingest(&good[..cut], STRIKER);
        assert!(!r.ok && i.is_none(), "cut at {cut} accepted");
    }
    // Over the upload limit.
    let huge = vec![0u8; gm_ingest::MAX_UPLOAD_BYTES + 1];
    assert!(has(&refused(&huge, STRIKER), "the limit is"));

    // An accessor that claims more elements than its buffer holds.
    let lying = patch(&good, |d| d["accessors"][0]["count"] = json!(50_000_000u32));
    assert!(has(&refused(&lying, STRIKER), "reaches past its buffer"));
    // A buffer view past the end of the BIN chunk.
    let lying = patch(&good, |d| {
        d["bufferViews"][0]["byteLength"] = json!(900_000_000u32)
    });
    let r = refused(&lying, STRIKER);
    assert!(
        has(&r, "reaches past its buffer") || has(&r, "not a valid glTF"),
        "{:?}",
        r.violations
    );
    // Indices pointing past the vertices.
    let lying = patch(&good, |d| {
        let idx = d["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize;
        // Read the indices as u32 instead of u16: every pair becomes one huge index.
        d["accessors"][idx]["componentType"] = json!(5125);
        let count = d["accessors"][idx]["count"].as_u64().unwrap();
        d["accessors"][idx]["count"] = json!(count / 2);
    });
    assert!(has(
        &refused(&lying, STRIKER),
        "an index is past the vertices"
    ));
    // A joint index past the skin.
    let lying = patch(&good, |d| {
        let joints = d["skins"][0]["joints"].as_array().unwrap().clone();
        d["skins"][0]["joints"] = json!(joints[..5]);
        d["skins"][0]
            .as_object_mut()
            .unwrap()
            .remove("inverseBindMatrices");
    });
    let r = refused(&lying, STRIKER);
    assert!(
        has(&r, "required bones are missing") || has(&r, "past the skin"),
        "{:?}",
        r.violations
    );
    // Sparse accessors, external images, external buffers, no skin, two skins, lines.
    let sparse = patch(&good, |d| {
        d["accessors"][0]["sparse"] = json!({
            "count": 1,
            "indices": { "bufferView": 0, "componentType": 5125 },
            "values": { "bufferView": 0 }
        })
    });
    assert!(has(
        &refused(&sparse, STRIKER),
        "sparse accessors are not supported"
    ));
    let external = patch(&good, |d| {
        d["images"][0] = json!({ "uri": "http://example.com/a.png" })
    });
    assert!(has(&refused(&external, STRIKER), "must be embedded"));
    let external = patch(&good, |d| d["buffers"][0]["uri"] = json!("data.bin"));
    assert!(!ingest(&external, STRIKER).0.ok);
    let none = patch(&good, |d| {
        d.as_object_mut().unwrap().remove("skins");
        d["nodes"][0].as_object_mut().unwrap().remove("skin");
    });
    assert!(has(&refused(&none, STRIKER), "0 skins"));
    let two = patch(&good, |d| {
        let s = d["skins"][0].clone();
        d["skins"].as_array_mut().unwrap().push(s);
    });
    assert!(has(&refused(&two, STRIKER), "2 skins"));
    let lines = patch(&good, |d| {
        d["meshes"][0]["primitives"][0]["mode"] = json!(1)
    });
    assert!(has(&refused(&lines, STRIKER), "only triangles are drawn"));
    // A node that is its own grandchild.
    let cycle = patch(&good, |d| {
        let n = d["nodes"].as_array().unwrap().len();
        d["nodes"][n - 1]["children"] = json!([1]);
    });
    assert!(!ingest(&cycle, STRIKER).0.ok);
    // No texture on the material; no material at all.
    let bare = patch(&good, |d| {
        d["materials"][0]["pbrMetallicRoughness"] = json!({});
    });
    assert!(has(&refused(&bare, STRIKER), "no base colour texture"));
    // NaN geometry.
    let mut b = small(9, STRIKER);
    b.mesh.positions[3] = Vec3::new(f32::NAN, 0.0, 0.0);
    assert!(!ingest(&b.build(), STRIKER).0.ok);
}

#[test]
fn every_byte_flip_in_the_header_region_is_survived() {
    // Not a fuzzer, but every single-byte corruption of the first kilobyte and a sample of
    // the rest must come back as a report, never as a panic.
    let good = small(10, STRIKER).build();
    let mut flips: Vec<usize> = (0..1024.min(good.len())).collect();
    flips.extend((1024..good.len()).step_by(997));
    for at in flips {
        let mut bad = good.clone();
        bad[at] ^= 0xff;
        let _ = ingest(&bad, STRIKER);
    }
}

#[test]
fn missing_normals_are_computed_and_strips_are_triangles() {
    let mut b = small(12, STRIKER);
    b.normals = false;
    let (r, i) = ingest(&b.build(), STRIKER);
    assert!(r.ok, "{:?}", r.violations);
    assert!(r.notes.iter().any(|n| n.contains("normals were missing")));
    let model = i.unwrap().model;
    // Computed normals still point outward: compare with the generator's.
    let agree = (0..model.vertices.len())
        .filter(|i| model.normal(*i).dot(b.mesh.normals[*i]) > 0.5)
        .count();
    assert!(
        agree > model.vertices.len() * 9 / 10,
        "{agree} of {}",
        model.vertices.len()
    );

    // The same triangles as one strip per triangle (mode 5 with 3 indices each is the same).
    let strip = patch(&small(12, STRIKER).build(), |d| {
        d["meshes"][0]["primitives"][0]["mode"] = json!(5);
    });
    let (r, _) = ingest(&strip, STRIKER);
    // A list read as a strip makes many more triangles: over budget, but parsed.
    assert!(
        !r.ok && has(&r, "triangles; the budget is"),
        "{:?}",
        r.violations
    );
}

#[test]
fn the_worker_writes_a_report_and_the_artifacts() {
    let dir = std::env::temp_dir().join(format!("gm-ingest-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("upload.glb");
    std::fs::write(&input, small(13, STRIKER).build()).unwrap();
    let report = gm_ingest::worker(&input, STRIKER, &dir).unwrap();
    assert!(report.ok);
    let json = std::fs::read(dir.join(gm_ingest::WORKER_REPORT)).unwrap();
    assert_eq!(serde_json::from_slice::<Report>(&json).unwrap(), report);
    let gmm = std::fs::read(dir.join(gm_ingest::WORKER_MODEL)).unwrap();
    assert_eq!(gm_model::id_hex(&gm_model::model_id(&gmm)), report.id);
    // The worker hands back the model and no picture of it: the hub draws its own.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    // A refused upload leaves a report and nothing else.
    let out = dir.join("refused");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(&input, b"not a model").unwrap();
    let report = gm_ingest::worker(&input, STRIKER, &out).unwrap();
    assert!(!report.ok);
    assert!(out.join(gm_ingest::WORKER_REPORT).exists());
    assert!(!out.join(gm_ingest::WORKER_MODEL).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// MODELS.md 6.2: the worker's answer is checked from the stored bytes alone.
#[test]
fn a_stored_model_is_verified_from_its_own_bytes() {
    let (_, i) = ingest(&small(21, STRIKER).build(), STRIKER);
    let i = i.expect("accepted");
    // The honest answer verifies, to the same id and the same preview.
    let v = gm_ingest::verify(&i.gmm, STRIKER).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(v.id, i.id);
    assert_eq!(v.preview, i.preview);
    assert_eq!(v.model, i.model);

    // Not a model at all, and a model for another frame.
    assert!(gm_ingest::verify(b"GMM1 but not really", STRIKER).is_err());
    let why = gm_ingest::verify(&i.gmm, ArchetypeFrame::Colossus).unwrap_err();
    assert!(why.iter().any(|w| w.contains("another frame")), "{why:?}");

    // A well-formed container whose model breaks the envelope: the same avatar at a third of
    // its size is a small target the strict reader has no objection to.
    let mut tiny = i.model.clone();
    for s in &mut tiny.scale {
        *s *= 0.3;
    }
    for p in tiny.pivots.iter_mut().flatten() {
        *p *= 0.3;
    }
    let bytes = tiny.encode().expect("a valid container");
    assert!(Model::decode(&bytes).is_ok());
    let why = gm_ingest::verify(&bytes, STRIKER).unwrap_err();
    assert!(why.iter().any(|w| w.contains("covers")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("`head` pivot")), "{why:?}");

    // A model whose limbs were re-posed after the checks: arms down its sides.
    let mut posed = i.model.clone();
    for (upper, fore, hand) in [
        (bone::UPPER_ARM_L, bone::FOREARM_L, bone::HAND_L),
        (bone::UPPER_ARM_R, bone::FOREARM_R, bone::HAND_R),
    ] {
        let shoulder = posed.pivots[upper];
        for (b, drop) in [(fore, 10.0), (hand, 20.0)] {
            posed.pivots[b] = [shoulder[0], shoulder[1], shoulder[2] - drop];
        }
    }
    let why = gm_ingest::verify(&posed.encode().unwrap(), STRIKER).unwrap_err();
    assert!(why.iter().any(|w| w.contains("T-pose")), "{why:?}");

    // One model, one id: the same content compressed differently, or with a byte after the
    // end, decodes (or not) but is never accepted.
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&i.gmm[12..]).unwrap();
    let mut other = i.gmm[..12].to_vec();
    other.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&raw, 1));
    assert_ne!(other, i.gmm);
    assert_eq!(Model::decode(&other).as_ref(), Ok(&i.model));
    let why = gm_ingest::verify(&other, STRIKER).unwrap_err();
    assert!(why.iter().any(|w| w.contains("canonical")), "{why:?}");
    let mut padded = i.gmm.clone();
    padded.push(0);
    assert!(gm_ingest::verify(&padded, STRIKER).is_err());
}

/// A small file can ask for a lot: one mesh of three vertices and 600,000 indices, drawn by
/// a thousand nodes. The reader stops at its totals instead of building 600 million triangles.
#[test]
fn an_instanced_index_bomb_is_refused_at_once() {
    let glb = small(31, STRIKER).build();
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let mut doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
    let mut bin = glb[20 + json_len + 8..].to_vec();
    // 600,000 zero bytes: indices that all name vertex 0.
    const INDICES: usize = 600_000;
    let offset = bin.len();
    bin.resize(offset + INDICES, 0);
    doc["buffers"][0]["byteLength"] = json!(bin.len());
    let view = doc["bufferViews"].as_array().unwrap().len();
    doc["bufferViews"]
        .as_array_mut()
        .unwrap()
        .push(json!({"buffer": 0, "byteOffset": offset, "byteLength": INDICES}));
    let accessor = doc["accessors"].as_array().unwrap().len();
    doc["accessors"].as_array_mut().unwrap().push(json!({
        "bufferView": view, "componentType": 5121, "count": INDICES, "type": "SCALAR"
    }));
    doc["meshes"][0]["primitives"][0]["indices"] = json!(accessor);
    let mesh_node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n.get("mesh").is_some())
        .cloned()
        .expect("a mesh node");
    let first = doc["nodes"].as_array().unwrap().len();
    for _ in 0..1000 {
        doc["nodes"].as_array_mut().unwrap().push(mesh_node.clone());
    }
    let scene = doc["scenes"][0]["nodes"].as_array_mut().unwrap();
    scene.extend((first..first + 1000).map(|i| json!(i)));
    let bomb = container(&doc, &bin);
    assert!(bomb.len() < 2 * 1024 * 1024, "{} bytes", bomb.len());

    let started = std::time::Instant::now();
    let report = refused(&bomb, STRIKER);
    assert!(
        has(&report, "triangles in the upload"),
        "{:?}",
        report.violations
    );
    assert!(
        report.violations.len() <= 66,
        "{} violations",
        report.violations.len()
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}

/// The hub runs `verify` in its own process on what a worker hands back, and the release
/// profile aborts on a panic: whatever the strict reader accepts must be judged, not crash.
/// Random containers, valid by construction and otherwise as wild as the format allows.
#[test]
fn verify_judges_every_container_the_strict_reader_accepts() {
    use gm_model::format::{Vertex, texture_bytes};
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut refused = 0;
    for case in 0..200 {
        let optional = (next() as u32) & rig::ALL_BONES;
        let bone_mask = rig::REQUIRED | optional;
        let bones: Vec<u8> = (0..BONES as u8)
            .filter(|b| bone_mask & (1 << b) != 0)
            .collect();
        let vertex_count = 3 + (next() % 500) as usize;
        let vertices: Vec<Vertex> = (0..vertex_count)
            .map(|_| {
                let mut v = Vertex::default();
                for k in 0..3 {
                    v.pos[k] = ((next() % 65535) as i32 - 32767) as i16;
                    v.normal[k] = ((next() % 255) as i32 - 127) as i8;
                }
                v.uv = [next() as u16, next() as u16];
                // One to four influences that sum to 255; unused slots are joint 0, weight 0.
                let n = 1 + (next() % 4) as usize;
                let mut left = 255u32;
                for k in 0..n {
                    let w = if k == n - 1 {
                        left
                    } else {
                        1 + next() as u32 % (left - (n - 1 - k) as u32)
                    };
                    left -= w;
                    v.joints[k] = bones[next() as usize % bones.len()];
                    v.weights[k] = w as u8;
                }
                v
            })
            .collect();
        let indices: Vec<u16> = (0..3 * (1 + next() % 700))
            .map(|_| (next() % vertex_count as u64) as u16)
            .collect();
        let (tex_w, tex_h) = (64u16 << (next() % 3), 64u16 << (next() % 3));
        // Scales from a speck to the format's limit; pivots anywhere, or all on one point.
        let extent = [0.001, 1.0, 40.0, limits::MAX_EXTENT][next() as usize % 4];
        let mut pivots = [[0f32; 3]; BONES];
        if case % 5 != 0 {
            for p in pivots.iter_mut().flatten() {
                *p = ((next() % 2001) as f32 / 1000.0 - 1.0) * extent;
            }
        }
        let model = Model {
            flags: (next() % 4) as u16,
            frame: (next() % 4) as u8,
            scale: [extent, extent * 0.5, extent],
            average: [next() as u8; 4],
            bone_mask,
            pivots,
            vertices,
            indices,
            tex_w,
            tex_h,
            texture: (0..texture_bytes(tex_w, tex_h))
                .map(|_| next() as u8)
                .collect(),
        };
        let bytes = model
            .encode()
            .unwrap_or_else(|e| panic!("case {case}: {e}"));
        assert_eq!(Model::decode(&bytes).as_ref(), Ok(&model), "case {case}");
        for frame in rig::FRAMES {
            if gm_ingest::verify(&bytes, frame).is_err() {
                refused += 1;
            }
        }
    }
    // Noise is not an avatar: all of it is refused, none of it is a crash.
    assert_eq!(refused, 200 * 4);
}

// ---------- props (CONTENT.md 3.1 and 4) ----------

#[test]
fn a_flat_coloured_sword_becomes_a_prop_with_swatches_and_an_avatar_reader_refuses_it() {
    use gm_ingest::source::Fit;
    let glb = synth::sword_prop(0.9).build();
    let (report, ingested) = gm_ingest::ingest_prop(&glb, Fit::default());
    assert!(report.ok, "{:?}", report.violations);
    let ing = ingested.unwrap();
    let m = &ing.model;
    assert!(m.is_prop() && m.bone_mask == 1 && m.frame == gm_model::format::PROP_FRAME);
    assert_eq!(report.facts.triangles, 4 * 12);
    assert!(
        report.notes.iter().any(|n| n.contains("3 flat colours")),
        "{:?}",
        report.notes
    );
    // The blade reaches 0.9 m = 28.8 units along +X (glTF's +Z is the game's forward).
    let tip = (0..m.vertices.len())
        .map(|i| m.position(i).x)
        .fold(0f32, f32::max);
    assert!((tip - 28.8).abs() < 0.1, "tip at {tip}");
    assert!((report.facts.top - 28.8).abs() < 0.1);
    // Deterministic, canonical, and under the prop's file budget.
    let again = gm_ingest::ingest_prop(&glb, Fit::default()).1.unwrap();
    assert_eq!(again.gmm, ing.gmm);
    assert!(ing.gmm.len() <= limits::PROP_MAX_FILE_BYTES);
    assert_eq!(Model::decode(&ing.gmm).unwrap(), *m);
    // The strict reader takes it; the avatar verification does not, for any frame.
    for frame in rig::FRAMES {
        assert!(gm_ingest::verify(&ing.gmm, frame).is_err());
    }
    // The same file as an avatar is refused (no skin), and never a panic.
    let r = refused(&glb, STRIKER);
    assert!(has(&r, "skins"), "{:?}", r.violations);
}

#[test]
fn a_fit_moves_and_turns_a_prop_and_a_reach_past_the_hand_is_refused() {
    use gm_ingest::source::Fit;
    let glb = synth::sword_prop(0.9).build();
    // Turned a quarter about Y, the blade points along glTF +X, the game's +Y (left).
    let fit = Fit {
        mov: [0.0, 0.0, 0.0],
        turn: [0.0, 90.0, 0.0],
        scale: 1.0,
    };
    let m = gm_ingest::ingest_prop(&glb, fit).1.unwrap().model;
    let left = (0..m.vertices.len())
        .map(|i| m.position(i).y)
        .fold(0f32, f32::max);
    assert!((left - 28.8).abs() < 0.1, "left reach {left}");
    // Scaled four times, it is a pike, past the 96-unit reach.
    let fit = Fit {
        scale: 4.0,
        ..Fit::default()
    };
    let (report, ingested) = gm_ingest::ingest_prop(&glb, fit);
    assert!(!report.ok && ingested.is_none());
    assert!(has(&report, "reaches"), "{:?}", report.violations);
    // Moved, the grip is elsewhere: the whole thing slides.
    let fit = Fit {
        mov: [0.0, 0.0, -0.5],
        ..Fit::default()
    };
    let m = gm_ingest::ingest_prop(&glb, fit).1.unwrap().model;
    let tip = (0..m.vertices.len())
        .map(|i| m.position(i).x)
        .fold(0f32, f32::max);
    assert!((tip - 12.8).abs() < 0.1, "tip at {tip}");
}

#[test]
fn a_textured_prop_keeps_its_texture_and_a_mixed_one_is_refused() {
    use gm_ingest::source::Fit;
    let mut g = synth::sword_prop(0.6);
    // Every part textured by one 32 × 32 image; the uvs point at its middle.
    let n = g.positions.len();
    g.uvs = vec![[0.5, 0.5]; n];
    g.image = Some(png(32, 32, &vec![200u8; 32 * 32 * 4]));
    for m in &mut g.materials {
        m.1 = true;
    }
    let (report, ingested) = gm_ingest::ingest_prop(&g.build(), Fit::default());
    assert!(report.ok, "{:?}", report.violations);
    assert_eq!(ingested.unwrap().model.tex_w, 64);
    assert!(!report.notes.iter().any(|n| n.contains("swatches")));
    // One part textured, the others flat: the file must choose.
    g.materials[1].1 = false;
    let (report, ingested) = gm_ingest::ingest_prop(&g.build(), Fit::default());
    assert!(!report.ok && ingested.is_none());
    assert!(has(&report, "textured or flat"), "{:?}", report.violations);
}

#[test]
fn a_baked_icon_runs_the_long_axis_from_bottom_left_to_top_right_and_is_deterministic() {
    use gm_ingest::source::Fit;
    let glb = synth::sword_prop(0.9).build();
    let model = gm_ingest::ingest_prop(&glb, Fit::default())
        .1
        .unwrap()
        .model;
    let icon = gm_ingest::raster::icon(&model, 32);
    assert_eq!(icon.len(), 32 * 32 * 4);
    assert_eq!(icon, gm_ingest::raster::icon(&model, 32));
    let alpha = |x: usize, y: usize| icon[(y * 32 + x) * 4 + 3];
    // Along the diagonal (y down: the bottom left is (2, 29)) the blade is there; the
    // other corners are empty; the margin rows are empty.
    assert!(alpha(16, 15) > 0 && alpha(6, 25) > 0 && alpha(26, 5) > 0);
    assert_eq!((alpha(2, 2), alpha(29, 29)), (0, 0));
    assert!((0..32).all(|x| alpha(x, 0) == 0 && alpha(x, 31) == 0));
    // Something is lit and coloured: the steel is grey, the grip is brown.
    let covered = (0..32 * 32).filter(|i| icon[i * 4 + 3] > 0).count();
    assert!(covered > 60 && covered < 500, "{covered} dots covered");
    if let Ok(dir) = std::env::var("GM_ICON_DIR") {
        std::fs::write(format!("{dir}/sword-icon.png"), png(32, 32, &icon)).unwrap();
        let big = gm_ingest::raster::icon(&model, 128);
        std::fs::write(format!("{dir}/sword-icon-128.png"), png(128, 128, &big)).unwrap();
    }
}

#[test]
fn a_portrait_is_the_head_and_shoulders() {
    let glb = small(7, STRIKER).build();
    let model = ingest(&glb, STRIKER).1.unwrap().model;
    let p = gm_ingest::raster::portrait(&model, 32);
    assert_eq!(p.len(), 32 * 32 * 4);
    let alpha = |x: usize, y: usize| p[(y * 32 + x) * 4 + 3];
    // The head is in the upper middle; the bottom corners hold the shoulders or nothing,
    // and the top corners are empty.
    assert!(alpha(16, 10) > 0, "no head in the middle");
    assert_eq!((alpha(0, 0), alpha(31, 0)), (0, 0));
    if let Ok(dir) = std::env::var("GM_ICON_DIR") {
        std::fs::write(format!("{dir}/portrait.png"), png(32, 32, &p)).unwrap();
    }
}
