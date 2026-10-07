//! A content pack built in code for tests, benchmarks and bots that run without the asset
//! files. The shipped content lives in `assets/content` (loaded by `gm-content`); this pack
//! mirrors its v1 abilities and the four preset builds of MATRIX.md 11 closely enough that
//! simulation tests mean something, but it is a fixture, not the source of truth.

use crate::build::{AbilityDef, Build, ContentPack, CreatureDef, Loot, NamedBuild, Sheet, Slot};
use crate::matrix::{ArmourClass, Aspects, Attributes, Element};
use crate::tick::TickRate;
use crate::trial::{Lens, TrialDef};
use crate::vocab::{
    Ability, AbilityId, ApplyStatus, ArchetypeFrame, AreaEffect, Block, Bounce, Bypass, Chain,
    Cone, Cooldown, Cost, DamagePacket, DamageType, Falloff, FireMode, Firearm, Interrupt,
    MeleeArc, Mode, MoveKind, MoveSelf, Origin, Parry, Projectile, Riposte, Shape, StackRule,
    Status, StatusTarget, Step, Timing, Trigger, Verb,
};

pub fn packet(amount: u16, dtype: DamageType, knockback: f32, stagger: u8) -> DamagePacket {
    DamagePacket {
        amount,
        dtype,
        bypass: Bypass::NONE,
        knockback,
        stagger,
    }
}

pub fn status(
    status: Status,
    ms: u32,
    magnitude: f32,
    stacking: StackRule,
    max_stacks: u8,
    target: StatusTarget,
    rate: TickRate,
) -> ApplyStatus {
    ApplyStatus {
        status,
        duration: rate.ms_to_ticks(ms),
        magnitude,
        max_stacks,
        stacking,
        target,
        dispellable: true,
    }
}

#[allow(clippy::too_many_arguments)]
fn melee(
    rate: TickRate,
    reach: f32,
    arc_deg: f32,
    windup_ms: u32,
    active_ms: u32,
    recovery_ms: u32,
    damage: DamagePacket,
    max_targets: u8,
    cleave: f32,
) -> Verb {
    Verb::MeleeArc(MeleeArc {
        reach,
        arc_deg,
        half_height: 40.0,
        timing: Timing {
            windup: rate.ms_to_ticks(windup_ms),
            active: rate.ms_to_ticks(active_ms),
            recovery: rate.ms_to_ticks(recovery_ms),
        },
        damage,
        max_targets,
        cleave_falloff: cleave,
        parryable: true,
        hit_stop: rate.ms_to_ticks(30),
        assist_deg: 0.0,
    })
}

/// An arc with the magnet (MODES.md 4.2).
fn assisted(verb: Verb, deg: f32) -> Verb {
    match verb {
        Verb::MeleeArc(m) => Verb::MeleeArc(MeleeArc {
            assist_deg: deg,
            ..m
        }),
        v => v,
    }
}

/// A bullet (MODES.md 3.6): 20,000 u/s, flat, gone within 400 ms, no spread of its own.
fn bullet(rate: TickRate, damage: DamagePacket, offset: [f32; 3]) -> Verb {
    Verb::Projectile(Projectile {
        speed: 20000.0,
        gravity_scale: 0.02,
        radius: 2.0,
        lifetime: rate.ms_to_ticks(400),
        damage,
        pierce: 0,
        bounce: Bounce::default(),
        drag: 0.0,
        spawn: Origin::Weapon { offset },
        inherit_velocity: 0.0,
        spread_deg: 0.0,
        count: 1,
        on_hit: vec![],
        on_expire: vec![],
    })
}

/// A firearm's numbers (MODES.md 3.2) on an ability; its cooldown becomes the cycle.
#[allow(clippy::too_many_arguments)]
fn firearm(
    rate: TickRate,
    mut ability: Ability,
    magazine: u8,
    ammo: &str,
    reload_ms: u32,
    cycle_ms: u32,
    fire: FireMode,
    recoil: &[(f32, f32)],
    cone: [f32; 5],
    recover_ms: u32,
    scope: u8,
) -> Ability {
    ability.cooldown.ticks = rate.ms_to_ticks(cycle_ms);
    ability.firearm = Some(Firearm {
        magazine,
        ammo: ammo.to_string(),
        reload: rate.ms_to_ticks(reload_ms),
        cycle: rate.ms_to_ticks(cycle_ms),
        fire,
        headshot: 4.0,
        scope,
        recoil: recoil.to_vec(),
        cone: Cone {
            stand: cone[0],
            crouch: cone[1],
            moving: cone[2],
            air: cone[3],
            shot: cone[4],
            recover: rate.ms_to_ticks(recover_ms),
        },
    });
    ability
}

fn projectile(
    rate: TickRate,
    speed: f32,
    gravity: f32,
    radius: f32,
    damage: DamagePacket,
    on_hit: Vec<Trigger>,
) -> Verb {
    Verb::Projectile(Projectile {
        speed,
        gravity_scale: gravity,
        radius,
        lifetime: rate.ms_to_ticks(3000),
        damage,
        pierce: 0,
        bounce: Bounce::default(),
        drag: 0.0,
        spawn: Origin::Weapon {
            offset: [16.0, 4.0, -2.0],
        },
        inherit_velocity: 0.0,
        spread_deg: 0.3,
        count: 1,
        on_hit,
        on_expire: vec![],
    })
}

