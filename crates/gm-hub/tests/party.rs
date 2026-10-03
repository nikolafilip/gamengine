//! Parties of people over the hub protocol (PARTY.md 2, 3, 5): asked through the zone a
//! character plays in, told to every other zone it concerns, numbered so that a zone
//! holds what the hub holds in whatever order it hears it. Needs `GM_TEST_DATABASE_URL`;
//! without it the test is skipped.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::tick::TickRate;
use gm_hub::party::Parties;
use gm_hub::protocol::{
    BuildChoice, CHANNEL_PARTY, CHANNEL_WHISPER, CharacterId, CharacterState, HubError, HubNotice,
    HubRequest, HubResponse, PARTY_MAX, PartyNews, PartyReading, PartyReply, SayTo, SessionId,
    SessionToken, ZonePartyOp,
};
use gm_hub::{Db, HubClient, HubClientError, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config};

const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "party-test-secret";

/// A zone as the hub sees one: its connection, and every notice it was sent.
struct Zone {
    id: &'static str,
    conn: Arc<HubClient>,
    notices: Arc<Mutex<Vec<HubNotice>>>,
}

impl Zone {
    async fn register(addr: std::net::SocketAddr, cert: &[u8], id: &'static str) -> Zone {
        let conn = Arc::new(
            HubClient::connect_with_cert(addr, cert.to_vec())
                .await
                .unwrap(),
        );
        let r = conn
            .request(&HubRequest::ZoneHello {
                secret: SECRET.into(),
                zone: id.into(),
                map: "test".into(),
                map_hash: 0,
                addr: "127.0.0.1:9".parse().unwrap(),
                cert_der: vec![1, 2, 3],
                web: None,
                min_trust: 0,
                requires: Vec::new(),
                max_players: 64,
            })
            .await
            .unwrap();
        assert!(matches!(r, HubResponse::Registered { .. }), "{r:?}");
        let notices = Arc::new(Mutex::new(Vec::new()));
        let (reader, store) = (conn.clone(), notices.clone());
        tokio::spawn(async move {
            while let Some(n) = reader.notice().await {
                store.lock().unwrap().push(n);
            }
        });
        Zone { id, conn, notices }
    }

