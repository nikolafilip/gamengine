//! The zone's mirror of the hub's parties (PARTY.md 2, 4). The hub is the owner; this
//! keeps, for each character, the hub's reading with the largest number, gives each of the
//! hub's parties one number for the bodies of this zone, and says what a body's number
//! ought to be. The number a body *has* changes only when the body is in no fight: that
//! is the caller's to see to.

use std::collections::HashMap;
use std::time::Duration;

use gm_core::vocab::EntityId;
use gm_hub_proto::protocol::{CharacterId, PartyNews, PartyReading, PartyState};
use tokio::time::Instant;

/// The numbers of parties of people begin here; entity ids stay below
/// (`gm_core::sim::MAX_ENTITY_ID`).
pub const PARTY_BASE: u32 = gm_core::sim::MAX_ENTITY_ID + 1;
const _: () = assert!(PARTY_BASE > gm_core::sim::MAX_ENTITY_ID);
/// A reading for a character that has no body here at the moment is kept this long: news
/// that arrived before the body a join is about to make.
pub const KEPT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct ZoneParties {
    /// What the hub last said of each character that has a body here.
    readings: HashMap<CharacterId, PartyReading>,
    /// And of characters that have none at the moment.
    kept: HashMap<CharacterId, (PartyReading, Instant)>,
    /// The number each of the hub's parties has in this zone, for as long as the party
    /// is (the hub never makes a party with an old party's id), and the next to give.
    numbers: HashMap<i64, u32>,
    next: u32,
}

impl ZoneParties {
    /// A character got a body: the reading its claim carried, or a newer one that was
    /// kept for it. Returns what it is now held to be.
    pub fn joined(&mut self, character: CharacterId, claim: PartyReading) -> &PartyReading {
        let mut reading = claim;
        if let Some((kept, _)) = self.kept.remove(&character)
            && kept.seq > reading.seq
        {
            reading = kept;
        }
        // (A body it had until a moment ago may have left a newer one too.)
        if let Some(had) = self.readings.remove(&character)
            && had.seq > reading.seq
        {
            reading = had;
        }
        self.readings.entry(character).or_insert(reading)
    }

    /// A character's body is gone: what was known of it is kept for a while, for the
    /// body it may get again.
    pub fn left(&mut self, character: CharacterId, now: Instant) {
        if let Some(reading) = self.readings.remove(&character) {
            self.keep(character, reading, now);
        }
    }

