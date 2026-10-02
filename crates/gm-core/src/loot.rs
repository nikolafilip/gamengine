//! The corrected boss loot split (PLAN.md 5.2, ECONOMY.md 9). Pure: the zone feeds it the
//! contribution ledger at kill time and hands the result to the hub.

/// One member's part in the kill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: u64,
    /// What it did (PARTY.md 2): the damage it and its companions dealt, the healing
    /// they are credited with and the blows the creatures aimed at them. Its rank among
    /// its party, and the member floor, are by this.
    pub contribution: u64,
    /// The damage it and its companions dealt to the creatures. A party's eligibility and
    /// its share among the parties are by this alone: standing in a boss's cleave, or
    /// mending somebody who does, earns a party nothing of another party's kill.
    pub damage: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Party {
    pub id: u64,
    /// At least one member alive in the arena at kill time.
    pub living_member_present: bool,
    pub members: Vec<Member>,
}

impl Party {
    /// What the party did, for the member floor.
    pub fn contribution(&self) -> u64 {
        self.members.iter().map(|m| m.contribution).sum()
    }

    /// The damage the party dealt, for its standing among the parties.
    pub fn damage(&self) -> u64 {
        self.members.iter().map(|m| m.damage).sum()
    }
}

/// A party must have dealt at least this share of the surviving parties' total damage to
/// be eligible (per mille).
pub const PARTY_FLOOR_PER_MILLE: u64 = 100;
/// A member below this share of the party's average contribution is skipped (per cent).
pub const MEMBER_FLOOR_PER_CENT: u64 = 40;

/// Components per party: only parties with a living member present and at or above the
/// floor; proportional by largest remainder; at least one each; with more eligible parties
/// than components the top contributors get one each. Wiped parties get nothing.
pub fn split_parties(components: u32, parties: &[Party]) -> Vec<(u64, u32)> {
    // The floor is a share of what the parties still standing dealt: a wiped party's damage
    // must not turn everyone else into taggers.
    let total: u64 = parties
        .iter()
        .filter(|p| p.living_member_present)
        .map(|p| p.damage())
        .sum();
    if components == 0 || total == 0 {
        return Vec::new();
    }
    let mut eligible: Vec<(u64, u64)> = parties
        .iter()
        .filter(|p| p.living_member_present)
        .map(|p| (p.id, p.damage()))
        .filter(|&(_, c)| c > 0 && c * 1000 >= total * PARTY_FLOOR_PER_MILLE)
        .collect();
    if eligible.is_empty() {
        return Vec::new();
    }
    // Deterministic order: contribution descending, then id.
    eligible.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let n = components as usize;
    if eligible.len() >= n {
        return eligible.iter().take(n).map(|&(id, _)| (id, 1)).collect();
    }
    // One each, then the rest proportionally by largest remainder.
    let rest = (n - eligible.len()) as u64;
    let pool: u64 = eligible.iter().map(|&(_, c)| c).sum();
    let mut shares: Vec<(u64, u32, u64)> = eligible
        .iter()
        .map(|&(id, c)| {
            let exact = c * rest;
            (id, 1 + (exact / pool) as u32, exact % pool)
        })
        .collect();
    let given: u32 = shares.iter().map(|s| s.1).sum();
    let mut left = components - given;
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|&a, &b| shares[b].2.cmp(&shares[a].2).then(a.cmp(&b)));
    for i in order {
        if left == 0 {
            break;
        }
        shares[i].1 += 1;
        left -= 1;
    }
    shares.into_iter().map(|(id, n, _)| (id, n)).collect()
}

/// Components per member of one party: round-robin by contribution rank, skipping members
/// below the leech floor (40% of the party's average). If everyone is below (impossible with
/// a positive total, but defended), the top contributor takes them.
pub fn split_members(components: u32, party: &Party) -> Vec<(u64, u32)> {
    if components == 0 || party.members.is_empty() {
        return Vec::new();
    }
    let total = party.contribution();
    let count = party.members.len() as u64;
    let mut ranked: Vec<Member> = party
        .members
        .iter()
        .copied()
        // contribution >= 40% of (total / count)  <=>  contribution * count * 100 >= total * 40
        .filter(|m| {
            m.contribution * count * 100 >= total * MEMBER_FLOOR_PER_CENT && m.contribution > 0
        })
        .collect();
    if ranked.is_empty() {
        ranked = party.members.clone();
    }
    ranked.sort_by(|a, b| b.contribution.cmp(&a.contribution).then(a.id.cmp(&b.id)));
    let mut out: Vec<(u64, u32)> = ranked.iter().map(|m| (m.id, 0)).collect();
    for i in 0..components as usize {
        out[i % ranked.len()].1 += 1;
    }
    out.retain(|&(_, n)| n > 0);
    out
}

