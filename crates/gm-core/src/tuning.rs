//! Tuning a running zone's timings by hand (GM.md 3): a tempo over every script, and
//! numbers set outright on single abilities. A tuning is applied to the pack the zone
//! loaded (never to one already tuned), so every change is reversible and the content
//! files stay the source. What comes out is an ordinary pack: clients run exactly it.

use crate::build::ContentPack;
use crate::tick::{Tick, TickRate};
use crate::vocab::{Guard, MoveKind, Verb};

/// A number set outright on one ability, in milliseconds (content's own unit); `None`
/// leaves the pack's (tempo-scaled) value.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AbilityTuning {
    pub key: String,
    /// The moment the script's last step fires (a cast's time, a swing's start); earlier
    /// steps keep their proportion.
    pub cast_ms: Option<u32>,
    /// A melee verb's three windows.
    pub windup_ms: Option<u32>,
    pub active_ms: Option<u32>,
    pub recovery_ms: Option<u32>,
    pub cooldown_ms: Option<u32>,
}

impl AbilityTuning {
    pub fn is_empty(&self) -> bool {
        self.cast_ms.is_none()
            && self.windup_ms.is_none()
            && self.active_ms.is_none()
            && self.recovery_ms.is_none()
            && self.cooldown_ms.is_none()
    }
}

/// Everything set by hand on a zone's content.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Tuning {
    /// Every script's windups, windows, recoveries, cast times, parry windows and dashes
    /// take this many times as long (1 = as authored). Cooldowns, statuses and bolts keep
    /// their own time: the tempo is about what a body shows before it lands something.
    pub tempo: f32,
    #[cfg_attr(feature = "serde", serde(default))]
    pub abilities: Vec<AbilityTuning>,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            tempo: 1.0,
            abilities: Vec::new(),
        }
    }
}

pub const TEMPO_RANGE: (f32, f32) = (0.25, 4.0);

impl Tuning {
    pub fn is_default(&self) -> bool {
        self.tempo == 1.0 && self.abilities.iter().all(|a| a.is_empty())
    }

    pub fn ability(&self, key: &str) -> Option<&AbilityTuning> {
        self.abilities.iter().find(|a| a.key == key)
    }

    /// Set one ability's numbers (replacing what was set on it before); an empty
    /// override is dropped.
    pub fn set(&mut self, a: AbilityTuning) {
        self.abilities.retain(|x| x.key != a.key);
        if !a.is_empty() {
            self.abilities.push(a);
        }
    }

    /// The pack as loaded with this tuning on it. The caller validates the result
    /// (`ContentPack::validate`): a number can be set that content would refuse.
    pub fn apply(&self, base: &ContentPack, rate: TickRate) -> ContentPack {
        let mut pack = base.clone();
        let tempo = self.tempo.clamp(TEMPO_RANGE.0, TEMPO_RANGE.1);
        for def in &mut pack.abilities {
            let a = &mut def.ability;
            if tempo != 1.0 {
                for step in &mut a.steps {
                    step.at = scale(step.at, tempo);
                    match &mut step.verb {
                        Verb::MeleeArc(m) => {
                            m.timing.windup = scale(m.timing.windup, tempo).max(1);
                            m.timing.active = scale(m.timing.active, tempo).max(1);
                            m.timing.recovery = scale(m.timing.recovery, tempo);
                        }
                        Verb::Guard(Guard::Parry(p)) => {
                            p.window = scale(p.window, tempo).max(1);
                            p.whiff_recovery = scale(p.whiff_recovery, tempo);
                        }
                        Verb::MoveSelf(mv) => match &mut mv.kind {
                            MoveKind::Dash { duration, .. } | MoveKind::Charge { duration, .. } => {
                                *duration = scale(*duration, tempo).max(1);
                            }
                            MoveKind::Leap { .. } | MoveKind::Blink { .. } => {}
                        },
                        Verb::Projectile(_) | Verb::AreaEffect(_) | Verb::ApplyStatus(_) => {}
                        Verb::Guard(Guard::Block(_)) => {}
                    }
                }
            }
            let Some(t) = self.ability(&def.key) else {
                continue;
            };
            if let Some(ms) = t.cast_ms
                && let Some(last) = a.steps.iter().map(|s| s.at).max()
            {
                let want = rate.ms_to_ticks(ms);
                for step in &mut a.steps {
                    step.at = if last == 0 {
                        want
                    } else {
                        ((step.at as u64 * want as u64) / last as u64) as Tick
                    };
                }
            }
            for step in &mut a.steps {
                if let Verb::MeleeArc(m) = &mut step.verb {
                    if let Some(ms) = t.windup_ms {
                        m.timing.windup = rate.ms_to_ticks(ms).max(1);
                    }
                    if let Some(ms) = t.active_ms {
                        m.timing.active = rate.ms_to_ticks(ms).max(1);
                    }
                    if let Some(ms) = t.recovery_ms {
                        m.timing.recovery = rate.ms_to_ticks(ms);
                    }
                }
            }
            if let Some(ms) = t.cooldown_ms {
                a.cooldown.ticks = rate.ms_to_ticks(ms);
            }
        }
        pack
    }
}

