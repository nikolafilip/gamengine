//! Who is drawn as what (MODELS.md 8, 9): every body gets its frame's mannequin at once and
//! its model when the cache has it; the shared animation set poses both. Also the offline
//! crowd used to benchmark a town full of avatars.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use gm_bsp::Bsp;
use gm_core::sim::anim;
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::vocab::Status;
#[cfg(not(target_arch = "wasm32"))]
use gm_model::Model;
use gm_model::anim::{Animator, armour_weight};
use gm_model::mannequin::{self, ARMOUR_TINTS, MANNEQUIN_TEXTURE_SIZE, Shape};
use gm_model::{ModelId, Pose, rig};

use crate::cache::{CacheStats, ModelCache};
#[cfg(not(target_arch = "wasm32"))]
use crate::cache::{DirSource, ModelSource, default_cache_dir};
use crate::characters::{CharacterDraw, Characters};
use crate::render::{EntityDraw, Gpu};
use crate::{Error, Options};

/// Key of the own body among the animators.
/// The paperdoll's key: its own animation track.
pub const DOLL: u32 = u32::MAX - 1;
pub const OWN: u32 = u32::MAX;
/// Bodies not seen for this many frames lose their animation state.
const FORGET_AFTER_FRAMES: u64 = 120;
/// The lightmap is stored at half brightness (`Renderer::lightmap_scale`).
const LIGHT_SCALE: f32 = 2.0;
/// A body is never darker than this: a silhouette must stay readable in a dark corner.
const MIN_LIGHT: f32 = 0.35;
const MAX_LIGHT: f32 = 1.6;

/// Element colours of the aspect ring (MATRIX.md 5 order: fire, water, grass, electric,
/// ground, air).
pub const ASPECT_COLOURS: [[f32; 3]; 6] = [
    [1.0, 0.42, 0.10],
    [0.30, 0.55, 1.0],
    [0.35, 0.80, 0.30],
    [1.0, 0.90, 0.20],
    [0.62, 0.50, 0.34],
    [0.85, 0.95, 1.0],
];

/// Where models come from when the store does not have them: a blocking source for the
/// loader threads natively, an async one in the browser (WEB.md 4).
#[cfg(not(target_arch = "wasm32"))]
pub type Source = std::sync::Arc<dyn ModelSource>;
#[cfg(target_arch = "wasm32")]
pub type Source = std::rc::Rc<dyn crate::web::store::WebSource>;

/// One body to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    /// Entity id, or `OWN`.
    pub key: u32,
    /// Hull origin (24 u above the feet).
    pub origin: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// `gm_core::sim::anim` state.
    pub anim: u8,
    /// Crouched (MODES.md 3.5): the squat over the stance, the body `CROUCH_DROP` lower.
    pub crouched: bool,
    /// Frame, armour class and aspect mask as in the snapshot's spawn info.
    pub frame: u8,
    pub armour: u8,
    pub aspects: u8,
    /// Status mask (the aura).
    pub status: u16,
    pub model: Option<ModelId>,
    /// Distance from the camera: nearest wearers load first.
    pub distance: f32,
    /// Just hit: 1 running down to 0, drawn as a flash of light on the body (LOOK.md 13).
    pub lit: f32,
    /// The prop it holds (LOOK.md 6), as a slot of the character renderer, when the
    /// bundle has it on the GPU.
    pub prop: Option<usize>,
}

struct Track {
    animator: Animator,
    last: Vec3,
    seen: u64,
}

struct CrowdMember {
    origin: Vec3,
    yaw: f32,
    frame: u8,
    armour: u8,
    model: Option<ModelId>,
}

pub struct Avatars {
    mannequins: [usize; 4],
    tracks: HashMap<u32, Track>,
    frame: u64,
    pub cache: Option<ModelCache>,
    /// `--avatar FILE`: the own body's model, loaded from a local file.
    own_slot: Option<usize>,
    crowd: Vec<CrowdMember>,
    pinned: Option<ModelId>,
    pub draws: Vec<CharacterDraw>,
    /// Bodies drawn with their model (not the mannequin) in the last frame.
    pub with_model: usize,
    /// A prop every member of the offline crowd holds (`--crowd-prop`): the armed town
    /// of the look gate (LOOK.md 8).
    pub crowd_prop: Option<usize>,
}

