//! Role trials (COMPANIONS.md 11): an encounter plus conditions, judged for each human
//! participant when the encounter is cleared. The verdict is a pure function of the trial
//! and of what the ledger says about the candidate.

/// The role lens: each share is per mille of the encounter's total; 0 = not asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Lens {
    /// The candidate's own damage, of all damage dealt to the creatures.
    pub damage: u16,
    /// The blows aimed at the candidate, of all blows the creatures aimed at anyone.
    pub tank: u16,
    /// The candidate's credited healing, of all credited healing.
    pub healing: u16,
    /// Damage the candidate's companions dealt under its `Attack` orders, of the party's.
    pub command: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct TrialDef {
    /// What is recorded on the character.
    pub key: String,
    pub name: String,
    pub map: String,
    pub encounter: String,
    /// Seconds from engaging to clearing; 0 = no limit.
    pub time_limit_s: u32,
    /// Deaths among the candidate and its companions; `None` = any number.
    pub max_party_deaths: Option<u8>,
    /// Humans in the candidate's party; 0 = any number.
    pub max_humans: u8,
    pub lens: Lens,
}

/// What a trial sees of one candidate (`gm_core::encounter::Ledger::standing`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    pub secs: u32,
    pub party_deaths: u32,
    pub humans: u32,
    /// Per mille, as in [`Lens`].
    pub damage: u32,
    pub tank: u32,
    pub healing: u32,
    pub command: u32,
}

impl core::fmt::Display for Standing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let pc = |v: u32| v as f32 / 10.0;
        write!(
            f,
            "{} s, {} of the party died; {:.1}% of the damage, {:.1}% of the blows, {:.1}% of the healing, {:.1}% under orders",
            self.secs,
            self.party_deaths,
            pc(self.damage),
            pc(self.tank),
            pc(self.healing),
            pc(self.command)
        )
    }
}

/// The first condition a candidate failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    TooSlow { secs: u32, limit: u32 },
    Deaths { deaths: u32, limit: u32 },
    Humans { humans: u32, limit: u32 },
    Damage { have: u32, need: u32 },
    Tank { have: u32, need: u32 },
    Healing { have: u32, need: u32 },
    Command { have: u32, need: u32 },
}

impl core::fmt::Display for Failure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let pc = |v: &u32| *v as f32 / 10.0;
        match self {
            Failure::TooSlow { secs, limit } => write!(f, "took {secs} s, the limit is {limit} s"),
            Failure::Deaths { deaths, limit } => {
                write!(f, "{deaths} of the party died, at most {limit} may")
            }
            Failure::Humans { humans, limit } => {
                write!(f, "{humans} players in the party, at most {limit} may be")
            }
            Failure::Damage { have, need } => write!(
                f,
                "dealt {:.1}% of the damage, {:.1}% is asked",
                pc(have),
                pc(need)
            ),
            Failure::Tank { have, need } => write!(
                f,
                "took {:.1}% of the blows, {:.1}% is asked",
                pc(have),
                pc(need)
            ),
            Failure::Healing { have, need } => write!(
                f,
                "did {:.1}% of the healing, {:.1}% is asked",
                pc(have),
                pc(need)
            ),
            Failure::Command { have, need } => write!(
                f,
                "{:.1}% of the party's damage was dealt under your orders, {:.1}% is asked",
                pc(have),
                pc(need)
            ),
        }
    }
}

/// Judge one candidate: `Ok` is a pass, `Err` names the first condition that failed.
pub fn judge(def: &TrialDef, s: &Standing) -> Result<(), Failure> {
    if def.time_limit_s > 0 && s.secs > def.time_limit_s {
        return Err(Failure::TooSlow {
            secs: s.secs,
            limit: def.time_limit_s,
        });
    }
    if let Some(limit) = def.max_party_deaths
        && s.party_deaths > limit as u32
    {
        return Err(Failure::Deaths {
            deaths: s.party_deaths,
            limit: limit as u32,
        });
    }
    if def.max_humans > 0 && s.humans > def.max_humans as u32 {
        return Err(Failure::Humans {
            humans: s.humans,
            limit: def.max_humans as u32,
        });
    }
    let l = &def.lens;
    if s.damage < l.damage as u32 {
        return Err(Failure::Damage {
            have: s.damage,
            need: l.damage as u32,
        });
    }
    if s.tank < l.tank as u32 {
        return Err(Failure::Tank {
            have: s.tank,
            need: l.tank as u32,
        });
    }
    if s.healing < l.healing as u32 {
        return Err(Failure::Healing {
            have: s.healing,
            need: l.healing as u32,
        });
    }
    if s.command < l.command as u32 {
        return Err(Failure::Command {
            have: s.command,
            need: l.command as u32,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trial(lens: Lens) -> TrialDef {
        TrialDef {
            key: "t".into(),
            name: "T".into(),
            map: "dungeon".into(),
            encounter: "warden".into(),
            time_limit_s: 300,
            max_party_deaths: Some(1),
            max_humans: 1,
            lens,
        }
    }

    #[test]
    fn conditions_are_checked_in_order_and_named() {
        let def = trial(Lens {
            command: 500,
            ..Lens::default()
        });
        let good = Standing {
            secs: 120,
            party_deaths: 1,
            humans: 1,
            damage: 200,
            tank: 100,
            healing: 0,
            command: 640,
        };
        assert_eq!(judge(&def, &good), Ok(()));
        assert_eq!(
            judge(&def, &Standing { secs: 301, ..good }),
            Err(Failure::TooSlow {
                secs: 301,
                limit: 300
            })
        );
        assert_eq!(
            judge(
                &def,
                &Standing {
                    party_deaths: 2,
                    ..good
                }
            ),
            Err(Failure::Deaths {
                deaths: 2,
                limit: 1
            })
        );
        assert_eq!(
            judge(&def, &Standing { humans: 2, ..good }),
            Err(Failure::Humans {
                humans: 2,
                limit: 1
            })
        );
        let f = judge(
            &def,
            &Standing {
                command: 499,
                ..good
            },
        )
        .unwrap_err();
        assert_eq!(
            f,
            Failure::Command {
                have: 499,
                need: 500
            }
        );
        assert!(f.to_string().contains("49.9%"), "{f}");
    }

    #[test]
    fn no_limits_means_no_limits() {
        let def = TrialDef {
            time_limit_s: 0,
            max_party_deaths: None,
            max_humans: 0,
            ..trial(Lens::default())
        };
        let s = Standing {
            secs: 99_999,
            party_deaths: 40,
            humans: 12,
            ..Standing::default()
        };
        assert_eq!(judge(&def, &s), Ok(()));
        // Each lens fails on its own share.
        for (lens, expect) in [
            (
                Lens {
                    damage: 350,
                    ..Lens::default()
                },
                Failure::Damage { have: 0, need: 350 },
            ),
            (
                Lens {
                    tank: 500,
                    ..Lens::default()
                },
                Failure::Tank { have: 0, need: 500 },
            ),
            (
                Lens {
                    healing: 500,
                    ..Lens::default()
                },
                Failure::Healing { have: 0, need: 500 },
            ),
        ] {
            let def = TrialDef {
                lens,
                ..def.clone()
            };
            assert_eq!(judge(&def, &s), Err(expect));
        }
    }
}