fn scale(ticks: Tick, tempo: f32) -> Tick {
    (ticks as f32 * tempo).round() as Tick
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::test_content;

    fn melee(pack: &ContentPack, key: &str) -> crate::vocab::Timing {
        let def = pack.abilities.iter().find(|d| d.key == key).unwrap();
        match &def.ability.steps[0].verb {
            Verb::MeleeArc(m) => m.timing,
            other => panic!("{key} is not a swing: {other:?}"),
        }
    }

    #[test]
    fn the_tempo_stretches_a_swing_and_nothing_else() {
        let rate = TickRate::COMBAT;
        let base = test_content::pack(rate);
        let slow = Tuning {
            tempo: 2.0,
            abilities: Vec::new(),
        }
        .apply(&base, rate);
        let (was, now) = (melee(&base, "sword"), melee(&slow, "sword"));
        assert_eq!(now.windup, was.windup * 2);
        assert_eq!(now.active, was.active * 2);
        assert_eq!(now.recovery, was.recovery * 2);
        let cooldown = |p: &ContentPack| {
            p.abilities
                .iter()
                .find(|d| d.key == "sword")
                .unwrap()
                .ability
                .cooldown
                .ticks
        };
        assert_eq!(
            cooldown(&slow),
            cooldown(&base),
            "cooldowns keep their time"
        );
        assert!(slow.validate(rate).is_ok());
        // Back to 1: the pack as loaded.
        assert_eq!(Tuning::default().apply(&base, rate), base);
    }

    #[test]
    fn a_number_set_outright_wins_over_the_tempo() {
        let rate = TickRate::COMBAT;
        let base = test_content::pack(rate);
        let mut t = Tuning {
            tempo: 3.0,
            abilities: Vec::new(),
        };
        t.set(AbilityTuning {
            key: "sword".into(),
            windup_ms: Some(500),
            cooldown_ms: Some(1000),
            ..Default::default()
        });
        let pack = t.apply(&base, rate);
        let now = melee(&pack, "sword");
        assert_eq!(now.windup, rate.ms_to_ticks(500));
        assert_eq!(now.active, melee(&base, "sword").active * 3);
        // Setting it empty forgets it.
        t.set(AbilityTuning {
            key: "sword".into(),
            ..Default::default()
        });
        assert!(t.ability("sword").is_none());
    }

    #[test]
    fn a_cast_time_moves_the_last_step_and_keeps_the_proportion() {
        let rate = TickRate::COMBAT;
        let base = test_content::pack(rate);
        let key = base
            .abilities
            .iter()
            .find(|d| d.ability.steps.iter().any(|s| s.at > 0))
            .map(|d| d.key.clone())
            .expect("a cast in the test content");
        let mut t = Tuning::default();
        t.set(AbilityTuning {
            key: key.clone(),
            cast_ms: Some(1000),
            ..Default::default()
        });
        let pack = t.apply(&base, rate);
        let def = pack.abilities.iter().find(|d| d.key == key).unwrap();
        let last = def.ability.steps.iter().map(|s| s.at).max().unwrap();
        assert_eq!(last, rate.ms_to_ticks(1000));
    }
}