#[allow(clippy::too_many_arguments)]
fn ability(
    id: u16,
    name: &str,
    cooldown_ms: u32,
    stamina: u16,
    focus: u16,
    move_scale: f32,
    interrupt: Interrupt,
    steps: Vec<(u32, Verb)>,
    rate: TickRate,
) -> Ability {
    Ability {
        id: AbilityId(id),
        name: name.into(),
        cost: Cost { stamina, focus },
        cooldown: Cooldown {
            ticks: rate.ms_to_ticks(cooldown_ms),
            group: None,
        },
        steps: steps
            .into_iter()
            .map(|(ms, verb)| Step {
                at: rate.ms_to_ticks(ms),
                verb,
            })
            .collect(),
        move_scale,
        interrupt,
        chain: None,
        firearm: None,
        range: 0.0,
    }
}

fn def(key: &str, slot: Slot, cost: u8, aspect: Option<Element>, ability: Ability) -> AbilityDef {
    AbilityDef {
        key: key.into(),
        ability,
        slot,
        cost,
        aspect,
        squad: 0,
        creature: false,
    }
}

/// The fixture pack.
pub fn pack(rate: TickRate) -> ContentPack {
    use DamageType::*;
    use Slot::*;
    let r = rate;
    let abilities = vec![
        // Primaries: the Phase 2 sword keeps its numbers (PROTOCOL.md 9 tests depend on them).
        def(
            "sword",
            Primary,
            2,
            None,
            ability(
                1,
                "Sword",
                600,
                0,
                0,
                0.6,
                Interrupt::OnStagger,
                vec![(
                    0,
                    assisted(
                        melee(
                            r,
                            72.0,
                            90.0,
                            150,
                            60,
                            300,
                            packet(60, Slash, 150.0, 20),
                            3,
                            0.7,
                        ),
                        30.0,
                    ),
                )],
                r,
            ),
        ),
        def(
            "hammer",
            Primary,
            2,
            None,
            ability(
                2,
                "Hammer",
                1000,
                0,
                0,
                0.5,
                Interrupt::OnStagger,
                vec![(
                    0,
                    melee(
                        r,
                        76.0,
                        80.0,
                        300,
                        80,
                        500,
                        DamagePacket {
                            bypass: Bypass::MAGIC_SHIELD,
                            ..packet(90, Blunt, 220.0, 35)
                        },
                        3,
                        0.7,
                    ),
                )],
                r,
            ),
        ),
        def(
            "staff",
            Primary,
            0,
            None,
            ability(
                3,
                "Staff",
                600,
                0,
                0,
                0.7,
                Interrupt::OnStagger,
                vec![(
                    0,
                    melee(
                        r,
                        64.0,
                        70.0,
                        150,
                        60,
                        300,
                        packet(40, Blunt, 120.0, 15),
                        2,
                        0.6,
                    ),
                )],
                r,
            ),
        ),
        def(
            "dagger",
            Primary,
            0,
            None,
            ability(
                4,
                "Dagger",
                400,
                0,
                0,
                0.8,
                Interrupt::OnStagger,
                vec![(
                    0,
                    assisted(
                        melee(
                            r,
                            56.0,
                            70.0,
                            100,
                            40,
                            200,
                            packet(35, Slash, 60.0, 10),
                            1,
                            1.0,
                        ),
                        30.0,
                    ),
                )],
                r,
            ),
        ),
        // The ranged primaries (secondaries until 2026-10-06: the range is the weapon's).
        def(
            "crossbow",
            Primary,
            2,
            None,
            ability(
                10,
                "Crossbow",
                1800,
                0,
                0,
                0.7,
                Interrupt::Never,
                vec![(
                    125,
                    projectile(r, 1800.0, 0.3, 2.0, packet(80, Pierce, 100.0, 10), vec![]),
                )],
                r,
            ),
        ),
        def(
            "firebolt",
            Primary,
            2,
            Some(Element::Flame),
            ability(
                11,
                "Firebolt",
                1200,
                0,
                10,
                0.7,
                Interrupt::OnDamage,
                vec![(
                    200,
                    projectile(
                        r,
                        1400.0,
                        0.1,
                        4.0,
                        packet(55, Flame, 60.0, 10),
                        vec![Trigger::Status(status(
                            Status::Burn,
                            3000,
                            12.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Hit,
                            r,
                        ))],
                    ),
                )],
                r,
            ),
        ),
        def(
            "ice_shard",
            Primary,
            2,
            Some(Element::Frost),
            ability(
                12,
                "Ice shard",
                1000,
                0,
                8,
                0.7,
                Interrupt::OnDamage,
                vec![(
                    150,
                    projectile(
                        r,
                        1500.0,
                        0.15,
                        4.0,
                        packet(50, Frost, 60.0, 10),
                        vec![Trigger::Status(status(
                            Status::Chill,
                            3000,
                            0.0,
                            StackRule::Extend,
                            3,
                            StatusTarget::Hit,
                            r,
                        ))],
                    ),
                )],
                r,
            ),
        ),
        def(
            "stone_throw",
            Primary,
            2,
            Some(Element::Stone),
            ability(
                13,
                "Stone throw",
                1800,
                0,
                8,
                0.6,
                Interrupt::OnDamage,
                vec![(
                    250,
                    projectile(r, 1100.0, 0.6, 6.0, packet(75, Stone, 250.0, 30), vec![]),
                )],
                r,
            ),
        ),
        def(
            "shadow_dart",
            Primary,
            2,
            Some(Element::Shadow),
            ability(
                14,
                "Shadow dart",
                800,
                0,
                6,
                0.8,
                Interrupt::Never,
                vec![(
                    100,
                    projectile(
                        r,
                        2000.0,
                        0.05,
                        2.0,
                        DamagePacket {
                            bypass: Bypass::EVASION,
                            ..packet(40, Shadow, 40.0, 5)
                        },
                        vec![],
                    ),
                )],
                r,
            ),
        ),
        def(
            "spark",
            Primary,
            2,
            Some(Element::Storm),
            ability(
                15,
                "Spark",
                1200,
                0,
                10,
                0.7,
                Interrupt::OnDamage,
                vec![(
                    150,
                    projectile(
                        r,
                        2400.0,
                        0.0,
                        3.0,
                        packet(50, Storm, 40.0, 10),
                        vec![Trigger::Status(status(
                            Status::Shock,
                            16,
                            1.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Hit,
                            r,
                        ))],
                    ),
                )],
                r,
            ),
        ),
        // Guards.
        def(
            "shield_wall",
            Guard,
            6,
            None,
            ability(
                20,
                "Shield wall",
                0,
                0,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::Guard(crate::vocab::Guard::Block(Block {
                        arc_deg: 150.0,
                        mitigation: 0.8,
                        stamina_per_hit: 12,
                        stops_projectiles: true,
                        move_speed_scale: 0.5,
                    })),
                )],
                r,
            ),
        ),
        def(
            "parry",
            Guard,
            4,
            None,
            ability(
                21,
                "Parry",
                1000,
                10,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::Guard(crate::vocab::Guard::Parry(Parry {
                        arc_deg: 90.0,
                        window: r.ms_to_ticks(150),
                        whiff_recovery: r.ms_to_ticks(400),
                        on_success: vec![
                            Riposte::Status(status(
                                Status::Stagger,
                                600,
                                1.0,
                                StackRule::Refresh,
                                1,
                                StatusTarget::Hit,
                                r,
                            )),
                            Riposte::Swing(MeleeArc {
                                reach: 72.0,
                                arc_deg: 90.0,
                                half_height: 40.0,
                                timing: Timing {
                                    windup: 0,
                                    active: r.ms_to_ticks(45),
                                    recovery: 0,
                                },
                                damage: packet(40, Slash, 120.0, 0),
                                max_targets: 1,
                                cleave_falloff: 1.0,
                                parryable: false,
                                hit_stop: 0,
                                assist_deg: 0.0,
                            }),
                        ],
                    })),
                )],
                r,
            ),
        ),
        def(
            "brace",
            Guard,
            2,
            None,
            ability(
                22,
                "Brace",
                0,
                0,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::Guard(crate::vocab::Guard::Block(Block {
                        arc_deg: 120.0,
                        mitigation: 0.5,
                        stamina_per_hit: 10,
                        stops_projectiles: false,
                        move_speed_scale: 0.7,
                    })),
                )],
                r,
            ),
        ),
        // Actives.
        def(
            "dash",
            Active,
            6,
            None,
            ability(
                30,
                "Dash",
                1000,
                30,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    // The dodge (MODES.md 4.4): untouchable for 150 ms, cuts a recovery.
                    Verb::MoveSelf(MoveSelf {
                        kind: MoveKind::Dash {
                            speed: 900.0,
                            duration: r.ms_to_ticks(150),
                        },
                        cancelable: false,
                        keep_friction: false,
                        iframes: r.ms_to_ticks(150),
                        cancel_recovery: true,
                    }),
                )],
                r,
            ),
        ),
        def(
            "leap",
            Active,
            6,
            None,
            ability(
                31,
                "Leap",
                1500,
                25,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::MoveSelf(MoveSelf {
                        kind: MoveKind::Leap {
                            forward: 400.0,
                            up: 320.0,
                        },
                        cancelable: false,
                        keep_friction: false,
                        iframes: 0,
                        cancel_recovery: false,
                    }),
                )],
                r,
            ),
        ),
        def(
            "charge",
            Active,
            6,
            None,
            ability(
                32,
                "Charge",
                4000,
                30,
                0,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::MoveSelf(MoveSelf {
                        kind: MoveKind::Charge {
                            speed: 700.0,
                            duration: r.ms_to_ticks(400),
                            stop_on_hit: true,
                        },
                        cancelable: false,
                        keep_friction: false,
                        iframes: 0,
                        cancel_recovery: false,
                    }),
                )],
                r,
            ),
        ),
        def(
            "blink",
            Active,
            10,
            Some(Element::Shadow),
            ability(
                33,
                "Blink",
                6000,
                0,
                30,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::MoveSelf(MoveSelf {
                        kind: MoveKind::Blink { distance: 256.0 },
                        cancelable: false,
                        keep_friction: false,
                        iframes: 0,
                        cancel_recovery: false,
                    }),
                )],
                r,
            ),
        ),
        def(
            "overhead",
            Active,
            8,
            None,
            ability(
                34,
                "Overhead",
                3000,
                20,
                0,
                0.4,
                Interrupt::OnStagger,
                vec![(
                    0,
                    melee(
                        r,
                        80.0,
                        60.0,
                        350,
                        60,
                        500,
                        packet(70, Slash, 200.0, 40),
                        3,
                        0.7,
                    ),
                )],
                r,
            ),
        ),
        def(
            "stomp",
            Active,
            10,
            Some(Element::Stone),
            ability(
                35,
                "Stomp",
                8000,
                0,
                25,
                0.3,
                Interrupt::OnStagger,
                vec![(
                    250,
                    Verb::AreaEffect(AreaEffect {
                        shape: Shape::Cylinder {
                            radius: 128.0,
                            height: 96.0,
                        },
                        origin: Origin::SelfFeet,
                        delay: 0,
                        duration: 0,
                        interval: 0,
                        damage: Some(packet(35, Stone, 200.0, 40)),
                        effects: vec![status(
                            Status::Slow,
                            2000,
                            0.3,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Area,
                            r,
                        )],
                        falloff: Falloff::None,
                        max_targets: 8,
                        requires_los: true,
                        exclude_actor: true,
                    }),
                )],
                r,
            ),
        ),
        def(
            "fortify",
            Active,
            8,
            Some(Element::Stone),
            ability(
                36,
                "Fortify",
                15000,
                0,
                30,
                0.8,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::ApplyStatus(status(
                        Status::Fortify,
                        5000,
                        0.4,
                        StackRule::Refresh,
                        1,
                        StatusTarget::Actor,
                        r,
                    )),
                )],
                r,
            ),
        ),
        def(
            "frost_nova",
            Active,
            10,
            Some(Element::Frost),
            ability(
                37,
                "Frost nova",
                8000,
                0,
                30,
                0.3,
                Interrupt::OnDamage,
                vec![(
                    300,
                    Verb::AreaEffect(AreaEffect {
                        shape: Shape::Sphere { radius: 160.0 },
                        origin: Origin::SelfFeet,
                        delay: 0,
                        duration: 0,
                        interval: 0,
                        damage: Some(packet(30, Frost, 60.0, 10)),
                        effects: vec![
                            status(
                                Status::Chill,
                                4000,
                                0.0,
                                StackRule::Extend,
                                3,
                                StatusTarget::Area,
                                r,
                            ),
                            status(
                                Status::Chill,
                                4000,
                                0.0,
                                StackRule::Extend,
                                3,
                                StatusTarget::Area,
                                r,
                            ),
                        ],
                        falloff: Falloff::None,
                        max_targets: 8,
                        requires_los: true,
                        exclude_actor: true,
                    }),
                )],
                r,
            ),
        ),
        def(
            "haste",
            Active,
            8,
            None,
            ability(
                38,
                "Haste",
                12000,
                0,
                20,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::ApplyStatus(status(
                        Status::Haste,
                        5000,
                        0.25,
                        StackRule::Refresh,
                        1,
                        StatusTarget::Actor,
                        r,
                    )),
                )],
                r,
            ),
        ),
        def(
            "thunderclap",
            Active,
            10,
            Some(Element::Storm),
            ability(
                39,
                "Thunderclap",
                7000,
                0,
                30,
                0.3,
                Interrupt::OnDamage,
                vec![(
                    250,
                    Verb::AreaEffect(AreaEffect {
                        shape: Shape::Sphere { radius: 140.0 },
                        origin: Origin::SelfFeet,
                        delay: 0,
                        duration: 0,
                        interval: 0,
                        damage: Some(packet(40, Storm, 80.0, 20)),
                        effects: vec![status(
                            Status::Shock,
                            16,
                            1.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Area,
                            r,
                        )],
                        falloff: Falloff::Linear,
                        max_targets: 8,
                        requires_los: true,
                        exclude_actor: true,
                    }),
                )],
                r,
            ),
        ),
        def(
            "vanish",
            Active,
            8,
            Some(Element::Shadow),
            ability(
                40,
                "Vanish",
                14000,
                0,
                25,
                1.0,
                Interrupt::Never,
                vec![(
                    0,
                    Verb::ApplyStatus(status(
                        Status::Stealth,
                        6000,
                        256.0,
                        StackRule::Refresh,
                        1,
                        StatusTarget::Actor,
                        r,
                    )),
                )],
                r,
            ),
        ),
        def(
            "poison_cloud",
            Active,
            8,
            Some(Element::Shadow),
            ability(
                41,
                "Poison cloud",
                10000,
                0,
                25,
                0.6,
                Interrupt::Never,
                vec![(
                    200,
                    Verb::AreaEffect(AreaEffect {
                        shape: Shape::Cylinder {
                            radius: 96.0,
                            height: 80.0,
                        },
                        origin: Origin::Weapon {
                            offset: [96.0, 0.0, -40.0],
                        },
                        delay: 0,
                        duration: r.ms_to_ticks(5000),
                        interval: r.ms_to_ticks(1000),
                        damage: None,
                        effects: vec![status(
                            Status::Bleed,
                            3000,
                            6.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Area,
                            r,
                        )],
                        falloff: Falloff::None,
                        max_targets: 8,
                        requires_los: false,
                        exclude_actor: false,
                    }),
                )],
                r,
            ),
        ),
        def(
            "fireball",
            Active,
            12,
            Some(Element::Flame),
            ability(
                42,
                "Fireball",
                5000,
                0,
                35,
                0.5,
                Interrupt::OnDamage,
                vec![(
                    400,
                    Verb::Projectile(Projectile {
                        speed: 900.0,
                        gravity_scale: 0.0,
                        radius: 6.0,
                        lifetime: r.ms_to_ticks(3000),
                        damage: packet(20, Flame, 40.0, 10),
                        pierce: 0,
                        bounce: Bounce::default(),
                        drag: 0.0,
                        spawn: Origin::Weapon {
                            offset: [16.0, 4.0, -2.0],
                        },
                        inherit_velocity: 0.0,
                        spread_deg: 0.0,
                        count: 1,
                        on_hit: vec![Trigger::Area(AreaEffect {
                            shape: Shape::Sphere { radius: 96.0 },
                            origin: Origin::Impact,
                            delay: 0,
                            duration: 0,
                            interval: 0,
                            damage: Some(packet(30, Flame, 120.0, 15)),
                            effects: vec![status(
                                Status::Burn,
                                3000,
                                6.0,
                                StackRule::Refresh,
                                1,
                                StatusTarget::Area,
                                r,
                            )],
                            falloff: Falloff::Linear,
                            max_targets: 8,
                            requires_los: true,
                            exclude_actor: false,
                        })],
                        on_expire: vec![],
                    }),
                )],
                r,
            ),
        ),
        // ---------- Phase 7 (COMPANIONS.md 12) ----------
        def(
            "mend",
            Secondary,
            4,
            None,
            ability(
                43,
                "Mend",
                2000,
                0,
                12,
                0.7,
                Interrupt::OnDamage,
                vec![(
                    150,
                    Verb::Projectile(Projectile {
                        speed: 1600.0,
                        gravity_scale: 0.1,
                        radius: 6.0,
                        lifetime: r.ms_to_ticks(3000),
                        // Amount 0: not an attack, only the carrier of its Regen.
                        damage: packet(0, Blunt, 0.0, 0),
                        pierce: 0,
                        bounce: Bounce::default(),
                        drag: 0.0,
                        spawn: Origin::Weapon {
                            offset: [16.0, 4.0, -2.0],
                        },
                        inherit_velocity: 0.0,
                        spread_deg: 0.3,
                        count: 1,
                        on_hit: vec![Trigger::Status(status(
                            Status::Regen,
                            3000,
                            100.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Hit,
                            r,
                        ))],
                        on_expire: vec![],
                    }),
                )],
                r,
            ),
        ),
        def(
            "sanctuary",
            Active,
            10,
            None,
            ability(
                44,
                "Sanctuary",
                14000,
                0,
                35,
                0.5,
                Interrupt::OnDamage,
                vec![(
                    300,
                    Verb::AreaEffect(AreaEffect {
                        shape: Shape::Cylinder {
                            radius: 140.0,
                            height: 96.0,
                        },
                        origin: Origin::Aim { range: 500.0 },
                        delay: 0,
                        duration: r.ms_to_ticks(6000),
                        interval: r.ms_to_ticks(1000),
                        damage: None,
                        effects: vec![status(
                            Status::Regen,
                            1500,
                            60.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Area,
                            r,
                        )],
                        falloff: Falloff::None,
                        max_targets: 8,
                        requires_los: false,
                        exclude_actor: false,
                    }),
                )],
                r,
            ),
        ),
        AbilityDef {
            squad: 2,
            ..def(
                "war_standard",
                Active,
                10,
                None,
                ability(
                    45,
                    "War standard",
                    20000,
                    20,
                    0,
                    0.8,
                    Interrupt::Never,
                    vec![(
                        200,
                        Verb::AreaEffect(AreaEffect {
                            shape: Shape::Cylinder {
                                radius: 256.0,
                                height: 96.0,
                            },
                            origin: Origin::SelfFeet,
                            delay: 0,
                            duration: 0,
                            interval: 0,
                            damage: None,
                            effects: vec![status(
                                Status::Fortify,
                                8000,
                                0.15,
                                StackRule::Refresh,
                                1,
                                StatusTarget::Area,
                                r,
                            )],
                            falloff: Falloff::None,
                            max_targets: 8,
                            requires_los: true,
                            exclude_actor: false,
                        }),
                    )],
                    r,
                ),
            )
        },
        // ---------- creature abilities (COMPANIONS.md 8.1) ----------
        AbilityDef {
            creature: true,
            ..def(
                "maul",
                Primary,
                0,
                None,
                ability(
                    46,
                    "Maul",
                    2000,
                    0,
                    0,
                    0.4,
                    Interrupt::OnStagger,
                    vec![(
                        0,
                        Verb::MeleeArc(MeleeArc {
                            reach: 96.0,
                            arc_deg: 120.0,
                            half_height: 48.0,
                            timing: Timing {
                                windup: r.ms_to_ticks(550),
                                active: r.ms_to_ticks(80),
                                recovery: r.ms_to_ticks(600),
                            },
                            damage: DamagePacket {
                                bypass: Bypass::MAGIC_SHIELD,
                                ..packet(55, Blunt, 260.0, 45)
                            },
                            max_targets: 4,
                            cleave_falloff: 1.0,
                            parryable: true,
                            hit_stop: r.ms_to_ticks(40),
                            assist_deg: 0.0,
                        }),
                    )],
                    r,
                ),
            )
        },
        AbilityDef {
            creature: true,
            ..def(
                "quake",
                Active,
                0,
                Some(Element::Stone),
                ability(
                    47,
                    "Quake",
                    9000,
                    0,
                    0,
                    0.2,
                    Interrupt::OnStagger,
                    vec![(
                        300,
                        Verb::AreaEffect(AreaEffect {
                            shape: Shape::Cylinder {
                                radius: 150.0,
                                height: 96.0,
                            },
                            origin: Origin::Aim { range: 700.0 },
                            delay: r.ms_to_ticks(1300),
                            duration: 0,
                            interval: 0,
                            damage: Some(packet(70, Stone, 300.0, 60)),
                            effects: vec![],
                            falloff: Falloff::None,
                            max_targets: 8,
                            requires_los: false,
                            exclude_actor: true,
                        }),
                    )],
                    r,
                ),
            )
        },
        // Phase 14: the gun (CONTENT.md 11), the crossbow's counterpart: slower, harder,
        // flatter, from a little further ahead of the hand.
        def(
            "musket",
            Primary,
            2,
            None,
            firearm(
                r,
                ability(
                    48,
                    "Musket",
                    1200,
                    0,
                    0,
                    0.5,
                    Interrupt::Never,
                    vec![(
                        0,
                        bullet(r, packet(110, Pierce, 160.0, 15), [20.0, 4.0, -2.0]),
                    )],
                    r,
                ),
                1,
                "ball",
                2800,
                1200,
                FireMode::Bolt,
                &[(0.0, 2.4)],
                [0.6, 0.3, 3.0, 8.0, 1.2],
                400,
                2,
            ),
        ),
        // 2026-10-06: the secondaries are short utilities; no bolt for everybody.
        def(
            "kick",
            Secondary,
            2,
            None,
            ability(
                49,
                "Kick",
                1500,
                0,
                0,
                0.6,
                Interrupt::OnStagger,
                vec![(
                    0,
                    melee(
                        r,
                        64.0,
                        60.0,
                        120,
                        50,
                        250,
                        packet(25, Blunt, 320.0, 40),
                        1,
                        1.0,
                    ),
                )],
                r,
            ),
        ),
        def(
            "shield_bash",
            Secondary,
            2,
            None,
            ability(
                50,
                "Shield bash",
                4000,
                0,
                0,
                0.5,
                Interrupt::OnStagger,
                vec![(
                    0,
                    match melee(
                        r,
                        60.0,
                        60.0,
                        150,
                        50,
                        300,
                        packet(35, Blunt, 200.0, 70),
                        1,
                        1.0,
                    ) {
                        Verb::MeleeArc(m) => Verb::MeleeArc(MeleeArc {
                            parryable: false,
                            ..m
                        }),
                        v => v,
                    },
                )],
                r,
            ),
        ),
        def(
            "throwing_knife",
            Secondary,
            3,
            None,
            ability(
                51,
                "Throwing knife",
                2000,
                0,
                0,
                0.8,
                Interrupt::Never,
                vec![(
                    100,
                    // 1200 u/s for 250 ms: 300 units and gone.
                    match projectile(r, 1200.0, 0.2, 2.0, packet(40, Pierce, 60.0, 10), vec![]) {
                        Verb::Projectile(p) => Verb::Projectile(Projectile {
                            lifetime: r.ms_to_ticks(250),
                            ..p
                        }),
                        v => v,
                    },
                )],
                r,
            ),
        ),
        // The three modes (MODES.md, 2026-10-07): the stages of two chains, the knife of a
        // gun build, a pistol and a carbine. Appended: builds name abilities by position.
        def(
            "sword_2",
            Extra,
            0,
            None,
            ability(
                36,
                "Sword II",
                600,
                0,
                0,
                0.6,
                Interrupt::OnStagger,
                vec![(
                    0,
                    assisted(
                        melee(
                            r,
                            76.0,
                            110.0,
                            120,
                            60,
                            280,
                            packet(65, Slash, 60.0, 25),
                            3,
                            0.7,
                        ),
                        30.0,
                    ),
                )],
                r,
            ),
        ),
        def(
            "sword_3",
            Extra,
            0,
            None,
            ability(
                37,
                "Sword III",
                900,
                0,
                0,
                0.4,
                Interrupt::OnStagger,
                vec![
                    (
                        0,
                        match assisted(
                            melee(
                                r,
                                80.0,
                                70.0,
                                260,
                                70,
                                450,
                                packet(85, Slash, 220.0, 40),
                                2,
                                0.7,
                            ),
                            30.0,
                        ) {
                            Verb::MeleeArc(m) => Verb::MeleeArc(MeleeArc {
                                half_height: 44.0,
                                hit_stop: r.ms_to_ticks(40),
                                ..m
                            }),
                            v => v,
                        },
                    ),
                    (
                        260,
                        Verb::ApplyStatus(status(
                            Status::Knockdown,
                            1200,
                            1.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Hit,
                            r,
                        )),
                    ),
                ],
                r,
            ),
        ),
        def(
            "dagger_2",
            Extra,
            0,
            None,
            ability(
                38,
                "Dagger II",
                400,
                0,
                0,
                0.8,
                Interrupt::OnStagger,
                vec![(
                    0,
                    assisted(
                        melee(
                            r,
                            58.0,
                            80.0,
                            80,
                            40,
                            180,
                            packet(38, Slash, 70.0, 12),
                            1,
                            1.0,
                        ),
                        30.0,
                    ),
                )],
                r,
            ),
        ),
        def(
            "dagger_3",
            Extra,
            0,
            None,
            ability(
                39,
                "Dagger III",
                800,
                0,
                0,
                0.6,
                Interrupt::OnStagger,
                vec![
                    (
                        0,
                        match assisted(
                            melee(
                                r,
                                60.0,
                                60.0,
                                180,
                                50,
                                360,
                                packet(55, Pierce, 120.0, 30),
                                1,
                                1.0,
                            ),
                            30.0,
                        ) {
                            Verb::MeleeArc(m) => Verb::MeleeArc(MeleeArc {
                                half_height: 44.0,
                                hit_stop: r.ms_to_ticks(40),
                                ..m
                            }),
                            v => v,
                        },
                    ),
                    (
                        180,
                        Verb::ApplyStatus(status(
                            Status::Launched,
                            900,
                            300.0,
                            StackRule::Refresh,
                            1,
                            StatusTarget::Hit,
                            r,
                        )),
                    ),
                ],
                r,
            ),
        ),
        def(
            "knife",
            Extra,
            0,
            None,
            ability(
                40,
                "Knife",
                400,
                0,
                0,
                1.0,
                Interrupt::OnStagger,
                vec![(
                    0,
                    melee(
                        r,
                        56.0,
                        70.0,
                        100,
                        40,
                        200,
                        packet(40, Slash, 60.0, 10),
                        1,
                        1.0,
                    ),
                )],
                r,
            ),
        ),
        def(
            "pistol",
            Secondary,
            3,
            None,
            firearm(
                r,
                ability(
                    41,
                    "Pistol",
                    180,
                    0,
                    0,
                    0.9,
                    Interrupt::Never,
                    vec![(0, bullet(r, packet(32, Pierce, 40.0, 6), [16.0, 4.0, -2.0]))],
                    r,
                ),
                8,
                "pistol_round",
                1600,
                180,
                FireMode::Semi,
                &[
                    (0.0, 0.8),
                    (0.3, 1.5),
                    (-0.4, 2.1),
                    (0.5, 2.6),
                    (-0.3, 3.0),
                    (0.2, 3.3),
                ],
                [1.0, 0.6, 2.5, 7.0, 0.8],
                350,
                0,
            ),
        ),
        def(
            "carbine",
            Primary,
            2,
            None,
            firearm(
                r,
                ability(
                    42,
                    "Carbine",
                    100,
                    0,
                    0,
                    0.7,
                    Interrupt::Never,
                    vec![(0, bullet(r, packet(26, Pierce, 30.0, 4), [20.0, 4.0, -2.0]))],
                    r,
                ),
                25,
                "carbine_round",
                2400,
                100,
                FireMode::Auto,
                &[
                    (0.0, 0.4),
                    (0.0, 0.9),
                    (0.1, 1.4),
                    (0.1, 2.0),
                    (0.2, 2.6),
                    (0.3, 3.1),
                    (0.5, 3.5),
                    (0.8, 3.8),
                    (1.1, 4.0),
                    (1.3, 4.1),
                    (1.2, 4.2),
                    (0.8, 4.3),
                    (0.3, 4.4),
                    (-0.3, 4.4),
                    (-0.9, 4.4),
                    (-1.4, 4.3),
                    (-1.7, 4.2),
                    (-1.6, 4.2),
                    (-1.2, 4.3),
                    (-0.6, 4.4),
                    (0.0, 4.5),
                    (0.6, 4.5),
                    (1.1, 4.5),
                    (1.4, 4.5),
                    (1.4, 4.5),
                ],
                [0.5, 0.3, 3.5, 9.0, 0.35],
                300,
                0,
            ),
        ),
    ];
    let mut pack = ContentPack {
        abilities,
        ..ContentPack::default()
    };
    let id = |p: &ContentPack, k: &str| p.find(k).expect(k);
    // The chains (MODES.md 4.3), by key, as the content loader resolves them.
    for (from, to, window_ms) in [
        ("sword", "sword_2", 400),
        ("sword_2", "sword_3", 400),
        ("dagger", "dagger_2", 350),
        ("dagger_2", "dagger_3", 350),
    ] {
        let next = id(&pack, to);
        let i = id(&pack, from) as usize;
        pack.abilities[i].ability.chain = Some(Chain {
            next,
            window: r.ms_to_ticks(window_ms),
        });
    }
    // The reach of a target-action (MODES.md 5.3), as the loader derives it: an arc's
    // reach, an aimed area's range, 600 for a bolt, or what content names.
    for def in &mut pack.abilities {
        let named = match def.key.as_str() {
            "musket" => Some(1500.0),
            "pistol" => Some(700.0),
            "carbine" => Some(1200.0),
            _ => None,
        };
        def.ability.range = named.unwrap_or_else(|| {
            def.ability
                .steps
                .iter()
                .find_map(|s| match &s.verb {
                    Verb::MeleeArc(m) => Some(m.reach),
                    Verb::Projectile(_) => Some(600.0),
                    Verb::AreaEffect(ae) => match ae.origin {
                        Origin::Aim { range } => Some(range),
                        _ => None,
                    },
                    _ => None,
                })
                .unwrap_or(0.0)
        });
    }
    let builds = vec![
        NamedBuild {
            name: "ironclad".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Colossus,
                attributes: Attributes::new(15, 5, 20, 5, 10),
                armour: ArmourClass::Plate,
                aspects: Aspects::one(Element::Stone),
                primary: id(&pack, "hammer"),
                secondary: id(&pack, "shield_bash"),
                guard: Some(id(&pack, "shield_wall")),
                actives: vec![id(&pack, "stomp"), id(&pack, "fortify")],
            },
        },
        NamedBuild {
            name: "blade".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::new(17, 13, 15, 5, 5),
                armour: ArmourClass::Mail,
                aspects: Aspects::one(Element::Flame),
                primary: id(&pack, "sword"),
                secondary: id(&pack, "kick"),
                guard: Some(id(&pack, "parry")),
                actives: vec![id(&pack, "dash"), id(&pack, "overhead")],
            },
        },
        NamedBuild {
            name: "frostweaver".into(),
            build: Build {
                mode: Mode::Rpg,
                frame: ArchetypeFrame::Caster,
                attributes: Attributes::new(5, 10, 5, 20, 15),
                armour: ArmourClass::Cloth,
                aspects: Aspects::two(Element::Frost, Element::Shadow),
                primary: id(&pack, "ice_shard"),
                secondary: id(&pack, "kick"),
                guard: Some(id(&pack, "brace")),
                actives: vec![id(&pack, "frost_nova"), id(&pack, "blink")],
            },
        },
        NamedBuild {
            name: "shade".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Infiltrator,
                attributes: Attributes::new(15, 20, 10, 5, 5),
                armour: ArmourClass::Leather,
                aspects: Aspects::one(Element::Shadow),
                primary: id(&pack, "dagger"),
                secondary: id(&pack, "throwing_knife"),
                guard: Some(id(&pack, "parry")),
                actives: vec![
                    id(&pack, "blink"),
                    id(&pack, "vanish"),
                    id(&pack, "poison_cloud"),
                ],
            },
        },
        NamedBuild {
            name: "mender".into(),
            build: Build {
                mode: Mode::Rpg,
                frame: ArchetypeFrame::Caster,
                attributes: Attributes::new(5, 5, 10, 15, 20),
                armour: ArmourClass::Cloth,
                aspects: Aspects::one(Element::Storm),
                primary: id(&pack, "staff"),
                secondary: id(&pack, "mend"),
                guard: Some(id(&pack, "brace")),
                actives: vec![
                    id(&pack, "sanctuary"),
                    id(&pack, "haste"),
                    id(&pack, "thunderclap"),
                ],
            },
        },
        NamedBuild {
            name: "captain".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::new(15, 10, 15, 5, 10),
                armour: ArmourClass::Mail,
                aspects: Aspects::one(Element::Flame),
                primary: id(&pack, "sword"),
                secondary: id(&pack, "kick"),
                guard: Some(id(&pack, "parry")),
                actives: vec![id(&pack, "war_standard"), id(&pack, "dash")],
            },
        },
        NamedBuild {
            name: "marksman".into(),
            build: Build {
                mode: Mode::Rpg,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::new(10, 20, 10, 5, 10),
                armour: ArmourClass::Leather,
                aspects: Aspects::one(Element::Storm),
                primary: id(&pack, "crossbow"),
                secondary: id(&pack, "throwing_knife"),
                guard: Some(id(&pack, "brace")),
                actives: vec![id(&pack, "dash"), id(&pack, "leap"), id(&pack, "haste")],
            },
        },
        // The gun (MODES.md 3): a musket, a pistol, the knife, a dodge and a vanish.
        NamedBuild {
            name: "musketeer".into(),
            build: Build {
                mode: Mode::Gun,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::new(10, 17, 13, 5, 10),
                armour: ArmourClass::Leather,
                aspects: Aspects::one(Element::Shadow),
                primary: id(&pack, "musket"),
                secondary: id(&pack, "pistol"),
                guard: None,
                actives: vec![id(&pack, "dash"), id(&pack, "vanish")],
            },
        },
    ];
    pack.builds = builds;
    pack.creatures = vec![
        CreatureDef {
            key: "sentinel".into(),
            name: "Sentinel".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::new(16, 14, 16, 8, 10),
                armour: ArmourClass::Mail,
                aspects: Aspects::one(Element::Flame),
                primary: id(&pack, "sword"),
                secondary: id(&pack, "crossbow"),
                guard: Some(id(&pack, "parry")),
                actives: vec![id(&pack, "overhead"), id(&pack, "dash")],
            },
            health: 1200,
            stagger_threshold: 0,
            sight: 700.0,
            leash: 900.0,
            boss: false,
            respawn_s: 600,
            loot: None,
            still: false,
            npc: false,
            might: 1.0,
        },
        CreatureDef {
            key: "warden".into(),
            name: "The Warden".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Colossus,
                attributes: Attributes::new(20, 8, 20, 14, 14),
                armour: ArmourClass::Plate,
                aspects: Aspects::one(Element::Stone),
                primary: id(&pack, "maul"),
                secondary: id(&pack, "stone_throw"),
                guard: None,
                actives: vec![id(&pack, "quake"), id(&pack, "stomp")],
            },
            health: 15000,
            stagger_threshold: 400,
            sight: 900.0,
            leash: 1100.0,
            boss: true,
            respawn_s: 120,
            loot: Some(Loot {
                components: 3,
                standard: vec![
                    "core/iron".into(),
                    "frame/ash".into(),
                    "catalyst/basalt".into(),
                ],
                top: vec![
                    "core/dragonbone".into(),
                    "shard/boss_scale".into(),
                    "catalyst/basalt".into(),
                ],
                coin: 30,
            }),
            still: false,
            npc: false,
            might: 2.0,
        },
        // The town (2026-10-06): the dummy and the trainer.
        CreatureDef {
            key: "dummy".into(),
            name: "Training dummy".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Colossus,
                attributes: Attributes::flat(5),
                armour: ArmourClass::Leather,
                aspects: Aspects::one(Element::Stone),
                primary: id(&pack, "staff"),
                secondary: id(&pack, "kick"),
                guard: None,
                actives: vec![],
            },
            health: 5000,
            stagger_threshold: 60000,
            sight: 0.0,
            leash: 0.0,
            boss: false,
            respawn_s: 5,
            loot: None,
            still: true,
            npc: false,
            might: 1.0,
        },
        CreatureDef {
            key: "trainer".into(),
            name: "Trainer".into(),
            build: Build {
                mode: Mode::Action,
                frame: ArchetypeFrame::Striker,
                attributes: Attributes::flat(5),
                armour: ArmourClass::Cloth,
                aspects: Aspects::one(Element::Flame),
                primary: id(&pack, "staff"),
                secondary: id(&pack, "kick"),
                guard: None,
                actives: vec![],
            },
            health: 1000,
            stagger_threshold: 0,
            sight: 0.0,
            leash: 0.0,
            boss: false,
            respawn_s: 0,
            loot: None,
            still: true,
            npc: true,
            might: 1.0,
        },
    ];
    let trial = |key: &str, role: &str, deaths: u8, lens: Lens| TrialDef {
        key: key.into(),
        name: format!("Trial of the Warden: the {role}"),
        map: "dungeon".into(),
        encounter: "warden".into(),
        time_limit_s: 300,
        max_party_deaths: Some(deaths),
        max_humans: 1,
        lens,
    };
    pack.trials = vec![
        trial(
            "warden_leader",
            "leader",
            1,
            Lens {
                command: 500,
                ..Lens::default()
            },
        ),
        trial(
            "warden_vanguard",
            "vanguard",
            1,
            Lens {
                tank: 500,
                ..Lens::default()
            },
        ),
        trial(
            "warden_striker",
            "striker",
            1,
            Lens {
                damage: 350,
                ..Lens::default()
            },
        ),
        trial(
            "warden_mender",
            "mender",
            0,
            Lens {
                healing: 500,
                ..Lens::default()
            },
        ),
    ];
    pack
}

/// The Phase 2 three-ability character (sword, crossbow, dash) on flat attributes, for the
/// simulation and netcode tests. Not budget-balanced; use with `add_player_at`.
pub fn phase2_build(pack: &ContentPack) -> Build {
    Build {
        mode: Mode::Action,
        frame: ArchetypeFrame::Striker,
        attributes: Attributes::flat(10),
        armour: ArmourClass::Cloth,
        aspects: Aspects::one(Element::Flame),
        primary: pack.find("sword").expect("sword"),
        secondary: pack.find("crossbow").expect("crossbow"),
        guard: None,
        actives: vec![pack.find("dash").expect("dash")],
    }
}

pub fn phase2_sheet(rate: TickRate) -> Sheet {
    let pack = pack(rate);
    Sheet::new(phase2_build(&pack), &pack, 0)
}