/// The ambient light at a body's feet.
pub fn light_at(bsp: &Bsp, origin: Vec3) -> [f32; 3] {
    match bsp.light_point(origin, origin - Vec3::Z * 512.0) {
        Some(l) => l.map(|c| (c * LIGHT_SCALE).clamp(MIN_LIGHT, MAX_LIGHT)),
        None => [1.0; 3],
    }
}

impl Avatars {
    /// Build the mannequins, open the cache and place the crowd.
    pub fn new(
        gpu: &Gpu,
        characters: &mut Characters,
        opts: &Options,
        bsp: &Bsp,
        source: Option<Source>,
    ) -> Result<Avatars, Error> {
        let texture = mannequin::mannequin_texture();
        let mannequins = rig::FRAMES.map(|frame| {
            characters.add_mesh(
                gpu,
                &mannequin::build(frame, &Shape::MANNEQUIN),
                MANNEQUIN_TEXTURE_SIZE,
                &texture,
            )
        });
        #[cfg(not(target_arch = "wasm32"))]
        let own_slot = match &opts.avatar {
            Some(path) => {
                let bytes =
                    std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
                let model = Model::decode(&bytes)
                    .map_err(|e| format!("{} is not an ingested model: {e}", path.display()))?;
                log::info!(
                    "avatar {}: {} triangles, {}x{} atlas",
                    path.display(),
                    model.triangles(),
                    model.tex_w,
                    model.tex_h
                );
                Some(characters.add_model(gpu, &model))
            }
            None => None,
        };
        #[cfg(target_arch = "wasm32")]
        let own_slot = None;
        // The crowd's models come from a directory; online, from the hub.
        #[cfg(not(target_arch = "wasm32"))]
        let (source, crowd_ids) = match (&opts.crowd_dir, source) {
            (Some(dir), _) if opts.crowd > 0 => {
                let (src, ids) =
                    DirSource::scan(dir).map_err(|e| format!("scanning {}: {e}", dir.display()))?;
                if ids.is_empty() {
                    return Err(format!("no .gmm files in {}", dir.display()).into());
                }
                let src: Source = std::sync::Arc::new(src);
                (Some(src), ids)
            }
            (_, source) => (source, Vec::new()),
        };
        #[cfg(target_arch = "wasm32")]
        let crowd_ids: Vec<ModelId> = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        let cache = if source.is_some() {
            let dir = opts.cache_dir.clone().unwrap_or_else(default_cache_dir);
            let cache = ModelCache::new(
                &dir,
                opts.cache_mb * 1024 * 1024,
                opts.vram_mb as usize * 1024 * 1024,
                source,
            )
            .map_err(|e| format!("opening the model cache {}: {e}", dir.display()))?;
            log::info!(
                "model cache {}: {:.1} of {} MiB on disk",
                dir.display(),
                cache.loader.disk.total() as f64 / 1048576.0,
                cache.loader.disk.cap() / 1048576
            );
            Some(cache)
        } else {
            None
        };
        // The browser's store opens in the background (WEB.md 4).
        #[cfg(target_arch = "wasm32")]
        let cache = source.map(|source| {
            ModelCache::with_loader(
                crate::cache::Loader::new(opts.cache_mb * 1024 * 1024, Some(source)),
                opts.vram_mb as usize * 1024 * 1024,
            )
        });
        let crowd = place_crowd(bsp, opts.crowd, &crowd_ids);
        if !crowd.is_empty() {
            log::info!(
                "crowd: {} bodies, {} distinct models",
                crowd.len(),
                crowd_ids.len().min(crowd.len())
            );
        }
        Ok(Avatars {
            mannequins,
            tracks: HashMap::new(),
            frame: 0,
            cache,
            own_slot,
            crowd,
            pinned: None,
            draws: Vec::new(),
            with_model: 0,
            crowd_prop: None,
        })
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.draws.clear();
        self.with_model = 0;
    }

