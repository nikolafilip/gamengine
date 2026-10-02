//! Parties of people (PARTY.md 2, 3): who is in which, who leads, who was asked. The hub is
//! the only writer. Every change is made by one writer at a time and takes the next number
//! of one sequence while it is that writer, so the numbers of the readings are in the order
//! of the changes, and a zone that keeps the reading with the largest number for each
//! character holds what the hub holds.

use std::time::Duration;

use gm_hub_proto::names::skeleton;
use gm_hub_proto::protocol::{
    CharacterId, HubError, INVITE_SECS, INVITES_WAITING, PARTY_MAX, PartyNews, PartyReading,
    PartyState, SayTo, ZoneId,
};
use sqlx::{PgPool, Row};

type Tx<'a> = sqlx::Transaction<'a, sqlx::Postgres>;

/// How often the hub looks for members that went offline (PARTY.md 2).
pub const SWEEP: Duration = Duration::from_secs(10);
/// How long a member that left the game is still of its party.
pub const AWAY: Duration = Duration::from_secs(120);

/// The advisory lock every change of a party takes in the database: for whoever writes
/// beside this hub (a test, a tool). Within the hub, `Parties::writer` comes first, and
/// those who wait for it hold no connection.
const LOCK: i64 = 0x7061_7274_7900;

fn internal(e: sqlx::Error) -> HubError {
    // A deadlock or a serialization failure is the database's way of saying "again".
    if let sqlx::Error::Database(d) = &e
        && matches!(d.code().as_deref(), Some("40P01" | "40001"))
    {
        tracing::warn!("database (parties): {e}");
        return HubError::Busy;
    }
    tracing::error!("database (parties): {e}");
    HubError::Internal
}

fn refuse<T>(words: impl Into<String>) -> Result<T, HubError> {
    Err(HubError::Invalid(words.into()))
}

/// Where the hub has a character: the zones that must be told what concerns it (the zone
/// it plays in; for one on its way, the zone it left and the zone it goes to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Whereabouts {
    pub character: CharacterId,
    pub zones: Vec<ZoneId>,
}

/// An invitation that now waits: whom to tell, and by which names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invitation {
    pub to: CharacterId,
    pub to_name: String,
    pub from_name: String,
}

/// What an answer to an invitation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answered {
    Joined(PartyNews),
    /// Whom to tell that it was refused, and who refused.
    Declined(CharacterId, String),
}

/// A line on its way: who hears it and where, the speaker's name, and for a whisper the
/// name of whom it goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub hearers: Vec<Whereabouts>,
    pub from_name: String,
    pub to_name: String,
}

pub struct Parties {
    pool: PgPool,
    /// One change at a time (PARTY.md 3.1).
    writer: tokio::sync::Mutex<()>,
    /// How long a member that left the game is still of its party (`AWAY` outside tests).
    pub away: Duration,
}

struct Who {
    id: CharacterId,
    name: String,
    zones: Vec<ZoneId>,
}

impl Who {
    fn in_game(&self) -> bool {
        !self.zones.is_empty()
    }

    fn whereabouts(&self) -> Whereabouts {
        Whereabouts {
            character: self.id,
            zones: self.zones.clone(),
        }
    }
}

fn who(r: &sqlx::postgres::PgRow) -> Result<Who, HubError> {
    let kind: String = r.try_get("location_kind").map_err(internal)?;
    let at: Option<String> = r.try_get("location_zone").map_err(internal)?;
    let to: Option<String> = r.try_get("transit_to").map_err(internal)?;
    let mut zones = Vec::new();
    if kind != "offline" {
        zones.extend(at);
        if kind == "transit"
            && let Some(to) = to
            && !zones.contains(&to)
        {
            zones.push(to);
        }
    }
    Ok(Who {
        id: r.try_get("id").map_err(internal)?,
        name: r.try_get("name").map_err(internal)?,
        zones,
    })
}

/// The columns `who` reads, of the characters that match (a literal: the database is
/// never handed a string that was put together).
macro_rules! who_sql {
    ($filter:literal) => {
        concat!(
            "select id, name, location_kind, location_zone, transit_to from characters where ",
            $filter
        )
    };
}

