//! Who is drawn as what (MODELS.md 8, 9): every body gets its frame's mannequin at once and
//! its model when the cache has it; the shared animation set poses both. Also the offline
//! crowd used to benchmark a town full of avatars.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use gm_bsp::Bsp;
use gm_core::sim::anim;
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::vocab::Status;
use gm_model::anim::{Animator, armour_weight};
use gm_model::mannequin::{self, ARMOUR_TINTS, MANNEQUIN_TEXTURE_SIZE, Shape};
use gm_model::{Model, ModelId, rig};

use crate::cache::{CacheStats, DirSource, ModelCache, ModelSource, default_cache_dir};
use crate::characters::{CharacterDraw, Characters};
use crate::render::{EntityDraw, Gpu};
use crate::{Error, Options};

/// Key of the own body among the animators.
pub const OWN: u32 = u32::MAX;
/// Bodies not seen for this many frames lose their animation state.
const FORGET_AFTER_FRAMES: u64 = 120;
/// The lightmap is stored at half brightness (`Renderer::lightmap_scale`).
const LIGHT_SCALE: f32 = 2.0;
/// A body is never darker than this: a silhouette must stay readable in a dark corner.
const MIN_LIGHT: f32 = 0.35;
const MAX_LIGHT: f32 = 1.6;

/// Element colours of the aspect ring (MATRIX.md 5 order: flame, shadow, storm, frost, stone).
pub const ASPECT_COLOURS: [[f32; 3]; 5] = [
    [1.0, 0.42, 0.10],
    [0.50, 0.22, 0.80],
    [1.0, 0.90, 0.20],
    [0.40, 0.85, 1.0],
    [0.62, 0.50, 0.34],
];

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
    /// Frame, armour class and aspect mask as in the snapshot's spawn info.
    pub frame: u8,
    pub armour: u8,
    pub aspects: u8,
    /// 0 none; otherwise drawn as a pip above the head, `friendly` or not.
    pub team: u8,
    pub friendly: bool,
    /// Status mask (the aura).
    pub status: u16,
    pub model: Option<ModelId>,
    /// Distance from the camera: nearest wearers load first.
    pub distance: f32,
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
        source: Option<Arc<dyn ModelSource>>,
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
        // The crowd's models come from a directory; online, from the hub.
        let (source, crowd_ids) = match (&opts.crowd_dir, source) {
            (Some(dir), _) if opts.crowd > 0 => {
                let (src, ids) =
                    DirSource::scan(dir).map_err(|e| format!("scanning {}: {e}", dir.display()))?;
                if ids.is_empty() {
                    return Err(format!("no .gmm files in {}", dir.display()).into());
                }
                let src: Arc<dyn ModelSource> = Arc::new(src);
                (Some(src), ids)
            }
            (_, source) => (source, Vec::new()),
        };
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
                cache.disk.total() as f64 / 1048576.0,
                cache.disk.cap() / 1048576
            );
            Some(cache)
        } else {
            None
        };
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
        let pose = track.animator.advance(
            body.anim,
            dt,
            moved,
            body.pitch,
            hips_z,
            armour_weight(body.armour),
        );
        let feet = body.origin + Vec3::Z * Hull::Player.mins().z;
        self.draws.push(CharacterDraw {
            slot,
            world: Mat4::from_translation(feet) * Mat4::from_rotation_z(body.yaw.to_radians()),
            pose,
            tint,
            light: light_at(bsp, body.origin),
        });
        if body.anim != anim::DEAD {
            markers(body, feet, boxes);
        }
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
                frame,
                armour,
                aspects: 1 << (i % 5),
                team: 1 + (i % 2) as u8,
                friendly: i % 2 == 0,
                status: 0,
                model,
                distance: (origin - camera).length(),
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
                cache.disk.pin(id);
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

/// The marks the client draws around every body whatever it wears (MODELS.md 9): the aspect
/// ring at the feet and the team pip above the head.
fn markers(body: &Body, feet: Vec3, boxes: &mut Vec<EntityDraw>) {
    let aspects: Vec<usize> = (0..5).filter(|i| body.aspects & (1 << i) != 0).collect();
    let r = 9.0;
    for (k, a) in aspects.iter().take(2).enumerate() {
        let c = ASPECT_COLOURS[*a];
        // One aspect: the whole plate; two: a half each.
        let (x0, x1) = match (aspects.len().min(2), k) {
            (1, _) => (-r, r),
            (_, 0) => (-r, 0.0),
            _ => (0.0, r),
        };
        boxes.push(EntityDraw {
            mins: feet + Vec3::new(x0, -r, 0.2),
            maxs: feet + Vec3::new(x1, r, 0.9),
            color: [c[0] * 0.8, c[1] * 0.8, c[2] * 0.8, 1.0],
        });
    }
    if body.team != 0 {
        let top = feet + Vec3::Z * 66.0;
        boxes.push(EntityDraw {
            mins: top - Vec3::splat(2.5),
            maxs: top + Vec3::splat(2.5),
            color: if body.friendly {
                [0.25, 0.45, 1.0, 1.0]
            } else {
                [1.0, 0.3, 0.2, 1.0]
            },
        });
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
        frame: stall.frame,
        armour: stall.armour,
        aspects: 0,
        team: 0,
        friendly: true,
        status: 0,
        model: stall.model,
        distance: (origin - camera).length(),
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
    let cloth = ASPECT_COLOURS[(stall.id.unsigned_abs() % 5) as usize].map(|v| 0.35 + 0.55 * v);
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
