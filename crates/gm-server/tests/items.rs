//! Phase 11 over the real protocol (ITEMS.md 5, 7): a hub, the town zone and clients driven
//! by hand. A stall is bought from only by a body standing at it; what is worn changes
//! through the zone, at once, and not in a fight; and the zone's hits show the edge, before
//! and after, to the point. Needs `GM_TEST_DATABASE_URL` (a Postgres the test may wipe);
//! without it the test is skipped.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::build::Sheet;
use gm_core::sim::{Input, buttons};
use gm_core::tick::TickRate;
use gm_core::trace::{CollisionWorld, Hull};
use gm_hub::economy::Economy;
use gm_hub::protocol::{
    BuildChoice, CharacterId, EconOp, EconReply, HubRequest, HubResponse, ItemSummary, SessionId,
    ZoneTicket,
};
use gm_hub::{Db, HubClient, HubConfig, HubKey, IngestMode};
use gm_net::PROTOCOL_VERSION;
use gm_net::client::ClientState;
use gm_net::control::{self, Control, StallEntry};
use gm_net::transport::{Identity, SERVER_NAME, client_config, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ZoneConfig, ZoneWorld};
use quinn::rustls::pki_types::CertificateDer;

const TOWN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/town.bsp"
);
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "items-zone-secret";
const PASSWORD: &str = "correct horse battery";
/// The fight lock of this test's zone: long enough to run into on a machine that is doing
/// other things, short enough to wait out.
const LOCK: Duration = Duration::from_secs(3);
const FIGHT: &str = "not in a fight: wait a moment";
const GATE: &str = "one thing at a time: try again in a moment";

/// What the test tells a client to do, and what the client has seen.
#[derive(Default)]
struct Shared {
    yaw: f32,
    buttons: u16,
    say: Vec<Control>,
    health: i32,
    alive: bool,
    synced: bool,
    heard: Vec<Control>,
    stalls: Vec<StallEntry>,
}

/// A client driven by the test: it stands where it was put, looks where it is told,
/// holds the buttons it is told to and says to the zone what it is told to.
struct Hand {
    shared: Arc<Mutex<Shared>>,
    task: tokio::task::JoinHandle<()>,
}