/// A character by a name as a person writes it: names are kept apart by their skeleton.
async fn by_name(tx: &mut Tx<'_>, name: &str) -> Result<Option<Who>, HubError> {
    sqlx::query(who_sql!("name_key = $1"))
        .bind(skeleton(name))
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .map(|r| who(&r))
        .transpose()
}

/// The asker: a character that plays in the zone that asks for it.
async fn here(tx: &mut Tx<'_>, id: CharacterId, zone: &ZoneId) -> Result<Who, HubError> {
    let r = sqlx::query(who_sql!(
        "id = $1 and location_kind = 'zone' and location_zone = $2"
    ))
    .bind(id)
    .bind(zone)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    .ok_or(HubError::Unauthorized)?;
    who(&r)
}

/// The party a character is in, and who leads it.
async fn membership(
    tx: &mut Tx<'_>,
    id: CharacterId,
) -> Result<Option<(i64, CharacterId)>, HubError> {
    sqlx::query(
        "select p.id, p.leader_id from party_members m join parties p on p.id = m.party_id \
         where m.character_id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    .map(|r| {
        Ok((
            r.try_get("id").map_err(internal)?,
            r.try_get("leader_id").map_err(internal)?,
        ))
    })
    .transpose()
}

/// A party's members where the hub has them, in the order they joined.
async fn members(tx: &mut Tx<'_>, party: i64) -> Result<Vec<Who>, HubError> {
    sqlx::query(
        "select c.id, c.name, c.location_kind, c.location_zone, c.transit_to from party_members m \
         join characters c on c.id = m.character_id where m.party_id = $1 \
         order by m.joined, m.character_id",
    )
    .bind(party)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?
    .iter()
    .map(who)
    .collect()
}

async fn next_seq(tx: &mut Tx<'_>) -> Result<i64, HubError> {
    sqlx::query("select nextval('party_seq') as seq")
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?
        .try_get("seq")
        .map_err(internal)
}

/// Write the number of a change to the rows of the characters that joined or left by it.
/// Their rows are taken in ascending order, as the economy takes characters' rows (a
/// trade's accept holds two of them).
async fn stamp(tx: &mut Tx<'_>, characters: &[CharacterId], seq: i64) -> Result<(), HubError> {
    let mut ids = characters.to_vec();
    ids.sort_unstable();
    ids.dedup();
    sqlx::query("select id from characters where id = any($1) order by id for update")
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
    sqlx::query("update characters set party_seq = $2 where id = any($1)")
        .bind(&ids)
        .bind(seq)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    Ok(())
}

impl Parties {
    pub fn new(pool: PgPool) -> Parties {
        Parties {
            pool,
            writer: tokio::sync::Mutex::new(()),
            away: AWAY,
        }
    }

    /// Become the one writer, then begin a transaction.
    async fn change(&self) -> Result<(tokio::sync::MutexGuard<'_, ()>, Tx<'_>), HubError> {
        let writer = self.writer.lock().await;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(LOCK)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        Ok((writer, tx))
    }

    /// What the hub holds of one character's party, in one statement (one view of the
    /// tables): its party's number and state, or its own number and nothing.
    pub async fn reading(&self, character: CharacterId) -> Result<PartyReading, HubError> {
        let rows = sqlx::query(
            "select c.party_seq, p.id as party, p.leader_id, p.seq, mm.character_id, cc.name \
             from characters c \
             left join party_members m on m.character_id = c.id \
             left join parties p on p.id = m.party_id \
             left join party_members mm on mm.party_id = p.id \
             left join characters cc on cc.id = mm.character_id \
             where c.id = $1 order by mm.joined, mm.character_id",
        )
        .bind(character)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let first = rows.first().ok_or(HubError::NotFound)?;
        let party: Option<i64> = first.try_get("party").map_err(internal)?;
        let Some(id) = party else {
            let seq: i64 = first.try_get("party_seq").map_err(internal)?;
            return Ok(PartyReading {
                seq: seq as u64,
                party: None,
            });
        };
        let seq: i64 = first.try_get("seq").map_err(internal)?;
        let mut members = Vec::with_capacity(rows.len());
        for r in &rows {
            members.push((
                r.try_get("character_id").map_err(internal)?,
                r.try_get("name").map_err(internal)?,
            ));
        }
        Ok(PartyReading {
            seq: seq as u64,
            party: Some(PartyState {
                id,
                leader: first.try_get("leader_id").map_err(internal)?,
                members,
            }),
        })
    }