/// The whole split: `(member id, components)` across every eligible party.
pub fn split(components: u32, parties: &[Party]) -> Vec<(u64, u32)> {
    let mut out = Vec::new();
    for (party_id, n) in split_parties(components, parties) {
        if let Some(p) = parties.iter().find(|p| p.id == party_id) {
            out.extend(split_members(n, p));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(id: u64, alive: bool, members: &[(u64, u64)]) -> Party {
        Party {
            id,
            living_member_present: alive,
            members: members
                .iter()
                .map(|&(id, contribution)| Member {
                    id,
                    contribution,
                    damage: contribution,
                })
                .collect(),
        }
    }

    #[test]
    fn proportional_with_a_minimum_of_one() {
        // 60 / 30 / 10 of the damage, 10 components: one each, then 7 by 60:30:10 = 4.2, 2.1,
        // 0.7 -> 4, 2, 0 and the last by largest remainder (0.7).
        let parties = [
            party(1, true, &[(11, 600)]),
            party(2, true, &[(21, 300)]),
            party(3, true, &[(31, 100)]),
        ];
        assert_eq!(split_parties(10, &parties), vec![(1, 5), (2, 3), (3, 2)]);
        let total: u32 = split_parties(10, &parties).iter().map(|p| p.1).sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn wiped_parties_and_taggers_get_nothing() {
        let parties = [
            party(1, true, &[(11, 500)]),
            // Did most of the work, then wiped: nothing (PLAN.md 1.2).
            party(2, false, &[(21, 900)]),
            // Tagged for 5% of what the survivors dealt: below the 10% floor.
            party(3, true, &[(31, 25)]),
        ];
        assert_eq!(split_parties(6, &parties), vec![(1, 6)]);
        // Everyone wiped: the components are not handed out at all.
        let all_dead = [party(1, false, &[(11, 500)])];
        assert!(split_parties(6, &all_dead).is_empty());
    }

    #[test]
    fn more_parties_than_components_favours_the_top_contributors() {
        let parties = [
            party(1, true, &[(11, 300)]),
            party(2, true, &[(21, 400)]),
            party(3, true, &[(31, 300)]),
        ];
        // Two components, three eligible parties: the two largest; ties break by id.
        assert_eq!(split_parties(2, &parties), vec![(2, 1), (1, 1)]);
    }

    #[test]
    fn members_round_robin_and_leeches_are_skipped() {
        // Average 250; the floor is 100. Member 4 (50) leeched.
        let p = party(1, true, &[(1, 500), (2, 300), (3, 150), (4, 50)]);
        assert_eq!(split_members(5, &p), vec![(1, 2), (2, 2), (3, 1)]);
        assert_eq!(split_members(1, &p), vec![(1, 1)]);
        let whole: u32 = split(7, &[p.clone(), party(2, true, &[(9, 400)])])
            .iter()
            .map(|m| m.1)
            .sum();
        assert_eq!(whole, 7);
    }

    #[test]
    fn nothing_is_ever_created_or_lost() {
        // Exhaustive small cases: the handed-out total equals the components whenever any
        // party is eligible.
        for comps in 1..=12u32 {
            for a in [1u64, 50, 400] {
                for b in [1u64, 200, 999] {
                    let parties = [
                        party(1, true, &[(1, a), (2, a / 2 + 1)]),
                        party(2, true, &[(3, b)]),
                    ];
                    let handed: u32 = split(comps, &parties).iter().map(|m| m.1).sum();
                    let eligible = !split_parties(comps, &parties).is_empty();
                    assert_eq!(handed, if eligible { comps } else { 0 }, "{comps} {a} {b}");
                }
            }
        }
    }
}
