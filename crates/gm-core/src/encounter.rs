//! The encounter ledger (COMPANIONS.md 9): who did what to an encounter's creatures and what
//! the creatures did back. Pure bookkeeping: the zone feeds it the simulation's events and
//! reads the loot parties (section 10) and the trial standings (section 11) out of it.

use std::collections::BTreeMap;

use crate::loot;
use crate::tick::Tick;
use crate::trial::Standing;
use crate::vocab::EntityId;

/// A body as the ledger files it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Who {
    pub id: EntityId,
    pub party: u32,
    /// The human it fights for: itself, or a companion's commander.
    pub owner: EntityId,
    pub human: bool,
}

/// One body's line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub party: u32,
    pub owner: EntityId,
    pub human: bool,
    /// Damage dealt to each creature of the encounter.
    pub dealt: Vec<(EntityId, u64)>,
    /// Of that, what a companion dealt to a creature its commander had ordered it to attack.
    pub ordered: u64,
    /// Melee and projectile damage the creatures aimed at this body, before its block.
    pub blows: u64,
    /// Every damage the creatures dealt to this body (the cap on healing credit).
    pub taken: u64,
    /// Healing this body is credited for, as the healer.
    pub healing: u64,
    /// Healing others were credited for on this body; never more than `taken`.
    pub mended: u64,
    pub deaths: u32,
}

impl Entry {
    pub fn damage(&self) -> u64 {
        self.dealt.iter().map(|(_, d)| *d).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    /// Server tick the encounter was engaged at.
    pub engaged: Tick,
    entries: BTreeMap<EntityId, Entry>,
}

impl Ledger {
    pub fn new(engaged: Tick) -> Ledger {
        Ledger {
            engaged,
            entries: BTreeMap::new(),
        }
    }

    fn entry(&mut self, who: &Who) -> &mut Entry {
        // A commander takes part through its squad: a companion's line brings its owner's,
        // or a leader who fought from the stance would not be on the ledger at all.
        if !who.human && who.owner != 0 && who.owner != who.id {
            let o = self.entries.entry(who.owner).or_default();
            o.party = who.party;
            o.owner = who.owner;
            o.human = true;
        }
        let e = self.entries.entry(who.id).or_default();
        e.party = who.party;
        e.owner = who.owner;
        e.human = who.human;
        e
    }

    /// `who` damaged a creature of the encounter. `ordered`: it is a companion under its
    /// commander's `Attack` order naming that creature.
    pub fn dealt(&mut self, who: &Who, creature: EntityId, amount: u32, ordered: bool) {
        let e = self.entry(who);
        match e.dealt.iter_mut().find(|(c, _)| *c == creature) {
            Some((_, d)) => *d += amount as u64,
            None => e.dealt.push((creature, amount as u64)),
        }
        if ordered {
            e.ordered += amount as u64;
        }
    }

    /// A creature damaged `who`: `amount` is what landed, `raw` what was aimed (before the
    /// block), `blow` whether it was a swing or a projectile (not an area, not a status).
    pub fn struck(&mut self, who: &Who, amount: u32, raw: u32, blow: bool) {
        let e = self.entry(who);
        e.taken += amount as u64;
        if blow {
            e.blows += raw as u64;
        }
    }

    /// `healer` restored `amount` health on `target`. Credited only as far as it mends what
    /// the creatures did to that target; returns the credit.
    pub fn healed(&mut self, healer: &Who, target: EntityId, amount: u32) -> u32 {
        let Some(t) = self.entries.get_mut(&target) else {
            return 0;
        };
        let credit = (t.taken.saturating_sub(t.mended)).min(amount as u64);
        if credit == 0 {
            return 0;
        }
        t.mended += credit;
        self.entry(healer).healing += credit;
        credit as u32
    }

    pub fn died(&mut self, id: EntityId) {
        if let Some(e) = self.entries.get_mut(&id) {
            e.deaths += 1;
        }
    }

    pub fn is_participant(&self, id: EntityId) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn participants(&self) -> impl Iterator<Item = (EntityId, &Entry)> {
        self.entries.iter().map(|(id, e)| (*id, e))
    }