impl Hand {
    async fn join(ticket: &ZoneTicket, name: &str, yaw: f32, world: Arc<Bsp>) -> Hand {
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            client_config(&[CertificateDer::from(ticket.cert_der.clone())]).unwrap(),
        );
        let conn = endpoint
            .connect(ticket.addr, SERVER_NAME)
            .unwrap()
            .await
            .expect("the zone answers");
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        control::send(
            &mut send,
            &Control::Hello {
                version: PROTOCOL_VERSION as u16,
                name: name.to_string(),
                token: bitcode::encode(&ticket.token),
                build: None,
                team: 0,
            },
        )
        .await
        .unwrap();
        let (entity, hz) = match control::recv(&mut recv).await.unwrap() {
            Some(Control::Welcome { entity, hz, .. }) => (entity, hz),
            other => panic!("{name}: no welcome: {other:?}"),
        };
        let (pack, own, team) = match control::recv(&mut recv).await.unwrap() {
            Some(Control::Content { pack, own, team }) => (pack, own, team),
            other => panic!("{name}: no content: {other:?}"),
        };
        let rate = TickRate::new(hz as u32);
        let mut client = ClientState::new(entity, rate, Sheet::new(own, &pack, team));
        let shared = Arc::new(Mutex::new(Shared {
            yaw,
            ..Shared::default()
        }));
        let seen = shared.clone();
        let task = tokio::spawn(async move {
            let _endpoint = endpoint;
            let mut next = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(next) => {
                        next += rate.period();
                        let (input, say) = {
                            let mut s = seen.lock().unwrap();
                            s.health = client.own_health;
                            s.alive = client.own_alive;
                            s.synced = client.synced();
                            let input = Input {
                                buttons: s.buttons,
                                yaw: s.yaw,
                                pitch: 0.0,
                                forward: 0.0,
                                side: 0.0,
                                ability: 0,
                            };
                            (input, std::mem::take(&mut s.say))
                        };
                        for msg in say {
                            if control::send(&mut send, &msg).await.is_err() {
                                return;
                            }
                        }
                        let datagram = client.local_tick(world.as_ref(), input);
                        if conn.send_datagram(Bytes::from(datagram.encode())).is_err() {
                            return;
                        }
                        let t = client.render_tick(0.0);
                        client.prune(t);
                    }
                    dg = conn.read_datagram() => match dg {
                        Ok(bytes) => {
                            let _ = client.on_snapshot(world.as_ref(), &bytes);
                        }
                        Err(_) => return,
                    },
                    msg = control::recv(&mut recv) => match msg {
                        Ok(Some(msg)) => {
                            let mut s = seen.lock().unwrap();
                            match &msg {
                                Control::Stalls(list) => s.stalls = list.clone(),
                                Control::StallOpened(stall) => s.stalls.push(stall.clone()),
                                Control::StallClosed(id) => s.stalls.retain(|x| x.id != *id),
                                _ => {}
                            }
                            s.heard.push(msg);
                        }
                        _ => return,
                    },
                }
            }
        });
        let hand = Hand { shared, task };
        hand.until("its first snapshot", |s| s.synced && s.alive)
            .await;
        hand
    }

    /// Wait until what the client has seen satisfies `ready` (five seconds at most).
    async fn until<T>(&self, what: &str, ready: impl Fn(&mut Shared) -> T) -> T
    where
        T: Ready,
    {
        let started = Instant::now();
        loop {
            let got = ready(&mut self.shared.lock().unwrap());
            if got.is_ready() {
                return got;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "waited five seconds for {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Say something to the zone and wait for its answer to it.
    async fn ask(&self, msg: Control) -> Result<(), String> {
        self.ask_all(vec![msg]).await.remove(0)
    }

    /// Say several things at once (they reach the zone within one of its ticks) and wait
    /// for an answer to each, in the order the zone gave them.
    async fn ask_all(&self, msgs: Vec<Control>) -> Vec<Result<(), String>> {
        let n = msgs.len();
        {
            let mut s = self.shared.lock().unwrap();
            s.heard.clear();
            s.say.extend(msgs);
        }
        self.until("the zone's answers", |s| {
            let answers: Vec<Result<(), String>> = s
                .heard
                .iter()
                .filter_map(|m| match m {
                    Control::BuyResult { result, .. }
                    | Control::WearResult { result, .. }
                    | Control::StallResult(result) => Some(result.clone()),
                    _ => None,
                })
                .collect();
            (answers.len() >= n).then_some(answers)
        })
        .await
        .unwrap()
    }

    /// Ask until the zone no longer says "in a fight" (or that it is busy with the last
    /// asking): what a person does who is told to wait a moment. `Err`: it said something
    /// else.
    async fn ask_when_calm(&self, msg: Control) -> Result<(), String> {
        let started = Instant::now();
        loop {
            match self.ask(msg.clone()).await {
                Err(why) if why == FIGHT || why == GATE => {
                    assert!(
                        started.elapsed() < Duration::from_secs(30),
                        "still \"{why}\" after thirty seconds"
                    );
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                other => return other,
            }
        }
    }

    fn health(&self) -> i32 {
        self.shared.lock().unwrap().health
    }

    /// One swing of the primary: held for a fifth of a second.
    async fn swing(&self) {
        self.shared.lock().unwrap().buttons = buttons::PRIMARY;
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.shared.lock().unwrap().buttons = 0;
    }

    /// Hang up without a goodbye: the zone saves the character and it goes offline.
    async fn leave(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

trait Ready {
    fn is_ready(&self) -> bool;
}

impl Ready for bool {
    fn is_ready(&self) -> bool {
        *self
    }
}

impl<T> Ready for Option<T> {
    fn is_ready(&self) -> bool {
        self.is_some()
    }
}

struct Someone {
    hub: HubClient,
    session: SessionId,
    character: CharacterId,
}

impl Someone {
    async fn new(addr: std::net::SocketAddr, cert: &[u8], name: &str, preset: &str) -> Someone {
        let hub = HubClient::connect_with_cert(addr, cert.to_vec())
            .await
            .unwrap();
        let HubResponse::Session { session, .. } = hub
            .request(&HubRequest::Register {
                email: format!("{name}@example.test"),
                password: PASSWORD.into(),
            })
            .await
            .unwrap()
        else {
            panic!("register")
        };
        let HubResponse::Character(c) = hub
            .request(&HubRequest::CreateCharacter {
                session,
                name: name.into(),
                build: BuildChoice::Preset(preset.into()),
            })
            .await
            .unwrap()
        else {
            panic!("create")
        };
        Someone {
            hub,
            session,
            character: c.id,
        }
    }

    async fn ticket(&self) -> ZoneTicket {
        // A character that just left is put away by its zone in a moment: asked again.
        for _ in 0..50 {
            match self
                .hub
                .request(&HubRequest::Enter {
                    session: self.session,
                    character: self.character,
                    zone: "town".into(),
                })
                .await
            {
                Ok(HubResponse::Ticket(t)) => return t,
                Ok(other) => panic!("{other:?}"),
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        panic!("the hub never let the character in again")
    }

    async fn econ(&self, op: EconOp) -> EconReply {
        match self
            .hub
            .request(&HubRequest::Econ {
                session: self.session,
                character: self.character,
                op,
            })
            .await
            .unwrap()
        {
            HubResponse::Econ(reply) => reply,
            other => panic!("{other:?}"),
        }
    }

    async fn inventory(&self) -> (i64, Vec<ItemSummary>) {
        match self.econ(EconOp::Inventory).await {
            EconReply::Holder { coin, items } => (coin, items),
            other => panic!("{other:?}"),
        }
    }
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_weapon_is_bought_at_a_stall_worn_and_felt_in_the_zone_s_hits() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    if let Ok(filter) = std::env::var("GM_TRACE") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let items = gm_content::items::load_items(Path::new(CONTENT)).expect("items");

    // The hub.
    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"]).unwrap();
    let hub_cert = identity.cert_der().to_vec();
    let endpoint = quinn::Endpoint::server(
        hub_server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let hub_addr = endpoint.local_addr().unwrap();
    let _hub = tokio::spawn(gm_hub::run(
        HubConfig {
            zone_secret: SECRET.into(),
            content: content.clone(),
            key: HubKey::generate(),
            session_secs: 3600,
            auth_per_minute: 1000.0,
            econ_per_second: 1000.0,
            items: items.clone(),
            max_coin_grant: 10_000,
            models_dir: std::env::temp_dir()
                .join(format!("gm-items-models-{}", std::process::id())),
            ingest: IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
            start_zone: None,
            blurbs: Vec::new(),
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));

    // The town, whose fight lock is short enough to wait out.
    let zone_identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(TOWN)).expect("town.bsp is built"));
    let map = Arc::new(world.bsp.clone());
    let grid = world.stall_grids[0];
    let zone_endpoint = quinn::Endpoint::server(
        server_config(&zone_identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let link = HubLink::connect(HubLinkConfig {
        addr: hub_addr,
        cert_der: hub_cert.clone(),
        zone: "town".into(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: zone_endpoint.local_addr().unwrap(),
        zone_cert_der: zone_identity.cert_der().to_vec(),
        web: None,
        min_trust: 0,
        requires: Vec::new(),
        max_players: 64,
    })
    .await
    .expect("the zone registers");
    let _zone = tokio::spawn(gm_server::run(
        ZoneConfig {
            max_ticks: Some(64 * 120),
            content,
            hub: Some(link),
            gear_after_fight: LOCK,
            ..ZoneConfig::default()
        },
        world.clone(),
        zone_endpoint,
        std::future::pending(),
    ));

    // A keeper on a market tile, a buyer in front of the counter and facing it, and
    // somebody across the square. (An operator puts them there: `gm-hub --place`.)
    // (A wall of plate: each blow moves it a hair, and five of them leave it standing.)
    let smith = Someone::new(hub_addr, &hub_cert, "Smith", "ironclad").await;
    let buyer = Someone::new(hub_addr, &hub_cert, "Buyer", "blade").await;
    let far = Someone::new(hub_addr, &hub_cert, "Far", "blade").await;
    let tile = grid.centre(grid.base_x, grid.base_y).expect("a tile");
    // A hair above standing: a body exactly on the floor is in it, as far as a zone that
    // asks whether a saved place is still a place to stand can tell.
    let up = Vec3::Z * (1.0 - Hull::Player.mins().z);
    for at in [
        tile,
        tile + Vec3::new(
            grid.yaw.to_radians().cos(),
            grid.yaw.to_radians().sin(),
            0.0,
        ) * 52.0,
    ] {
        assert_eq!(
            world.bsp.point_contents(Hull::Player, at + up),
            gm_core::trace::Contents::Empty,
            "{at:?} is a place to stand"
        );
    }
    let (sin, cos) = grid.yaw.to_radians().sin_cos();
    let front = tile + Vec3::new(cos, sin, 0.0) * 52.0;
    assert!(
        db.place(smith.character, "town", (tile + up).into(), grid.yaw)
            .await
            .unwrap()
    );
    assert!(
        db.place(
            buyer.character,
            "town",
            (front + up).into(),
            grid.yaw + 180.0
        )
        .await
        .unwrap()
    );
    let smith_hand = Hand::join(&smith.ticket().await, "Smith", grid.yaw, map.clone()).await;
    let mut buyer_hand = Hand::join(
        &buyer.ticket().await,
        "Buyer",
        grid.yaw + 180.0,
        map.clone(),
    )
    .await;
    let far_hand = Hand::join(&far.ticket().await, "Far", 0.0, map.clone()).await;
    assert!(
        !db.place(buyer.character, "town", (front + up).into(), 0.0)
            .await
            .unwrap(),
        "a character that plays is not moved behind its zone's back"
    );

    // The keeper opens its stall where it stands and puts two swords up; it keeps a
    // cuirass. The buyer is given what the dearer sword costs, and a little.
    assert_eq!(smith_hand.ask(Control::StallOpen).await, Ok(()));
    let stall = buyer_hand
        .until("the stall", |s| {
            s.stalls.iter().find(|x| x.owner == "Smith").map(|x| x.id)
        })
        .await
        .unwrap();
    let direct = Economy::new(db.pool().clone());
    let best = strings(&[
        "shard/boss_scale",
        "core/dragonbone",
        "catalyst/ember",
        "frame/whalebone",
        "gem/opal",
        "gem/opal",
    ]);
    let sword = direct
        .grant_item(smith.character, "sword", &best, None)
        .await
        .unwrap();
    let plain = direct
        .grant_item(
            smith.character,
            "sword",
            &strings(&["core/iron", "frame/oak"]),
            None,
        )
        .await
        .unwrap();
    let cuirass = direct
        .grant_item(
            smith.character,
            "cuirass",
            &strings(&[
                "shard/boss_scale",
                "core/dragonbone",
                "frame/whalebone",
                "gem/opal",
                "gem/opal",
            ]),
            None,
        )
        .await
        .unwrap();
    direct
        .grant_coin_as(buyer.character, 12_500, "grant", 0)
        .await
        .unwrap();
    direct
        .grant_coin_as(far.character, 50_000, "grant", 0)
        .await
        .unwrap();
    let list = |item: i64, price: i64| smith.econ(EconOp::StallList { item, price });
    let EconReply::Id(listing) = list(sword, 12_000).await else {
        panic!("list")
    };
    let EconReply::Id(dear) = list(plain, 90_000).await else {
        panic!("list")
    };
    let buy = |listing: i64, price: i64| Control::StallBuy {
        stall,
        listing,
        price,
    };

    // Buying is done standing at the stall: across the square the zone says so, whatever
    // the purse holds. (A refusal of the zone's own costs nobody their second: asked
    // twice at once, it is told twice.)
    let walk = Err("walk up to the stall to buy".to_string());
    assert_eq!(
        far_hand
            .ask_all(vec![buy(listing, 12_000), buy(listing, 12_000)])
            .await,
        vec![walk.clone(), walk]
    );
    // At the counter. Two requests at once: the hub is asked the first (and refuses a
    // price that is not the listing's), the second is stopped by the gate, and both are
    // answered.
    let mut two = buyer_hand
        .ask_all(vec![buy(listing, 11_000), buy(listing, 12_000)])
        .await;
    two.sort();
    assert_eq!(
        two,
        vec![Err(GATE.to_string()), Err("the price changed".to_string())]
    );
    // Then, a second later each time: the purchase; not twice; not more than the purse
    // holds; not from oneself; not from a stall that is not there.
    assert_eq!(buyer_hand.ask_when_calm(buy(listing, 12_000)).await, Ok(()));
    assert_eq!(
        buyer_hand.ask_when_calm(buy(listing, 12_000)).await,
        Err("it is no longer for sale here".into())
    );
    assert_eq!(
        buyer_hand.ask_when_calm(buy(dear, 90_000)).await,
        Err("not enough coin".into())
    );
    assert_eq!(
        smith_hand.ask_when_calm(buy(dear, 90_000)).await,
        Err("that is your own stall".into())
    );
    let nowhere = Control::StallBuy {
        stall: stall + 7,
        listing: dear,
        price: 90_000,
    };
    assert_eq!(
        buyer_hand.ask_when_calm(nowhere).await,
        Err("that stall has closed".into())
    );
    let (coin, has) = buyer.inventory().await;
    assert_eq!((coin, has.len(), has[0].id), (500, 1, sword));
    // A part, for what cannot be worn.
    let gem = direct
        .grant_components("town", &[(buyer.character, "gem/quartz".to_string())], 0)
        .await
        .unwrap()[0];
    assert_eq!(smith.inventory().await.0, 12_000, "no tax and no fee");

    // The zone's hits, before and after. A swing in nothing first: what the keeper loses
    // is the number everything else is measured against.
    let hit = async |buyer_hand: &Hand| -> i32 {
        let before = smith_hand.health();
        buyer_hand.swing().await;
        smith_hand
            .until(&format!("a blow to land on {before}"), |s| {
                s.health < before
            })
            .await;
        // The swing is over before the next thing is asked.
        tokio::time::sleep(Duration::from_millis(300)).await;
        before - smith_hand.health()
    };
    let bare = hit(&buyer_hand).await;
    assert!(bare >= 10, "a sword on plate: {bare}");
    // The unrounded number is within half a point of `bare`: what a factor does to it.
    let moved = |factor: f32| {
        let (lo, hi) = (
            ((bare as f32 - 0.5) * factor + 0.5).floor() as i32,
            ((bare as f32 + 0.5) * factor + 0.5).floor() as i32,
        );
        lo..=hi
    };

    // What a body wears does not change in a fight: the buyer has just dealt damage and
    // the keeper has just taken it.
    assert_eq!(
        buyer_hand.ask(Control::Wear { item: sword }).await,
        Err(FIGHT.into())
    );
    assert_eq!(
        smith_hand.ask(Control::Wear { item: cuirass }).await,
        Err(FIGHT.into())
    );
    // A moment later it does (asked again until the zone has stopped saying so), at once:
    // the sword's edge on its own kind is 220 per mille, and a place counts for half:
    // eleven per cent more.
    assert_eq!(
        buyer_hand
            .ask_when_calm(Control::Wear { item: sword })
            .await,
        Ok(())
    );
    let armed = hit(&buyer_hand).await;
    assert!(
        moved(1.11).contains(&armed) && armed > bare,
        "{bare} in nothing, {armed} with the sword"
    );
    // What is not one's own is not put on, nor what cannot be worn at all; and of two
    // requests at once the gate stops one and answers both.
    assert_eq!(
        buyer_hand
            .ask_when_calm(Control::Wear { item: cuirass })
            .await,
        Err("that is not in the inventory".into())
    );
    assert_eq!(
        buyer_hand.ask_when_calm(Control::Wear { item: gem }).await,
        Err("that cannot be worn".into())
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let mut two = buyer_hand
        .ask_all(vec![
            Control::Wear { item: gem },
            Control::Wear { item: gem },
        ])
        .await;
    two.sort();
    assert_eq!(
        two,
        vec![
            Err(GATE.to_string()),
            Err("that cannot be worn".to_string())
        ]
    );
    // The keeper's cuirass takes the same eleven per cent off again: a wash.
    assert_eq!(
        smith_hand
            .ask_when_calm(Control::Wear { item: cuirass })
            .await,
        Ok(())
    );
    let both = hit(&buyer_hand).await;
    assert_eq!(both, bare, "sword against cuirass");

    // A claim carries what is worn: the buyer hangs up, is put away, comes back to where
    // it stood, and its sword still counts.
    buyer_hand.leave().await;
    buyer_hand = Hand::join(
        &buyer.ticket().await,
        "Buyer",
        grid.yaw + 180.0,
        map.clone(),
    )
    .await;
    let back = hit(&buyer_hand).await;
    assert_eq!(back, bare, "the sword came back with its wearer");

    // Taken off, the cuirass alone is left: eleven per cent less than in nothing.
    assert_eq!(
        buyer_hand
            .ask_when_calm(Control::TakeOff { item: sword })
            .await,
        Ok(())
    );
    let warded = hit(&buyer_hand).await;
    assert!(
        moved(1.0 / 1.11).contains(&warded) && warded < bare,
        "{bare} in nothing, {warded} against the cuirass"
    );
    println!(
        "items: a sword swing on plate {bare}; with the best sword {armed}; against the best cuirass too {both}; the cuirass alone {warded}"
    );

    // What the hub holds is what the zone applied; and the books are sound.
    let (_, has) = buyer.inventory().await;
    assert!(has.iter().all(|i| !i.worn));
    let (_, has) = smith.inventory().await;
    assert!(has.iter().any(|i| i.id == cuirass && i.worn));
    assert_eq!(direct.audit().await, Ok(0));
}