    /// The number of a character's present reading, for a zone to hold its own against
    /// (PARTY.md 3.2, the repair).
    pub async fn seq_of(&self, character: CharacterId) -> Result<u64, HubError> {
        let seq: i64 = sqlx::query(
            "select coalesce(p.seq, c.party_seq) as seq from characters c \
             left join party_members m on m.character_id = c.id \
             left join parties p on p.id = m.party_id where c.id = $1",
        )
        .bind(character)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or(HubError::NotFound)?
        .try_get("seq")
        .map_err(internal)?;
        Ok(seq as u64)
    }

    /// A zone asks for the reading of a character that plays in it.
    pub async fn read(
        &self,
        character: CharacterId,
        zone: &ZoneId,
    ) -> Result<PartyReading, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        here(&mut tx, character, zone).await?;
        drop(tx);
        self.reading(character).await
    }

    /// The zones to tell about these characters, as of now: read after a change is
    /// committed, so that whoever moved while it was made is told where it is.
    pub async fn zones_of(&self, characters: &[CharacterId]) -> Result<Vec<ZoneId>, HubError> {
        let rows = sqlx::query(who_sql!("id = any($1)"))
            .bind(characters)
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?;
        let mut zones = Vec::new();
        for r in &rows {
            zones.extend(who(r)?.zones);
        }
        zones.sort();
        zones.dedup();
        Ok(zones)
    }

    /// `from`, playing in `zone`, invites the character called `to` (PARTY.md 3.2).
    pub async fn invite(
        &self,
        from: CharacterId,
        zone: &ZoneId,
        to: &str,
    ) -> Result<Invitation, HubError> {
        let (_writer, mut tx) = self.change().await?;
        let asker = here(&mut tx, from, zone).await?;
        let nobody = || refuse(format!("nobody called {to} is in the game"));
        let Some(target) = by_name(&mut tx, to).await? else {
            return nobody();
        };
        if target.id == from {
            return refuse("that is you");
        }
        if !target.in_game() {
            return nobody();
        }
        let mut size = 1;
        if let Some((party, leader)) = membership(&mut tx, from).await? {
            if leader != from {
                return refuse("only the party's leader invites");
            }
            let in_it = members(&mut tx, party).await?;
            if in_it.iter().any(|m| m.id == target.id) {
                return refuse(format!("{} is in the party already", target.name));
            }
            size = in_it.len();
        }
        if size >= PARTY_MAX {
            return refuse("the party is full");
        }
        // The invitations of the last minute that concern either of the two: those that
        // wait for the target, and those the asker has out.
        let recent = sqlx::query(
            "select to_id, from_id, declined from party_invites \
             where (to_id = $1 or from_id = $2) and at > now() - make_interval(secs => $3)",
        )
        .bind(target.id)
        .bind(from)
        .bind(INVITE_SECS as f64)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        let (mut waiting, mut out) = (0, 0);
        for r in &recent {
            let (to_id, from_id, declined): (i64, i64, bool) = (
                r.try_get("to_id").map_err(internal)?,
                r.try_get("from_id").map_err(internal)?,
                r.try_get("declined").map_err(internal)?,
            );
            if to_id == target.id && from_id == from {
                // Answered or not: not again within its minute.
                return refuse(format!("{} has been asked already", target.name));
            }
            if !declined {
                waiting += (to_id == target.id) as i64;
                out += (from_id == from) as usize;
            }
        }
        if waiting >= INVITES_WAITING {
            return refuse(format!("{} has too many invitations waiting", target.name));
        }
        // An invitation is for a place there is.
        if size + out >= PARTY_MAX {
            return refuse("as many are asked as the party has room for: wait for an answer");
        }
        sqlx::query(
            "insert into party_invites (to_id, from_id) values ($1, $2) \
             on conflict (to_id, from_id) do update set at = now(), declined = false",
        )
        .bind(target.id)
        .bind(from)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(Invitation {
            to: target.id,
            to_name: target.name,
            from_name: asker.name,
        })
    }

    /// `character`, playing in `zone`, answers the invitation of the character called
    /// `from`: it joins that one's party, or it does not.
    pub async fn answer(
        &self,
        character: CharacterId,
        zone: &ZoneId,
        from: &str,
        join: bool,
    ) -> Result<Answered, HubError> {
        let (_writer, mut tx) = self.change().await?;
        let me = here(&mut tx, character, zone).await?;
        let gone = || refuse("that invitation is gone");
        let Some(inviter) = by_name(&mut tx, from).await? else {
            return gone();
        };
        let waits = sqlx::query(
            "select 1 as one from party_invites where to_id = $1 and from_id = $2 \
             and not declined and at > now() - make_interval(secs => $3)",
        )
        .bind(character)
        .bind(inviter.id)
        .bind(INVITE_SECS as f64)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .is_some();
        if !waits {
            return gone();
        }
        if !join {
            // It stays, declined, until its minute is over: the same one cannot ask
            // again at once, and it holds none of the places.
            sqlx::query(
                "update party_invites set declined = true where to_id = $1 and from_id = $2",
            )
            .bind(character)
            .bind(inviter.id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            tx.commit().await.map_err(internal)?;
            return Ok(Answered::Declined(inviter.id, me.name));
        }
        // A refusal from here leaves the invitation where it was.
        if membership(&mut tx, character).await?.is_some() {
            return refuse("leave your party first");
        }
        if !inviter.in_game() {
            return refuse(format!("{} is not in the game any more", inviter.name));
        }
        let party = match membership(&mut tx, inviter.id).await? {
            Some((_, leader)) if leader != inviter.id => return gone(),
            Some((party, _)) => {
                if members(&mut tx, party).await?.len() >= PARTY_MAX {
                    return refuse("the party is full");
                }
                party
            }
            None => {
                // The first to join makes it a party: the inviter leads, and stands first.
                let first = next_seq(&mut tx).await?;
                let party: i64 = sqlx::query(
                    "insert into parties (leader_id, seq) values ($1, $2) returning id",
                )
                .bind(inviter.id)
                .bind(first)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?
                .try_get("id")
                .map_err(internal)?;
                Self::add(&mut tx, party, inviter.id, first).await?;
                party
            }
        };
        sqlx::query("delete from party_invites where to_id = $1 and from_id = $2")
            .bind(character)
            .bind(inviter.id)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        let seq = next_seq(&mut tx).await?;
        Self::add(&mut tx, party, character, seq).await?;
        // (Read by the party's number from here; its own row says when it last changed.)
        stamp(&mut tx, &[character], seq).await?;
        let news = Self::settle(&mut tx, party, seq, Vec::new()).await?;
        tx.commit().await.map_err(internal)?;
        Ok(Answered::Joined(news))
    }

    /// (Somebody who is in a party is read by the party's number: its own is written
    /// when it is in none again.)
    async fn add(
        tx: &mut Tx<'_>,
        party: i64,
        character: CharacterId,
        seq: i64,
    ) -> Result<(), HubError> {
        sqlx::query(
            "insert into party_members (character_id, party_id, joined) values ($1, $2, $3)",
        )
        .bind(character)
        .bind(party)
        .bind(seq)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        Ok(())
    }

    /// After a change: the party's number, a leader that is in it (and, of those, one
    /// that is in the game when there is one), and no party of one. `left`: who is out of
    /// it by this change. Returns the news.
    async fn settle(
        tx: &mut Tx<'_>,
        party: i64,
        seq: i64,
        mut left: Vec<CharacterId>,
    ) -> Result<PartyNews, HubError> {
        let mut in_it = members(tx, party).await?;
        if in_it.len() < 2 {
            // A party of one is no party.
            left.extend(in_it.drain(..).map(|last| last.id));
            sqlx::query("delete from parties where id = $1")
                .bind(party)
                .execute(&mut **tx)
                .await
                .map_err(internal)?;
        }
        if !left.is_empty() {
            stamp(tx, &left, seq).await?;
        }
        let mut leader: CharacterId = 0;
        if !in_it.is_empty() {
            leader = sqlx::query("select leader_id from parties where id = $1")
                .bind(party)
                .fetch_one(&mut **tx)
                .await
                .map_err(internal)?
                .try_get("leader_id")
                .map_err(internal)?;
            let leads = in_it.iter().find(|m| m.id == leader);
            if !leads.is_some_and(Who::in_game) {
                // Whoever of those in the game has been in it longest leads; when nobody
                // is in the game, whoever has been in it longest.
                let next = in_it.iter().find(|m| m.in_game()).or(in_it.first());
                leader = next.map_or(leader, |m| m.id);
            }
            sqlx::query("update parties set seq = $2, leader_id = $3 where id = $1")
                .bind(party)
                .bind(seq)
                .bind(leader)
                .execute(&mut **tx)
                .await
                .map_err(internal)?;
        }
        Ok(PartyNews {
            seq: seq as u64,
            party: PartyState {
                id: party,
                leader,
                members: in_it.into_iter().map(|m| (m.id, m.name)).collect(),
            },
            left,
        })
    }

    /// Take one member out, within a change.
    async fn drop_member(
        tx: &mut Tx<'_>,
        party: i64,
        character: CharacterId,
    ) -> Result<PartyNews, HubError> {
        let seq = next_seq(tx).await?;
        sqlx::query("delete from party_members where character_id = $1")
            .bind(character)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        Self::settle(tx, party, seq, vec![character]).await
    }

    /// `character`, playing in `zone`, leaves its party.
    pub async fn leave(
        &self,
        character: CharacterId,
        zone: &ZoneId,
    ) -> Result<PartyNews, HubError> {
        let (_writer, mut tx) = self.change().await?;
        here(&mut tx, character, zone).await?;
        let Some((party, _)) = membership(&mut tx, character).await? else {
            return refuse("you are in no party");
        };
        let news = Self::drop_member(&mut tx, party, character).await?;
        tx.commit().await.map_err(internal)?;
        Ok(news)
    }

    /// `leader`, playing in `zone`, takes the member called `name` out of its party.
    pub async fn remove(
        &self,
        leader: CharacterId,
        zone: &ZoneId,
        name: &str,
    ) -> Result<PartyNews, HubError> {
        let (_writer, mut tx) = self.change().await?;
        here(&mut tx, leader, zone).await?;
        let Some((party, led_by)) = membership(&mut tx, leader).await? else {
            return refuse("you are in no party");
        };
        if led_by != leader {
            return refuse("only the party's leader removes");
        }
        let key = skeleton(name);
        let Some(target) = members(&mut tx, party)
            .await?
            .into_iter()
            .find(|m| skeleton(&m.name) == key)
        else {
            return refuse(format!("nobody called {name} is in the party"));
        };
        if target.id == leader {
            return refuse("that is you: leave instead");
        }
        let news = Self::drop_member(&mut tx, party, target.id).await?;
        tx.commit().await.map_err(internal)?;
        Ok(news)
    }

    /// The sweep (PARTY.md 3.2): a party whose leader is out of the game is led by
    /// somebody who is in it; a member that has been out of the game longer than `away`
    /// is out of its party; an invitation past its time is forgotten. One change each;
    /// what the database refuses in the middle ends this sweep (the next one takes it up
    /// again), and what was changed before it is told.
    pub async fn sweep(&self) -> Result<Vec<PartyNews>, HubError> {
        sqlx::query("delete from party_invites where at <= now() - make_interval(secs => $1)")
            .bind(INVITE_SECS as f64)
            .execute(&self.pool)
            .await
            .map_err(internal)?;
        let mut news = Vec::new();
        // Leaders that are out of the game while somebody of the party is in it.
        let led_from_afar: Vec<i64> = sqlx::query(
            "select p.id from parties p join characters c on c.id = p.leader_id \
             where c.location_kind = 'offline' and exists (select 1 from party_members m \
             join characters mc on mc.id = m.character_id \
             where m.party_id = p.id and mc.location_kind <> 'offline') order by p.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?
        .iter()
        .map(|r| r.try_get("id").map_err(internal))
        .collect::<Result<_, _>>()?;
        for party in led_from_afar {
            match self.pass_lead(party).await {
                Ok(Some(told)) => news.push(told),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("party sweep: passing the lead of party {party}: {e:?}");
                    return Ok(news);
                }
            }
        }
        // Members that have been out of the game too long.
        let away: Vec<(i64, CharacterId)> = sqlx::query(
            "select m.party_id, m.character_id from party_members m \
             join characters c on c.id = m.character_id where c.location_kind = 'offline' \
             and c.offline_since < now() - make_interval(secs => $1) order by m.party_id, m.character_id",
        )
        .bind(self.away.as_secs_f64())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?
        .iter()
        .map(|r| {
            Ok((
                r.try_get("party_id").map_err(internal)?,
                r.try_get("character_id").map_err(internal)?,
            ))
        })
        .collect::<Result<_, HubError>>()?;
        for (party, character) in away {
            match self.let_go(party, character).await {
                Ok(Some(told)) => news.push(told),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("party sweep: letting go of {character}: {e:?}");
                    return Ok(news);
                }
            }
        }
        Ok(news)
    }

    /// One change of the sweep: the lead of `party`, whose leader was out of the game, to
    /// somebody who is in it. Nothing, when it came back meanwhile.
    async fn pass_lead(&self, party: i64) -> Result<Option<PartyNews>, HubError> {
        let (_writer, mut tx) = self.change().await?;
        let before: Option<CharacterId> =
            sqlx::query("select leader_id from parties where id = $1")
                .bind(party)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?
                .map(|r| r.try_get("leader_id").map_err(internal))
                .transpose()?;
        let Some(before) = before else {
            return Ok(None);
        };
        let seq = next_seq(&mut tx).await?;
        let told = Self::settle(&mut tx, party, seq, Vec::new()).await?;
        if told.party.leader == before {
            // It came back meanwhile: nothing changed, and nothing is said.
            return Ok(None);
        }
        tx.commit().await.map_err(internal)?;
        Ok(Some(told))
    }

    /// One change of the sweep: `character`, out of the game for too long, out of `party`.
    /// Nothing, when it came back meanwhile, or the party is gone.
    async fn let_go(
        &self,
        party: i64,
        character: CharacterId,
    ) -> Result<Option<PartyNews>, HubError> {
        let (_writer, mut tx) = self.change().await?;
        // The rows of everybody in the party, in ascending order (the change may dissolve
        // it, and stamp the last one left): the order every multi-row lock on characters
        // takes, so that nothing waits on this sweep while it waits on them.
        sqlx::query(
            "select c.id from characters c join party_members m on m.character_id = c.id \
             where m.party_id = $1 order by c.id for update of c",
        )
        .bind(party)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        // Looked at again, under its row's lock: somebody who came back meanwhile, or
        // is coming back this moment, stays.
        let still = sqlx::query(
            "select 1 as one from characters where id = $1 and location_kind = 'offline' \
             and offline_since < now() - make_interval(secs => $2)",
        )
        .bind(character)
        .bind(self.away.as_secs_f64())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .is_some();
        // (An earlier one of this sweep may have dissolved the party under it.)
        if !still || membership(&mut tx, character).await?.map(|m| m.0) != Some(party) {
            return Ok(None);
        }
        let told = Self::drop_member(&mut tx, party, character).await?;
        tx.commit().await.map_err(internal)?;
        Ok(Some(told))
    }

    /// A line of `from`, playing in `zone`: who hears it, and where they are.
    pub async fn line(
        &self,
        from: CharacterId,
        zone: &ZoneId,
        to: &SayTo,
    ) -> Result<Line, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let speaker = here(&mut tx, from, zone).await?;
        let line = match to {
            SayTo::Party => {
                let Some((party, _)) = membership(&mut tx, from).await? else {
                    return refuse("you are in no party");
                };
                Line {
                    hearers: members(&mut tx, party)
                        .await?
                        .iter()
                        .filter(|m| m.in_game())
                        .map(Who::whereabouts)
                        .collect(),
                    from_name: speaker.name,
                    to_name: String::new(),
                }
            }
            SayTo::Whisper(name) => {
                let target = by_name(&mut tx, name).await?.filter(|t| t.in_game());
                let Some(target) = target else {
                    return refuse(format!("nobody called {name} is in the game"));
                };
                if target.id == from {
                    return refuse("that is you");
                }
                Line {
                    hearers: vec![target.whereabouts()],
                    from_name: speaker.name,
                    to_name: target.name,
                }
            }
        };
        tx.commit().await.map_err(internal)?;
        Ok(line)
    }
}