    /// Add one body: its character draw, and its markers to `boxes`.
    pub fn push(
        &mut self,
        body: &Body,
        dt: f32,
        bsp: &Bsp,
        characters: &Characters,
        boxes: &mut Vec<EntityDraw>,
    ) {
        let frame = (body.frame as usize).min(3);
        let model_slot = if body.key == OWN && self.own_slot.is_some() {
            self.own_slot
        } else {
            match (&mut self.cache, &body.model) {
                (Some(cache), Some(id)) => cache.want(id, body.distance),
                _ => None,
            }
        };
        let (slot, mut tint) = match model_slot {
            Some(slot) => {
                self.with_model += 1;
                (slot, [1.0; 3])
            }
            None => (
                self.mannequins[frame],
                ARMOUR_TINTS[(body.armour as usize).min(3)],
            ),
        };
        // Auras: a staggered or rooted body darkens, a hasted one brightens.
        let has = |s: Status| body.status & (1 << s.index()) != 0;
        if has(Status::Stagger) || has(Status::Root) {
            tint = tint.map(|c| c * 0.5);
        } else if has(Status::Haste) {
            tint = tint.map(|c| c * 1.35);
        }
        // A hit lights the body for a moment.
        if body.lit > 0.0 {
            tint = tint.map(|c| c + (2.2 - c) * body.lit.min(1.0));
        }
        let hips_z = characters.info(slot).map_or(29.0, |i| i.hips_z());
        let track = self.tracks.entry(body.key).or_insert_with(|| Track {
            animator: Animator::new(body.anim),
            last: body.origin,
            seen: 0,
        });
        // A jump of more than a few metres is a respawn or a teleport, not a stride.
        let mut moved = body.origin - track.last;
        if moved.length() > 96.0 {
            moved = Vec3::ZERO;
        }
        track.last = body.origin;
        track.seen = self.frame;
        let pose = track.animator.advance_posture(
            body.anim,
            body.crouched,
            dt,
            moved,
            body.pitch,
            hips_z,
            armour_weight(body.armour),
        );
        let feet = body.origin + Vec3::Z * Hull::Player.mins().z;
        let world = Mat4::from_translation(feet) * Mat4::from_rotation_z(body.yaw.to_radians());
        let light = light_at(bsp, body.origin);
        // What it holds, in its right hand: the prop's own draw, placed by the wearer's
        // skinning matrix of `prop_r` at that bone's pivot (LOOK.md 6.3).
        if let (Some(prop), Some(info)) = (body.prop, characters.info(slot)) {
            let skin = gm_model::skin_matrices(&info.pivots, info.mask, &pose);
            let attach = gm_model::pose::prop_attach(&info.pivots, &skin);
            self.draws.push(CharacterDraw {
                slot: prop,
                world,
                pose: Pose::default(),
                tint: [1.0; 3],
                light,
                attach: Some(attach),
            });
        }
        self.draws.push(CharacterDraw {
            slot,
            world,
            pose,
            tint,
            light,
            attach: None,
        });
        // The marks round a body (its aspects' ring, its name) are the frame's: the
        // effects and the HUD draw them (LOOK.md 13); nothing solid stands for them.
        let _ = (feet, boxes);
    }