    fn keep(&mut self, character: CharacterId, reading: PartyReading, now: Instant) {
        self.kept
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < KEPT);
        let newer = self
            .kept
            .get(&character)
            .is_none_or(|(known, _)| reading.seq > known.seq);
        if newer {
            self.kept.insert(character, (reading, now));
        }
        // A number whose party nobody here (nor anybody kept) is held to be of any more is
        // let go of: a party dissolved after its last member left this zone is news this
        // zone never hears, and its number would otherwise stay for the zone's life.
        let held: std::collections::HashSet<i64> = self
            .readings
            .values()
            .chain(self.kept.values().map(|(r, _)| r))
            .filter_map(|r| r.party.as_ref().map(|p| p.id))
            .collect();
        self.numbers.retain(|id, _| held.contains(id));
    }

    /// The hub's news of a change. Returns the characters with a body here whose reading
    /// it changed.
    pub fn news(&mut self, news: &PartyNews, now: Instant) -> Vec<CharacterId> {
        let mut changed = Vec::new();
        let members = news.party.members.iter().map(|(id, _)| (*id, true));
        let left = news.left.iter().map(|id| (*id, false));
        for (character, member) in members.chain(left) {
            let reading = PartyReading {
                seq: news.seq,
                party: member.then(|| news.party.clone()),
            };
            match self.readings.get_mut(&character) {
                Some(known) => {
                    if reading.seq > known.seq {
                        *known = reading;
                        changed.push(character);
                    }
                }
                None => self.keep(character, reading, now),
            }
        }
        if news.party.members.is_empty()
            && !self
                .readings
                .values()
                .any(|r| r.party.as_ref().is_some_and(|p| p.id == news.party.id))
        {
            // Dissolved, and nobody here is held to be of it any more: its number is let
            // go of (a body in a fight under it keeps the number as a plain number until
            // the fight is over; the hub never makes a party with this id again).
            self.numbers.remove(&news.party.id);
        }
        changed
    }

    /// Whether what is held of a character with a body here is older than the hub's
    /// reading number `seq`.
    pub fn behind(&self, character: CharacterId, seq: u64) -> bool {
        self.readings.get(&character).is_some_and(|r| r.seq < seq)
    }

    /// A reading the zone asked for. `true`: it is newer than what was held of a
    /// character with a body here, and is held now.
    pub fn read(&mut self, character: CharacterId, reading: PartyReading) -> bool {
        match self.readings.get_mut(&character) {
            Some(known) if reading.seq > known.seq => {
                *known = reading;
                true
            }
            _ => false,
        }
    }

    /// The party the hub has a character in.
    pub fn of(&self, character: CharacterId) -> Option<&PartyState> {
        self.readings.get(&character)?.party.as_ref()
    }

    /// The number the body `id` of `character` ought to carry: its party's number in
    /// this zone, or its own id.
    pub fn number(&mut self, character: CharacterId, id: EntityId) -> u32 {
        let Some(party) = self.readings.get(&character).and_then(|r| r.party.as_ref()) else {
            return id;
        };
        let next = PARTY_BASE + self.next;
        *self.numbers.entry(party.id).or_insert_with(|| {
            self.next += 1;
            next
        })
    }

    /// The names of a character's party as its client is told them: the leader first,
    /// the rest in the order they joined; nobody when it is in none.
    pub fn names(&self, character: CharacterId) -> Vec<String> {
        let Some(party) = self.of(character) else {
            return Vec::new();
        };
        let leader = party.members.iter().filter(|(id, _)| *id == party.leader);
        let rest = party.members.iter().filter(|(id, _)| *id != party.leader);
        leader.chain(rest).map(|(_, name)| name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: i64, leader: CharacterId, members: &[(CharacterId, &str)]) -> PartyState {
        PartyState {
            id,
            leader,
            members: members.iter().map(|(c, n)| (*c, n.to_string())).collect(),
        }
    }

    fn news(seq: u64, party: PartyState, left: &[CharacterId]) -> PartyNews {
        PartyNews {
            seq,
            party,
            left: left.to_vec(),
        }
    }

    #[test]
    fn the_reading_with_the_largest_number_is_kept_whatever_the_order() {
        let t0 = Instant::now();
        let mut p = ZoneParties::default();
        // Two characters with bodies (10 and 20), in no party.
        p.joined(1, PartyReading::default());
        p.joined(2, PartyReading::default());
        assert_eq!((p.number(1, 10), p.number(2, 20)), (10, 20));
        assert!(p.names(1).is_empty());
        // They become a party: one number for both, above every entity id.
        let pair = state(7, 2, &[(1, "Ana"), (2, "Bojan")]);
        assert_eq!(p.news(&news(5, pair.clone(), &[]), t0), [1, 2]);
        assert_eq!(p.number(1, 10), PARTY_BASE);
        assert_eq!(p.number(2, 20), PARTY_BASE);
        // The leader first, whoever joined first.
        assert_eq!(p.names(1), ["Bojan", "Ana"]);
        // The same news again, and older news, change nothing.
        assert!(p.news(&news(5, pair.clone(), &[]), t0).is_empty());
        assert!(p.news(&news(4, state(7, 1, &[]), &[1, 2]), t0).is_empty());
        assert_eq!(p.number(1, 10), PARTY_BASE);
        // A third joins elsewhere: the two here are told, the third is kept for later.
        let three = state(7, 2, &[(1, "Ana"), (2, "Bojan"), (3, "Cvita")]);
        assert_eq!(p.news(&news(6, three.clone(), &[]), t0), [1, 2]);
        assert_eq!(p.names(2).len(), 3);
        // The third arrives with an older claim (it was read before it joined): what was
        // kept for it is newer, and wins.
        let held = p.joined(3, PartyReading::default()).clone();
        assert_eq!((held.seq, held.party), (6, Some(three)));
        assert_eq!(p.number(3, 30), PARTY_BASE);
        // One leaves: its number is its own again; the others keep theirs.
        let two = state(7, 2, &[(2, "Bojan"), (3, "Cvita")]);
        assert_eq!(p.news(&news(8, two, &[1]), t0), [2, 3, 1]);
        assert_eq!((p.number(1, 10), p.number(2, 20)), (10, PARTY_BASE));
        // Dissolved: nobody is in it, and its number is let go of. Another party gets
        // another number: the first party's is not given again while the zone runs.
        assert_eq!(p.news(&news(9, state(7, 0, &[]), &[2, 3]), t0), [2, 3]);
        assert_eq!((p.number(2, 20), p.number(3, 30)), (20, 30));
        assert!(p.numbers.is_empty());
        let other = state(9, 1, &[(1, "Ana"), (3, "Cvita")]);
        p.news(&news(11, other, &[]), t0);
        assert_eq!(
            (p.number(1, 10), p.number(3, 30)),
            (PARTY_BASE + 1, PARTY_BASE + 1)
        );
    }

    #[test]
    fn a_body_that_comes_again_finds_what_was_known_and_old_news_is_forgotten() {
        let t0 = Instant::now();
        let later = |secs: u64| t0 + Duration::from_secs(secs);
        let mut p = ZoneParties::default();
        let pair = state(7, 1, &[(1, "Ana"), (2, "Bojan")]);
        p.joined(1, PartyReading::default());
        p.news(&news(5, pair.clone(), &[]), t0);
        // Its body goes (a reconnect); it comes back with a claim read before the change.
        p.left(1, t0);
        assert!(p.of(1).is_none(), "no body, no reading in use");
        assert_eq!(
            p.joined(
                1,
                PartyReading {
                    seq: 3,
                    party: None
                }
            )
            .seq,
            5
        );
        // A claim that is newer than what was kept wins the other way.
        p.left(1, t0);
        assert_eq!(
            p.joined(
                1,
                PartyReading {
                    seq: 9,
                    party: None
                }
            )
            .party,
            None
        );
        // News for somebody who never comes is forgotten after two minutes.
        p.news(&news(12, pair, &[]), t0);
        assert_eq!(p.kept.len(), 1, "for the one without a body");
        p.left(1, later(121));
        assert!(!p.kept.contains_key(&2));
        assert_eq!(p.joined(2, PartyReading::default()).seq, 0);
        // A party whose every member left this zone, and whose dissolution this zone
        // never hears of: its number goes when nobody is held to be of it any more (the
        // kept readings' two minutes), not at the zone's end.
        let mut q = ZoneParties::default();
        q.joined(1, PartyReading::default());
        q.news(&news(5, state(7, 1, &[(1, "Ana"), (2, "Bojan")]), &[]), t0);
        assert_eq!(q.number(1, 10), PARTY_BASE);
        q.left(1, later(10));
        assert_eq!(q.numbers.len(), 1, "kept for the body that may come again");
        q.joined(3, PartyReading::default());
        q.left(3, later(200));
        assert!(q.numbers.is_empty(), "nobody here nor kept is of it");
        // The repair: a zone that is behind the hub's number takes the reading it asks
        // for; one that is not, and a reading for somebody without a body, change nothing.
        assert!(!p.behind(2, 0) && p.behind(2, 7) && !p.behind(9, 7));
        let mended = PartyReading {
            seq: 7,
            party: Some(state(3, 2, &[(2, "Bojan"), (4, "Dane")])),
        };
        assert!(p.read(2, mended.clone()));
        assert!(!p.read(2, mended.clone()) && !p.read(9, mended));
        assert_eq!(p.names(2), ["Bojan", "Dane"]);
    }
}