    async fn ask(&self, op: ZonePartyOp) -> Result<PartyReply, HubError> {
        match self.conn.request(&HubRequest::ZoneParty(op)).await {
            Ok(HubResponse::Party(r)) => Ok(r),
            Err(HubClientError::Refused(e)) => Err(e),
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn invite(&self, from: CharacterId, to: &str) -> Result<PartyReply, HubError> {
        self.ask(ZonePartyOp::Invite {
            from,
            to: to.into(),
        })
        .await
    }

    async fn join(&self, character: CharacterId, from: &str) -> Result<PartyNews, HubError> {
        let op = ZonePartyOp::Answer {
            character,
            from: from.into(),
            join: true,
        };
        match self.ask(op).await? {
            PartyReply::News(news) => Ok(news),
            other => panic!("join: {other:?}"),
        }
    }

    async fn leave(&self, character: CharacterId) -> Result<PartyNews, HubError> {
        match self.ask(ZonePartyOp::Leave { character }).await? {
            PartyReply::News(news) => Ok(news),
            other => panic!("leave: {other:?}"),
        }
    }

    async fn remove(&self, leader: CharacterId, name: &str) -> Result<PartyNews, HubError> {
        let op = ZonePartyOp::Remove {
            leader,
            name: name.into(),
        };
        match self.ask(op).await? {
            PartyReply::News(news) => Ok(news),
            other => panic!("remove: {other:?}"),
        }
    }

    async fn say(&self, from: CharacterId, to: SayTo, text: &str) -> Result<String, HubError> {
        let op = ZonePartyOp::Say {
            from,
            to,
            text: text.into(),
        };
        match self.ask(op).await? {
            PartyReply::Said { to } => Ok(to),
            other => panic!("say: {other:?}"),
        }
    }

    /// The first notice that `want` takes, within two seconds; it is taken out.
    async fn hears<T>(&self, want: impl Fn(&HubNotice) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            {
                let mut all = self.notices.lock().unwrap();
                if let Some((i, found)) = all
                    .iter()
                    .enumerate()
                    .find_map(|(i, n)| want(n).map(|t| (i, t)))
                {
                    all.remove(i);
                    return found;
                }
            }
            assert!(
                Instant::now() < until,
                "{}: no such notice among {:?}",
                self.id,
                self.notices.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn hears_news(&self, seq: u64) -> PartyNews {
        self.hears(|n| match n {
            HubNotice::Party(news) if news.seq == seq => Some(news.clone()),
            _ => None,
        })
        .await
    }

    /// Nothing about parties or lines is waiting (after a moment for what is on its way).
    async fn quiet(&self) {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let all = self.notices.lock().unwrap();
        let about: Vec<&HubNotice> = all
            .iter()
            .filter(|n| !matches!(n, HubNotice::Claimed { .. }))
            .collect();
        assert!(about.is_empty(), "{}: {about:?}", self.id);
    }

    /// Hand a character of this zone to `to` and have that zone claim it: what the claim
    /// said of its party.
    async fn hand(
        &self,
        character: CharacterId,
        state: &CharacterState,
        to: &Zone,
    ) -> PartyReading {
        let ticket = self.ticket(character, state, to).await;
        to.claim(ticket).await
    }

    async fn ticket(
        &self,
        character: CharacterId,
        state: &CharacterState,
        to: &Zone,
    ) -> SessionToken {
        let req = HubRequest::Handoff {
            character,
            state: state.clone(),
            to_zone: to.id.into(),
            web: false,
        };
        match self.conn.request(&req).await.unwrap() {
            HubResponse::Ticket(t) => t.token,
            other => panic!("handoff: {other:?}"),
        }
    }

    async fn claim(&self, token: SessionToken) -> PartyReading {
        match self
            .conn
            .request(&HubRequest::Claim { token })
            .await
            .unwrap()
        {
            HubResponse::Claimed { party, .. } => party,
            other => panic!("claim: {other:?}"),
        }
    }

    /// The character leaves the game from this zone.
    async fn gone(&self, character: CharacterId, state: &CharacterState) {
        let req = HubRequest::Save {
            character,
            state: state.clone(),
            leaving: true,
        };
        assert!(matches!(self.conn.request(&req).await, Ok(HubResponse::Ok)));
    }
}

struct Person {
    client: HubClient,
    session: SessionId,
    id: CharacterId,
    state: CharacterState,
}

/// Register, make a character and, with a zone, enter it.
async fn person(
    addr: std::net::SocketAddr,
    cert: &[u8],
    name: &str,
    zone: Option<&Zone>,
) -> Person {
    let client = HubClient::connect_with_cert(addr, cert.to_vec())
        .await
        .unwrap();
    let HubResponse::Session { session, .. } = client
        .request(&HubRequest::Register {
            email: format!("{name}@example.test"),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap()
    else {
        panic!("register")
    };
    let HubResponse::Character(c) = client
        .request(&HubRequest::CreateCharacter {
            session,
            name: name.into(),
            build: BuildChoice::Preset("ironclad".into()),
        })
        .await
        .unwrap()
    else {
        panic!("create")
    };
    let mut p = Person {
        client,
        session,
        id: c.id,
        state: fresh(c.build.clone()),
    };
    if let Some(zone) = zone {
        enter(&mut p, zone).await;
    }
    p
}

/// The state of a character that stands nowhere yet.
fn fresh(build: gm_core::build::Build) -> CharacterState {
    CharacterState {
        build,
        zone: None,
        position: [0.0; 3],
        yaw: 0.0,
        viewport: 0,
        play_seconds: 0,
    }
}

async fn enter(p: &mut Person, zone: &Zone) -> PartyReading {
    let req = HubRequest::Enter {
        session: p.session,
        character: p.id,
        zone: zone.id.into(),
    };
    let HubResponse::Ticket(ticket) = p.client.request(&req).await.unwrap() else {
        panic!("enter")
    };
    let req = HubRequest::Claim {
        token: ticket.token,
    };
    match zone.conn.request(&req).await.unwrap() {
        HubResponse::Claimed { state, party, .. } => {
            p.state = state;
            party
        }
        other => panic!("claim: {other:?}"),
    }
}

fn config(content: gm_core::build::ContentPack, sweep: Duration, tag: &str) -> HubConfig {
    HubConfig {
        zone_secret: SECRET.into(),
        content,
        key: HubKey::generate(),
        session_secs: 3600,
        auth_per_minute: 1000.0,
        econ_per_second: 1000.0,
        party_sweep: sweep,
        party_away: Duration::from_millis(600),
        items: gm_content::items::load_items(Path::new(CONTENT)).expect("items"),
        max_coin_grant: 500,
        models_dir: std::env::temp_dir().join(format!("gm-hub-party-{tag}-{}", std::process::id())),
        ingest: gm_hub::IngestMode::InProcess,
        ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
        start_zone: None,
        blurbs: Vec::new(),
    }
}

async fn hub(db: &Db, sweep: Duration, tag: &str) -> (std::net::SocketAddr, Vec<u8>) {
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"]).unwrap();
    let cert = identity.cert_der().to_vec();
    let endpoint = quinn::Endpoint::server(
        hub_server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    tokio::spawn(gm_hub::run(
        config(content, sweep, tag),
        db.clone(),
        endpoint,
        std::future::pending(),
    ));
    (addr, cert)
}

fn refused<T: std::fmt::Debug>(r: Result<T, HubError>, words: &str) {
    match r {
        Err(HubError::Invalid(why)) => assert_eq!(why, words),
        other => panic!("expected {words:?}, got {other:?}"),
    }
}

fn names(news: &PartyNews) -> Vec<&str> {
    news.party.members.iter().map(|(_, n)| n.as_str()).collect()
}

/// What a zone keeps (PARTY.md 3.1): for each character the reading with the largest
/// number, whatever order it hears them in.
#[derive(Default)]
struct Kept(HashMap<CharacterId, PartyReading>);

impl Kept {
    fn reading(&mut self, character: CharacterId, reading: &PartyReading) {
        let kept = self.0.entry(character).or_default();
        if reading.seq > kept.seq {
            *kept = reading.clone();
        }
    }

    fn news(&mut self, news: &PartyNews) {
        for &(member, _) in &news.party.members {
            let reading = PartyReading {
                seq: news.seq,
                party: Some(news.party.clone()),
            };
            self.reading(member, &reading);
        }
        for &gone in &news.left {
            let reading = PartyReading {
                seq: news.seq,
                party: None,
            };
            self.reading(gone, &reading);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_party_is_the_hub_s_and_every_zone_holds_what_the_hub_holds() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    // The sweep is the test's own to call here; the hub's is tried at the end.
    let (addr, cert) = hub(&db, Duration::from_secs(3600), "a").await;
    // (A member that left the game is out at the next sweep here: the two minutes it
    // has in production are set where they are tried.)
    let mut direct = Parties::new(db.pool().clone());
    direct.away = Duration::ZERO;

    let town = Zone::register(addr, &cert, "town").await;
    let dungeon = Zone::register(addr, &cert, "dungeon").await;
    let ana = person(addr, &cert, "Ana", Some(&town)).await;
    let mut bojan = person(addr, &cert, "Bojan", Some(&town)).await;
    let cvita = person(addr, &cert, "Cvita", Some(&town)).await;
    let dane = person(addr, &cert, "Dane", Some(&town)).await;
    let ema = person(addr, &cert, "Ema", Some(&town)).await;
    let filip = person(addr, &cert, "Filip", Some(&town)).await;
    let mut away = person(addr, &cert, "Away", None).await;
    let nobody = PartyReading::default();
    assert_eq!(direct.reading(ana.id).await.unwrap(), nobody);

    // --- Who can be invited, and by whom.
    refused(
        town.invite(ana.id, "Nobody").await,
        "nobody called Nobody is in the game",
    );
    refused(
        town.invite(ana.id, "Away").await,
        "nobody called Away is in the game",
    );
    refused(town.invite(ana.id, "ana").await, "that is you");
    // A zone speaks for the characters that play in it, and for no others.
    assert_eq!(
        dungeon.invite(ana.id, "Bojan").await,
        Err(HubError::Unauthorized)
    );
    assert_eq!(
        town.invite(away.id, "Bojan").await,
        Err(HubError::Unauthorized)
    );
    // A name is found as a person writes it.
    assert_eq!(
        town.invite(ana.id, "bojan").await,
        Ok(PartyReply::Invited {
            name: "Bojan".into()
        })
    );
    let invited = |to: CharacterId| {
        move |n: &HubNotice| match n {
            HubNotice::Invited { to: t, from } if *t == to => Some(from.clone()),
            _ => None,
        }
    };
    assert_eq!(town.hears(invited(bojan.id)).await, "Ana");
    refused(
        town.invite(ana.id, "Bojan").await,
        "Bojan has been asked already",
    );

    // --- An answer: to an invitation that is there, once.
    refused(
        town.join(bojan.id, "Cvita").await,
        "that invitation is gone",
    );
    refused(town.join(cvita.id, "Ana").await, "that invitation is gone");
    let made = town.join(bojan.id, "Ana").await.unwrap();
    assert_eq!(names(&made), ["Ana", "Bojan"]);
    assert_eq!((made.party.leader, made.left.len()), (ana.id, 0));
    assert!(made.seq > 0);
    refused(town.join(bojan.id, "Ana").await, "that invitation is gone");
    // The asking zone has it in its answer, and is sent the notice too: an answer may be
    // late or lost, and the notice is the same news with the same number.
    assert_eq!(town.hears_news(made.seq).await, made);
    town.quiet().await;
    let reading = |news: &PartyNews| PartyReading {
        seq: news.seq,
        party: Some(news.party.clone()),
    };
    assert_eq!(direct.reading(ana.id).await.unwrap(), reading(&made));
    assert_eq!(direct.reading(bojan.id).await.unwrap(), reading(&made));
    refused(
        town.invite(ana.id, "Bojan").await,
        "Bojan is in the party already",
    );

    // --- A claim carries it; a zone elsewhere is told of what changes.
    assert_eq!(
        town.hand(bojan.id, &bojan.state, &dungeon).await,
        reading(&made)
    );
    town.invite(ana.id, "Cvita").await.unwrap();
    assert_eq!(town.hears(invited(cvita.id)).await, "Ana");
    let three = town.join(cvita.id, "Ana").await.unwrap();
    assert_eq!(names(&three), ["Ana", "Bojan", "Cvita"]);
    assert!(three.seq > made.seq);
    assert_eq!(dungeon.hears_news(three.seq).await, three);
    assert_eq!(town.hears_news(three.seq).await, three);
    town.quiet().await;
    // Only the leader invites, wherever the others are.
    refused(
        dungeon.invite(bojan.id, "Dane").await,
        "only the party's leader invites",
    );

    // --- Five at most, and an invitation is for a place there is: with three in the
    // party, two may be asked.
    for who in ["Dane", "Ema"] {
        town.invite(ana.id, who).await.unwrap();
    }
    refused(
        town.invite(ana.id, "Filip").await,
        "as many are asked as the party has room for: wait for an answer",
    );
    town.join(dane.id, "Ana").await.unwrap();
    let five = town.join(ema.id, "Ana").await.unwrap();
    assert_eq!(five.party.members.len(), PARTY_MAX);
    refused(town.invite(ana.id, "Filip").await, "the party is full");
    refused(
        town.invite(ana.id, "Away").await,
        "nobody called Away is in the game",
    );
    let four = town.leave(cvita.id).await.unwrap();
    assert_eq!(
        (names(&four), &four.left[..]),
        (vec!["Ana", "Bojan", "Dane", "Ema"], &[cvita.id][..])
    );
    assert_eq!(
        direct.reading(cvita.id).await.unwrap(),
        PartyReading {
            seq: four.seq,
            party: None
        }
    );
    town.invite(ana.id, "Filip").await.unwrap();
    let again = town.join(filip.id, "Ana").await.unwrap();
    assert_eq!(names(&again), ["Ana", "Bojan", "Dane", "Ema", "Filip"]);
    refused(town.leave(cvita.id).await, "you are in no party");
    // Somebody in a party joins no other; the invitation stays where it was.
    town.invite(cvita.id, "Filip").await.unwrap();
    refused(town.join(filip.id, "Cvita").await, "leave your party first");
    refused(
        town.invite(cvita.id, "Filip").await,
        "Filip has been asked already",
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    for zone in [&dungeon, &town] {
        let all: Vec<_> = zone.notices.lock().unwrap().drain(..).collect();
        assert_eq!(
            all.iter()
                .filter(|n| matches!(n, HubNotice::Party(_)))
                .count(),
            4,
            "{} was told of every change of the party: {all:?}",
            zone.id
        );
    }

    // --- Removing: the leader's, by name.
    refused(
        dungeon.remove(bojan.id, "Dane").await,
        "only the party's leader removes",
    );
    refused(town.remove(cvita.id, "Dane").await, "you are in no party");
    refused(
        town.remove(ana.id, "Cvita").await,
        "nobody called Cvita is in the party",
    );
    refused(
        town.remove(ana.id, "Ana").await,
        "that is you: leave instead",
    );
    let removed = town.remove(ana.id, "dane").await.unwrap();
    assert_eq!(
        (names(&removed), &removed.left[..]),
        (vec!["Ana", "Bojan", "Ema", "Filip"], &[dane.id][..])
    );
    assert_eq!(dungeon.hears_news(removed.seq).await, removed);

    // --- The leader leaves: whoever has been in it longest leads.
    let led = town.leave(ana.id).await.unwrap();
    assert_eq!(
        (names(&led), led.party.leader),
        (vec!["Bojan", "Ema", "Filip"], bojan.id)
    );
    assert_eq!(dungeon.hears_news(led.seq).await, led);
    assert_eq!(
        dungeon.invite(bojan.id, "Ana").await,
        Ok(PartyReply::Invited { name: "Ana".into() })
    );
    assert_eq!(town.hears(invited(ana.id)).await, "Bojan");
    // Declined: the inviter's zone is told, and the invitation is gone.
    let no = ZonePartyOp::Answer {
        character: ana.id,
        from: "Bojan".into(),
        join: false,
    };
    assert_eq!(town.ask(no).await, Ok(PartyReply::Declined));
    let declined = dungeon
        .hears(|n| match n {
            HubNotice::Declined { to, by } => Some((*to, by.clone())),
            _ => None,
        })
        .await;
    assert_eq!(declined, (bojan.id, "Ana".to_string()));
    refused(town.join(ana.id, "Bojan").await, "that invitation is gone");
    // And it stays declined for its minute: the same one does not ask again at once,
    // and it holds none of the places that may wait for Ana.
    refused(
        dungeon.invite(bojan.id, "Ana").await,
        "Ana has been asked already",
    );
    let waiting: i64 =
        sqlx::query_scalar("select count(*) from party_invites where to_id = $1 and declined")
            .bind(ana.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(waiting, 1);
    town.notices.lock().unwrap().clear();

    // --- Lines: a party's to its members where they are, a whisper to one.
    let heard = |channel: u8| {
        move |n: &HubNotice| match n {
            HubNotice::Heard {
                to,
                channel: c,
                from,
                text,
            } if *c == channel => Some((to.clone(), from.clone(), text.clone())),
            _ => None,
        }
    };
    assert_eq!(
        town.say(ema.id, SayTo::Party, "  ready?  ").await,
        Ok(String::new())
    );
    let (mut to, from, text) = town.hears(heard(CHANNEL_PARTY)).await;
    to.sort();
    assert_eq!(
        (to, from.as_str(), text.as_str()),
        (vec![ema.id, filip.id], "Ema", "ready?")
    );
    assert_eq!(
        dungeon.hears(heard(CHANNEL_PARTY)).await,
        (vec![bojan.id], "Ema".to_string(), "ready?".to_string())
    );
    assert_eq!(
        town.say(ana.id, SayTo::Whisper("BOJAN".into()), "psst")
            .await,
        Ok("Bojan".into())
    );
    assert_eq!(
        dungeon.hears(heard(CHANNEL_WHISPER)).await,
        (vec![bojan.id], "Ana".to_string(), "psst".to_string())
    );
    town.quiet().await;
    refused(
        town.say(ana.id, SayTo::Party, "hello").await,
        "you are in no party",
    );
    refused(
        town.say(ana.id, SayTo::Whisper("Away".into()), "hello")
            .await,
        "nobody called Away is in the game",
    );
    refused(
        town.say(ana.id, SayTo::Whisper("Ana".into()), "hello")
            .await,
        "that is you",
    );
    refused(
        town.say(ema.id, SayTo::Party, "two\nlines").await,
        "that is not a line",
    );
    refused(
        town.say(ema.id, SayTo::Party, "   ").await,
        "that is not a line",
    );
    assert_eq!(
        dungeon.say(ema.id, SayTo::Party, "hello").await,
        Err(HubError::Unauthorized)
    );
    // On the way between two zones a character is told in both.
    let token = dungeon.ticket(bojan.id, &bojan.state, &town).await;
    town.say(ana.id, SayTo::Whisper("Bojan".into()), "where are you")
        .await
        .unwrap();
    for zone in [&dungeon, &town] {
        assert_eq!(zone.hears(heard(CHANNEL_WHISPER)).await.0, vec![bojan.id]);
    }
    assert_eq!(town.claim(token).await, reading(&led));
    town.notices.lock().unwrap().clear();
    dungeon.notices.lock().unwrap().clear();

    // --- A party of one is no party.
    let two = town.leave(ema.id).await.unwrap();
    assert_eq!(names(&two), ["Bojan", "Filip"]);
    let none = town.leave(filip.id).await.unwrap();
    assert!(none.party.members.is_empty());
    assert_eq!(none.left, [filip.id, bojan.id]);
    for who in [bojan.id, filip.id] {
        assert_eq!(
            direct.reading(who).await.unwrap(),
            PartyReading {
                seq: none.seq,
                party: None
            }
        );
    }
    let parties: i64 = sqlx::query_scalar("select count(*) from parties")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(parties, 0);

    // --- The numbers are in the order of the changes, so a zone that hears them in any
    // order holds what the hub holds. Two leave one party of three at once, over and
    // over, and a claim races a change.
    let mut last = none.seq;
    for round in 0..24 {
        town.invite(ana.id, "Bojan").await.unwrap();
        town.invite(ana.id, "Cvita").await.unwrap();
        let a = town.join(bojan.id, "Ana").await.unwrap();
        let b = town.join(cvita.id, "Ana").await.unwrap();
        assert!(a.seq > last && b.seq > a.seq, "round {round}");
        // Bojan is claimed by the dungeon while the others leave (every other round
        // only the leader leaves, and the round ends in a party of two: an order of
        // hearing that was wrong would show there).
        let token = town.ticket(bojan.id, &bojan.state, &dungeon).await;
        let mut kept = Kept::default();
        let claimed = if round % 2 == 0 {
            let (claimed, x, y) = tokio::join!(
                dungeon.claim(token),
                town.leave(ana.id),
                town.leave(cvita.id)
            );
            let (x, y) = (x.unwrap(), y.unwrap());
            assert_ne!(x.seq, y.seq);
            let (first, second) = if x.seq < y.seq { (&x, &y) } else { (&y, &x) };
            assert_eq!(first.party.members.len(), 2, "round {round}");
            assert!(second.party.members.is_empty(), "round {round}");
            last = second.seq;
            // Heard in the worst order: the last change first, the claim's reading last.
            for news in [second, first, &b, &a] {
                kept.news(news);
            }
            claimed
        } else {
            let (claimed, x) = tokio::join!(dungeon.claim(token), town.leave(ana.id));
            let x = x.unwrap();
            assert_eq!(names(&x), ["Bojan", "Cvita"], "round {round}");
            assert_eq!(x.party.leader, bojan.id, "the longest-standing leads");
            last = x.seq;
            for news in [&x, &b, &a] {
                kept.news(news);
            }
            claimed
        };
        kept.reading(bojan.id, &claimed);
        for who in [ana.id, bojan.id, cvita.id] {
            let truth = direct.reading(who).await.unwrap();
            assert_eq!(kept.0[&who], truth, "round {round}");
            let in_party = round % 2 == 1 && who != ana.id;
            assert_eq!(truth.party.is_some(), in_party, "round {round}");
        }
        if round % 2 == 1 {
            // Dissolved for the next round, from the dungeon.
            let gone = dungeon.leave(bojan.id).await.unwrap();
            assert!(gone.party.members.is_empty());
            last = gone.seq;
        }
        // Bojan back to the town for the next round.
        let r = dungeon.hand(bojan.id, &bojan.state, &town).await;
        assert_eq!(r.party, None, "round {round}");
    }
    town.notices.lock().unwrap().clear();
    dungeon.notices.lock().unwrap().clear();

    // --- A zone that missed a notice sees it at its next save, and asks (the repair).
    town.invite(ana.id, "Bojan").await.unwrap();
    town.invite(ana.id, "Cvita").await.unwrap();
    town.join(bojan.id, "Ana").await.unwrap();
    let trio = town.join(cvita.id, "Ana").await.unwrap();
    let save = HubRequest::Save {
        character: cvita.id,
        state: cvita.state.clone(),
        leaving: false,
    };
    assert_eq!(
        town.conn.request(&save).await.unwrap(),
        HubResponse::Saved { party: trio.seq }
    );
    assert_eq!(
        town.ask(ZonePartyOp::Read {
            character: cvita.id
        })
        .await,
        Ok(PartyReply::Reading(reading(&trio)))
    );
    assert_eq!(
        dungeon
            .ask(ZonePartyOp::Read {
                character: cvita.id
            })
            .await,
        Err(HubError::Unauthorized)
    );

    // --- Away, then out. Somebody who left the game is still of the party for a while
    // (two minutes; an hour here), and one who came back in that time never left it.
    direct.away = Duration::from_secs(3600);
    assert!(
        direct.sweep().await.unwrap().is_empty(),
        "everybody is in the game"
    );
    town.gone(bojan.id, &bojan.state).await;
    assert!(direct.sweep().await.unwrap().is_empty(), "away, not out");
    assert_eq!(enter(&mut bojan, &town).await, reading(&trio), "back");
    // The leader goes: at the next sweep the longest-standing member in the game leads,
    // and the leader is still of the party.
    town.gone(ana.id, &ana.state).await;
    let swept = direct.sweep().await.unwrap();
    assert_eq!(swept.len(), 1, "{swept:?}");
    assert_eq!(names(&swept[0]), ["Ana", "Bojan", "Cvita"]);
    assert_eq!((swept[0].party.leader, swept[0].left.len()), (bojan.id, 0));
    assert!(direct.sweep().await.unwrap().is_empty(), "said once");
    // An invitation past its time goes with the same sweep.
    town.invite(ema.id, "Dane").await.unwrap();
    sqlx::query("update party_invites set at = now() - interval '61 seconds'")
        .execute(db.pool())
        .await
        .unwrap();
    refused(town.join(dane.id, "Ema").await, "that invitation is gone");
    assert!(direct.sweep().await.unwrap().is_empty());
    let invites: i64 = sqlx::query_scalar("select count(*) from party_invites")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(invites, 0);
    // The time is over (no time at all, here): Ana is out. Then Bojan goes too: Cvita
    // leads for a moment, and then is alone, which is no party.
    direct.away = Duration::ZERO;
    let swept = direct.sweep().await.unwrap();
    assert_eq!(swept.len(), 1, "{swept:?}");
    assert_eq!(
        (names(&swept[0]), &swept[0].left[..]),
        (vec!["Bojan", "Cvita"], &[ana.id][..])
    );
    town.gone(bojan.id, &bojan.state).await;
    let swept = direct.sweep().await.unwrap();
    assert_eq!(swept.len(), 2, "{swept:?}");
    assert_eq!((swept[0].party.leader, swept[0].left.len()), (cvita.id, 0));
    assert!(swept[1].party.members.is_empty());
    assert_eq!(swept[1].left, [bojan.id, cvita.id]);
    assert_eq!(
        direct.reading(cvita.id).await.unwrap(),
        PartyReading {
            seq: swept[1].seq,
            party: None
        }
    );
    // Whom to tell is read after the change: Cvita is in the town, the others nowhere.
    assert_eq!(
        direct
            .zones_of(&[ana.id, bojan.id, cvita.id])
            .await
            .unwrap(),
        ["town"]
    );
    // Somebody who enters the game is in no party it was swept out of.
    assert_eq!(enter(&mut away, &town).await, nobody);

    // --- The hub's own sweep tells the zones (a second hub on the same database, with a
    // sweep that runs).
    let (addr, cert) = hub(&db, Duration::from_millis(200), "b").await;
    let meadow = Zone::register(addr, &cert, "meadow").await;
    let cave = Zone::register(addr, &cert, "cave").await;
    let gita = person(addr, &cert, "Gita", Some(&meadow)).await;
    let hana = person(addr, &cert, "Hana", Some(&cave)).await;
    meadow.invite(gita.id, "Hana").await.unwrap();
    let pair = cave.join(hana.id, "Gita").await.unwrap();
    assert_eq!(meadow.hears_news(pair.seq).await, pair);
    // (This hub lets go after 600 ms.)
    cave.gone(hana.id, &hana.state).await;
    let swept = meadow
        .hears(|n| match n {
            HubNotice::Party(news) if news.seq > pair.seq => Some(news.clone()),
            _ => None,
        })
        .await;
    assert!(swept.party.members.is_empty());
    assert_eq!(swept.left, [hana.id, gita.id]);

    // --- A storm: four parties of five made and unmade at once, round after round, with
    // lines said through it all. One writer makes every change (PARTY.md 3.1): nothing
    // deadlocks, nothing is answered Busy, every change is numbered once, and the zone
    // that hears them in whatever order ends up holding what the hub holds. What this
    // measures is how many changes a second that writer makes (PARTY.md 13).
    const PARTIES: usize = 4;
    const ROUNDS: usize = 6;
    let storm: Vec<Vec<Person>> = {
        let mut all = Vec::new();
        for p in 0..PARTIES {
            let mut members = Vec::new();
            for m in 0..PARTY_MAX {
                let name = format!("Storm{p}{m}");
                members.push(person(addr, &cert, &name, Some(&meadow)).await);
            }
            all.push(members);
        }
        all
    };
    let meadow = Arc::new(meadow);
    let began = Instant::now();
    let mut tasks = Vec::new();
    for (p, members) in storm.iter().enumerate() {
        let zone = meadow.clone();
        let ids: Vec<(CharacterId, String)> = members
            .iter()
            .enumerate()
            .map(|(m, person)| (person.id, format!("Storm{p}{m}")))
            .collect();
        tasks.push(tokio::spawn(async move {
            let leader = ids[0].0;
            let mut seqs = Vec::new();
            for _ in 0..ROUNDS {
                for (_, name) in &ids[1..] {
                    zone.invite(leader, name).await.unwrap();
                }
                for (id, _) in &ids[1..] {
                    seqs.push(zone.join(*id, &ids[0].1).await.unwrap().seq);
                }
                zone.say(leader, SayTo::Party, "all here?").await.unwrap();
                for (id, _) in &ids[1..] {
                    seqs.push(zone.leave(*id).await.unwrap().seq);
                }
            }
            seqs
        }));
    }
    let mut seqs: Vec<u64> = Vec::new();
    for t in tasks {
        seqs.extend(t.await.unwrap());
    }
    let secs = began.elapsed().as_secs_f64();
    let changes = PARTIES * ROUNDS * 2 * (PARTY_MAX - 1);
    assert_eq!(seqs.len(), changes);
    println!(
        "party storm: {changes} changes by one writer in {secs:.2} s: {:.0} a second (with {} invitations and {} lines besides)",
        changes as f64 / secs,
        PARTIES * ROUNDS * (PARTY_MAX - 1),
        PARTIES * ROUNDS
    );
    // Every change has a number of its own.
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), changes);
    // The zone heard every change (in whatever order), and keeping the largest number per
    // character leaves everybody in no party, as the hub has it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut kept = Kept::default();
    let heard: Vec<PartyNews> = meadow
        .notices
        .lock()
        .unwrap()
        .iter()
        .filter_map(|n| match n {
            HubNotice::Party(news) if news.seq > swept.seq => Some(news.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(heard.len(), changes, "every change told to the zone");
    for news in &heard {
        kept.news(news);
    }
    for members in &storm {
        for p in members {
            let direct = Parties::new(db.pool().clone());
            let reading = direct.reading(p.id).await.unwrap();
            assert_eq!(reading.party, None, "{}", p.id);
            assert_eq!(kept.0.get(&p.id).map(|r| r.party.clone()), Some(None));
        }
    }
}