    /// The view model (LOOK.md 6.4): the held prop drawn in view space, at the bottom
    /// right of the frame, its business end along the look, bobbing with `stride` (in
    /// strides) and kicked back by `kick` (1 at a launch, decaying to 0).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn view_model(
        &mut self,
        slot: usize,
        fit: Mat4,
        eye: Vec3,
        yaw: f32,
        pitch: f32,
        stride: f32,
        kick: f32,
        swing: f32,
        reload: f32,
        light: [f32; 3],
    ) {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let forward = Vec3::new(cp * cy, cp * sy, -sp);
        let right = Vec3::new(sy, -cy, 0.0);
        let up = right.cross(forward).normalize_or(Vec3::Z);
        // A hand's width to the right, a little under the eye, half a metre out; the bob is
        // a figure of eight a stride long; the kick pulls it back and tips it up.
        let bob = Vec3::new(
            (stride * std::f32::consts::TAU).sin() * 0.6,
            0.0,
            (stride * 2.0 * std::f32::consts::TAU).sin() * 0.4,
        );
        // A swing carries it across the view (LOOK.md 13): drawn back to the right in the
        // windup (`swing` under 0), cut across to the left (over 0).
        // (Twice as far out as the hand is, so that it takes a corner of the view and
        // not half of it: a view model is seen, the fight is looked at.)
        // The reload (MODES.md 3.2, LOOK.md 6.4): the weapon is brought down and rolled
        // over to the left to be worked, with the off hand's bob at it, and raised again
        // at the end; `reload` is how far along it is, 0 when none.
        let working = if reload > 0.0 {
            let ease = |x: f32| x * x * (3.0 - 2.0 * x);
            ease((reload / 0.22).min(1.0)) * ease(((1.0 - reload) / 0.22).min(1.0))
        } else {
            0.0
        };
        let busy = working * (reload * 9.0 * std::f32::consts::TAU).sin();
        let at = eye
            + forward * (26.0 - kick * 4.0 + swing.max(0.0) * 4.0 - working * 3.0)
            + right * (11.0 + bob.x - swing * 14.0 - working * 3.0)
            + up * (-10.0 + bob.z + swing.abs() * 2.0 - working * 4.5 + busy * 0.6);
        // The prop's +X along the look (its +Y to the left, +Z up), then, in its own
        // frame, tipped up by the kick and turned a little inward.
        let basis = Mat4::from_cols(
            forward.extend(0.0),
            (-right).extend(0.0),
            up.extend(0.0),
            at.extend(1.0),
        );
        let tip = Mat4::from_rotation_y((-(4.0 + kick * 14.0f32) + working * 24.0).to_radians());
        let inward = Mat4::from_rotation_z((8.0f32 + swing * 62.0 + working * 18.0).to_radians())
            * Mat4::from_rotation_x((swing * -35.0f32 - working * 40.0 + busy * 3.0).to_radians());
        self.draws.push(CharacterDraw {
            slot,
            world: Mat4::IDENTITY,
            pose: Pose::default(),
            tint: [1.0; 3],
            light,
            attach: Some(basis * inward * tip * fit),
        });
    }

    /// The paperdoll (LOOK.md 5): one body's draws on their own, in model space at the
    /// origin (its `yaw` turns it), lit flat, with its prop, animated like any other.
    pub fn doll(
        &mut self,
        body: &Body,
        dt: f32,
        bsp: &Bsp,
        characters: &Characters,
    ) -> Vec<CharacterDraw> {
        let kept = std::mem::take(&mut self.draws);
        let mut boxes = Vec::new();
        self.push(body, dt, bsp, characters, &mut boxes);
        let mut draws = std::mem::replace(&mut self.draws, kept);
        for d in &mut draws {
            d.light = [1.15; 3];
        }
        draws
    }

    /// The offline crowd: every member stands in place and cycles through the animations.
    pub fn push_crowd(
        &mut self,
        time: f32,
        dt: f32,
        camera: Vec3,
        bsp: &Bsp,
        characters: &Characters,
        boxes: &mut Vec<EntityDraw>,
    ) {
        const STATES: [u8; 6] = [
            anim::IDLE,
            anim::RUN,
            anim::GUARD,
            anim::CAST,
            anim::WINDUP,
            anim::SWING,
        ];
        for i in 0..self.crowd.len() {
            let (origin, yaw, frame, armour, model) = {
                let m = &self.crowd[i];
                (m.origin, m.yaw, m.frame, m.armour, m.model)
            };
            let state = STATES[(i + (time / 3.0) as usize) % STATES.len()];
            let body = Body {
                key: i as u32,
                origin,
                yaw,
                pitch: 0.0,
                anim: state,
                crouched: false,
                frame,
                armour,
                aspects: 1 << (i % 6),
                status: 0,
                model,
                distance: (origin - camera).length(),
                lit: 0.0,
                prop: self.crowd_prop,
            };
            self.push(&body, dt, bsp, characters, boxes);
            // Running on the spot: feed the stride the animator would have seen.
            if state == anim::RUN
                && let Some(t) = self.tracks.get_mut(&(i as u32))
            {
                let (sin, cos) = yaw.to_radians().sin_cos();
                t.last = origin - Vec3::new(cos, sin, 0.0) * 300.0 * dt;
            }
        }
    }

    /// Load what was asked for, upload what arrived, forget who left.
    pub fn end_frame(&mut self, gpu: &Gpu, characters: &mut Characters) {
        if let Some(cache) = &mut self.cache {
            cache.pump(gpu, characters);
        }
        let frame = self.frame;
        self.tracks
            .retain(|_, t| frame - t.seen < FORGET_AFTER_FRAMES);
    }

    /// Keep this model on disk before all others (the own avatar).
    pub fn pin(&mut self, id: &ModelId) {
        if self.pinned != Some(*id) {
            self.pinned = Some(*id);
            if let Some(cache) = &self.cache {
                #[cfg(not(target_arch = "wasm32"))]
                cache.loader.disk.pin(id);
                #[cfg(target_arch = "wasm32")]
                cache.loader.store.pin(id);
            }
        }
    }

    pub fn revoke(&mut self, id: &ModelId, characters: &mut Characters) {
        if let Some(cache) = &mut self.cache {
            cache.revoke(id, characters);
        }
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.as_ref().map(|c| c.stats).unwrap_or_default()
    }

    pub fn crowd_len(&self) -> usize {
        self.crowd.len()
    }
}