    /// The parties on the ledger, ascending.
    pub fn parties(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self.entries.values().map(|e| e.party).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A party that brought any companion draws from the standard list (the loot ceiling).
    pub fn has_companions(&self, party: u32) -> bool {
        self.entries.values().any(|e| e.party == party && !e.human)
    }

    /// A party goes out (COMPANIONS.md 9): its lines leave the ledger and the damage it dealt
    /// is handed back, per creature, to be healed.
    pub fn remove_party(&mut self, party: u32) -> Vec<(EntityId, u64)> {
        let mut back: BTreeMap<EntityId, u64> = BTreeMap::new();
        self.entries.retain(|_, e| {
            if e.party != party {
                return true;
            }
            for (creature, d) in &e.dealt {
                *back.entry(*creature).or_default() += *d;
            }
            false
        });
        back.into_iter().collect()
    }

    /// The loot split's input (ECONOMY.md 9): one party per ledger party, its members the
    /// humans, each credited with its own damage and that of the companions it commands.
    /// `present(party)`: a participant of the party is alive within the leash at the kill.
    /// `here(human)`: the human is still in the zone; one who left before the kill is no
    /// member any more, so its share goes to those who stayed instead of to nobody.
    pub fn loot_parties(
        &self,
        present: impl Fn(u32) -> bool,
        here: impl Fn(EntityId) -> bool,
    ) -> Vec<loot::Party> {
        self.parties()
            .into_iter()
            .map(|party| {
                let members = self
                    .entries
                    .iter()
                    .filter(|(id, e)| e.party == party && e.human && here(**id))
                    .map(|(&id, e)| loot::Member {
                        id: id as u64,
                        contribution: e.damage()
                            + self
                                .entries
                                .values()
                                .filter(|c| !c.human && c.owner == id)
                                .map(|c| c.damage())
                                .sum::<u64>(),
                    })
                    .collect();
                loot::Party {
                    id: party as u64,
                    living_member_present: present(party),
                    members,
                }
            })
            .collect()
    }

    /// What a trial sees of `candidate` (a human participant), `secs` after engaging.
    pub fn standing(&self, candidate: EntityId, secs: u32) -> Option<Standing> {
        let me = self.entries.get(&candidate).filter(|e| e.human)?;
        let pm = |num: u64, den: u64| -> u32 { (num * 1000).checked_div(den).unwrap_or(0) as u32 };
        let all = || self.entries.values();
        let mine = |e: &&Entry| e.party == me.party;
        let squad = |e: &&Entry| !e.human && e.owner == candidate;
        Some(Standing {
            secs,
            party_deaths: me.deaths + all().filter(squad).map(|e| e.deaths).sum::<u32>(),
            humans: all().filter(mine).filter(|e| e.human).count() as u32,
            damage: pm(me.damage(), all().map(|e| e.damage()).sum()),
            tank: pm(me.blows, all().map(|e| e.blows).sum()),
            healing: pm(me.healing, all().map(|e| e.healing).sum()),
            command: pm(
                all().filter(squad).map(|e| e.ordered).sum(),
                all().filter(mine).map(|e| e.damage()).sum(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOSS: EntityId = 100;
    const ADD: EntityId = 101;

    fn human(id: EntityId) -> Who {
        Who {
            id,
            party: id,
            owner: id,
            human: true,
        }
    }

    fn companion(id: EntityId, owner: EntityId) -> Who {
        Who {
            id,
            party: owner,
            owner,
            human: false,
        }
    }

    #[test]
    fn a_commander_is_credited_with_its_squad() {
        let mut l = Ledger::new(10);
        let (leader, tank, dps) = (human(1), companion(2, 1), companion(3, 1));
        // The leader fights from the stance: no damage of its own.
        l.dealt(&tank, BOSS, 300, true);
        l.dealt(&dps, BOSS, 500, true);
        l.dealt(&dps, ADD, 100, false);
        l.struck(&tank, 40, 200, true);
        let rival = human(9);
        l.dealt(&rival, BOSS, 100, false);
        let parties = l.loot_parties(|_| true, |_| true);
        assert_eq!(parties.len(), 2);
        // A human who left the zone before the kill is no member: nothing is split to it.
        let stayed = l.loot_parties(|_| true, |id| id != 9);
        assert!(stayed[1].members.is_empty());
        assert_eq!(loot::split(3, &stayed), vec![(1, 3)]);
        assert_eq!(parties[0].id, 1);
        assert_eq!(
            parties[0].members,
            vec![loot::Member {
                id: 1,
                contribution: 900
            }]
        );
        assert_eq!(parties[1].members[0].contribution, 100);
        assert!(l.has_companions(1) && !l.has_companions(9));
        // The split of three components: the squad's work is the leader's work.
        let split = loot::split(3, &parties);
        assert_eq!(split, vec![(1, 2), (9, 1)]);
        // The leader's standing: its companions dealt 800 of the party's 900 under orders.
        let s = l.standing(leader.id, 90).unwrap();
        assert_eq!(s.command, 888);
        assert_eq!(s.damage, 0);
        assert_eq!(s.humans, 1);
        assert_eq!(s.tank, 0);
        // A companion is not a candidate.
        assert!(l.standing(tank.id, 90).is_none());
    }

    #[test]
    fn healing_is_credited_only_against_what_the_creatures_did() {
        let mut l = Ledger::new(0);
        let (healer, tank) = (human(1), companion(2, 1));
        // Nothing to mend yet: topping up a full body earns nothing.
        assert_eq!(l.healed(&healer, tank.id, 50), 0);
        l.struck(&tank, 80, 400, true);
        assert_eq!(l.healed(&healer, tank.id, 50), 50);
        // Only 30 of the next 50 mend what the boss did.
        assert_eq!(l.healed(&healer, tank.id, 50), 30);
        assert_eq!(l.healed(&healer, tank.id, 50), 0);
        // An area is damage taken but not a blow: it does not count for the tank lens.
        l.struck(&healer, 70, 70, false);
        let s = l.standing(1, 10).unwrap();
        assert_eq!(s.healing, 1000);
        assert_eq!(s.tank, 0);
        assert_eq!(l.participants().count(), 2);
    }

    #[test]
    fn a_party_that_goes_out_takes_its_damage_with_it() {
        let mut l = Ledger::new(0);
        let (a, a2, b) = (human(1), companion(2, 1), human(5));
        l.dealt(&a, BOSS, 100, false);
        l.dealt(&a2, BOSS, 250, false);
        l.dealt(&a2, ADD, 40, false);
        l.dealt(&b, BOSS, 70, false);
        l.died(a.id);
        l.died(a2.id);
        assert_eq!(l.standing(1, 5).unwrap().party_deaths, 2);
        assert_eq!(l.parties(), vec![1, 5]);
        let back = l.remove_party(1);
        assert_eq!(back, vec![(BOSS, 350), (ADD, 40)]);
        assert_eq!(l.parties(), vec![5]);
        assert!(!l.is_participant(1) && l.is_participant(5));
        // The rival's share is now the whole of what is left.
        assert_eq!(l.standing(5, 5).unwrap().damage, 1000);
        assert!(l.remove_party(1).is_empty());
    }
}