/// Rows of ten, three body widths apart, in front of the map's start, standing on whatever
/// is under them.
fn place_crowd(bsp: &Bsp, count: u32, ids: &[ModelId]) -> Vec<CrowdMember> {
    if count == 0 {
        return Vec::new();
    }
    let (start, yaw) = bsp
        .player_start()
        .unwrap_or((Vec3::new(0.0, 0.0, 64.0), 0.0));
    let (sin, cos) = yaw.to_radians().sin_cos();
    let (forward, right) = (Vec3::new(cos, sin, 0.0), Vec3::new(sin, -cos, 0.0));
    let mut out = Vec::new();
    let mut slot = 0u32;
    while out.len() < count as usize && slot < count * 8 {
        let (row, col) = (slot / 10, slot % 10);
        slot += 1;
        let p = start
            + forward * (160.0 + row as f32 * 48.0)
            + right * ((col as f32 - 4.5) * 48.0)
            + Vec3::Z * 32.0;
        // Drop the hull to the floor; skip a spot inside a wall or over a pit.
        let tr = bsp.trace(Hull::Player, p, p - Vec3::Z * 512.0);
        if tr.start_solid || tr.fraction >= 1.0 {
            continue;
        }
        let i = out.len();
        // Facing the start, so the bench camera sees faces.
        out.push(CrowdMember {
            origin: tr.end,
            yaw: (yaw + 180.0).rem_euclid(360.0),
            // The mannequin's frame; generated models cycle through the four the same way.
            frame: (i % 4) as u8,
            armour: ((i / 4) % 4) as u8,
            model: (!ids.is_empty()).then(|| ids[i % ids.len()]),
        });
    }
    out
}

/// Keys of stall keepers among the animators: clear of entity ids and of `OWN`.
const STALL_KEYS: u32 = 0x4000_0000;

/// The keeper of a stall: a body standing behind its counter (ECONOMY.md 7).
pub fn stall_keeper(stall: &gm_net::control::StallEntry, camera: Vec3) -> Body {
    let feet = Vec3::from(stall.pos);
    let origin = feet - Vec3::Z * Hull::Player.mins().z;
    Body {
        key: STALL_KEYS | (stall.id as u32 & 0x0fff_ffff),
        origin,
        yaw: stall.yaw,
        pitch: 0.0,
        anim: anim::IDLE,
        crouched: false,
        frame: stall.frame,
        armour: stall.armour,
        aspects: 0,
        status: 0,
        model: stall.model,
        distance: (origin - camera).length(),
        lit: 0.0,
        prop: None,
    }
}

/// The stall itself: a counter in front of the keeper and an awning on four posts.
pub fn stall_boxes(stall: &gm_net::control::StallEntry, boxes: &mut Vec<EntityDraw>) {
    let c = Vec3::from(stall.pos);
    // Boxes are axis-aligned: the stall faces the nearest of the four directions.
    let quarter = (stall.yaw / 90.0).round().rem_euclid(4.0) as i32;
    let (f, r) = match quarter {
        0 => (Vec3::X, Vec3::NEG_Y),
        1 => (Vec3::Y, Vec3::X),
        2 => (Vec3::NEG_X, Vec3::Y),
        _ => (Vec3::NEG_Y, Vec3::NEG_X),
    };
    let mut push = |centre: Vec3, half_f: f32, half_r: f32, z0: f32, z1: f32, color: [f32; 3]| {
        let a = centre + f * half_f + r * half_r;
        let b = centre - f * half_f - r * half_r;
        boxes.push(EntityDraw {
            mins: a.min(b) + Vec3::Z * z0,
            maxs: a.max(b) + Vec3::Z * z1,
            color: [color[0], color[1], color[2], 1.0],
        });
    };
    let wood = [0.42, 0.28, 0.16];
    // The cloth takes its colour from the stall's id, so neighbours differ.
    let cloth = ASPECT_COLOURS[(stall.id.unsigned_abs() % 6) as usize].map(|v| 0.35 + 0.55 * v);
    push(c + f * 36.0, 10.0, 46.0, 1.0, 30.0, wood);
    push(c + f * 36.0, 13.0, 49.0, 30.0, 33.0, [0.55, 0.40, 0.25]);
    for (sf, sr) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        push(
            c + f * (sf * 44.0) + r * (sr * 54.0),
            2.0,
            2.0,
            1.0,
            92.0,
            wood,
        );
    }
    push(c, 50.0, 60.0, 92.0, 96.0, cloth);
}
