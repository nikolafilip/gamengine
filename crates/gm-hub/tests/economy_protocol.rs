//! The economy over the hub protocol (ECONOMY.md, HUB.md 3): sessions act for their own
//! characters, zones report drops and contract outcomes, and nobody else can do either.
//! Needs `GM_TEST_DATABASE_URL`; without it the test is skipped.

use std::path::Path;
use std::time::Duration;

use gm_core::tick::TickRate;
use gm_hub::economy::Economy;
use gm_hub::protocol::{
    BuildChoice, CharacterId, ContractOutcome, EconOp, EconReply, HubError, HubRequest,
    HubResponse, PLACE_WEAPON, SessionId, ZoneEconOp,
};
use gm_hub::{Db, HubClient, HubClientError, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config};

const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "economy-test-secret";

async fn zone(addr: std::net::SocketAddr, cert: &[u8], id: &str) -> HubClient {
    let client = HubClient::connect_with_cert(addr, cert.to_vec())
        .await
        .unwrap();
    let r = client
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
    client
}

/// Register, create a character, enter `zone_id` and have the zone claim it.
async fn player(
    addr: std::net::SocketAddr,
    cert: &[u8],
    zone_conn: &HubClient,
    zone_id: &str,
    name: &str,
) -> (HubClient, SessionId, CharacterId) {
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
    let HubResponse::Ticket(ticket) = client
        .request(&HubRequest::Enter {
            session,
            character: c.id,
            zone: zone_id.into(),
        })
        .await
        .unwrap()
    else {
        panic!("enter")
    };
    let r = zone_conn
        .request(&HubRequest::Claim {
            token: ticket.token,
        })
        .await
        .unwrap();
    assert!(matches!(r, HubResponse::Claimed { .. }), "{r:?}");
    (client, session, c.id)
}

async fn econ(
    client: &HubClient,
    session: SessionId,
    character: CharacterId,
    op: EconOp,
) -> Result<EconReply, HubError> {
    match client
        .request(&HubRequest::Econ {
            session,
            character,
            op,
        })
        .await
    {
        Ok(HubResponse::Econ(r)) => Ok(r),
        Err(HubClientError::Refused(e)) => Err(e),
        other => panic!("unexpected {other:?}"),
    }
}

async fn zone_econ(client: &HubClient, op: ZoneEconOp) -> Result<EconReply, HubError> {
    match client.request(&HubRequest::ZoneEcon(op)).await {
        Ok(HubResponse::Econ(r)) => Ok(r),
        Err(HubClientError::Refused(e)) => Err(e),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_economy_over_the_wire() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let items = gm_content::items::load_items(Path::new(CONTENT)).expect("items");

    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"]).unwrap();
    let cert = identity.cert_der().to_vec();
    let endpoint = quinn::Endpoint::server(
        hub_server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let hub = tokio::spawn(gm_hub::run(
        HubConfig {
            zone_secret: SECRET.into(),
            content,
            key: HubKey::generate(),
            session_secs: 3600,
            auth_per_minute: 100.0,
            econ_per_second: 1000.0,
            party_sweep: std::time::Duration::from_millis(300),
            party_away: std::time::Duration::from_secs(2),
            items: items.clone(),
            max_coin_grant: 10_000, // this test grants 900 at once: the cap is a knob, the production default is 500 (ECONOMY.md 12)
            models_dir: std::env::temp_dir().join(format!("gm-hub-models-{}", std::process::id())),
            ingest: gm_hub::IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
            start_zone: None,
            blurbs: Vec::new(),
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));

    let town = zone(addr, &cert, "town").await;
    let other_zone = zone(addr, &cert, "elsewhere").await;
    let (smith_conn, smith_s, smith) = player(addr, &cert, &town, "town", "Smith").await;
    let (buyer_conn, buyer_s, buyer) = player(addr, &cert, &town, "town", "Buyer").await;

    // The zone reports a boss kill: components to the smith, a small purse to the buyer.
    let grants = vec![
        (smith, "core/iron".to_string()),
        (smith, "frame/oak".to_string()),
    ];
    let EconReply::Ids(parts) = zone_econ(
        &town,
        ZoneEconOp::GrantComponents {
            grants: grants.clone(),
            reference: 1,
        },
    )
    .await
    .unwrap() else {
        panic!("grant")
    };
    zone_econ(
        &town,
        ZoneEconOp::GrantCoin {
            character: buyer,
            amount: 900,
            reference: 1,
        },
    )
    .await
    .unwrap();

    // Nobody but the character's own zone can create things.
    assert_eq!(
        zone_econ(
            &other_zone,
            ZoneEconOp::GrantComponents {
                grants,
                reference: 2
            }
        )
        .await,
        Err(HubError::Unauthorized)
    );
    assert_eq!(
        zone_econ(
            &smith_conn,
            ZoneEconOp::GrantCoin {
                character: smith,
                amount: 5,
                reference: 0
            }
        )
        .await,
        Err(HubError::Unauthorized)
    );
    assert!(matches!(
        zone_econ(
            &town,
            ZoneEconOp::GrantCoin {
                character: buyer,
                amount: 1_000_000,
                reference: 0
            }
        )
        .await,
        Err(HubError::Invalid(_))
    ));
    // A session acts only for its own characters.
    assert_eq!(
        econ(&buyer_conn, buyer_s, smith, EconOp::Inventory).await,
        Err(HubError::NotFound)
    );

    // Craft: content decides which templates exist.
    assert!(matches!(
        econ(
            &smith_conn,
            smith_s,
            smith,
            EconOp::Craft {
                template: "railgun".into(),
                components: parts.clone()
            }
        )
        .await,
        Err(HubError::Invalid(_))
    ));
    let EconReply::Id(sword) = econ(
        &smith_conn,
        smith_s,
        smith,
        EconOp::Craft {
            template: "sword".into(),
            components: parts,
        },
    )
    .await
    .unwrap() else {
        panic!("craft")
    };

    // The trade window, with the real 3 s cooldown. It is opened by the zone both play
    // in (PARTY.md 6): no other zone can, and nobody can for somebody elsewhere.
    let open = ZoneEconOp::TradeOpen { a: smith, b: buyer };
    assert_eq!(
        zone_econ(&other_zone, open.clone()).await,
        Err(HubError::Unauthorized)
    );
    let EconReply::Id(trade) = zone_econ(&town, open).await.unwrap() else {
        panic!("trade")
    };
    econ(
        &smith_conn,
        smith_s,
        smith,
        EconOp::TradeOfferItem { trade, item: sword },
    )
    .await
    .unwrap();
    econ(
        &buyer_conn,
        buyer_s,
        buyer,
        EconOp::TradeSetCoin { trade, coin: 400 },
    )
    .await
    .unwrap();
    let EconReply::TradeView {
        version,
        mine,
        theirs,
        ..
    } = econ(&buyer_conn, buyer_s, buyer, EconOp::TradeView { trade })
        .await
        .unwrap()
    else {
        panic!("view")
    };
    assert_eq!(
        (mine.coin, theirs.items.len(), theirs.items[0].id),
        (400, 1, sword)
    );
    assert_eq!(theirs.items[0].components.len(), 2);
    assert_eq!(
        econ(
            &buyer_conn,
            buyer_s,
            buyer,
            EconOp::TradeAccept { trade, version }
        )
        .await,
        Err(HubError::Cooldown)
    );
    tokio::time::sleep(Duration::from_millis(3_100)).await;
    assert_eq!(
        econ(
            &buyer_conn,
            buyer_s,
            buyer,
            EconOp::TradeAccept { trade, version }
        )
        .await,
        Ok(EconReply::Trade { committed: false })
    );
    assert_eq!(
        econ(
            &smith_conn,
            smith_s,
            smith,
            EconOp::TradeAccept { trade, version }
        )
        .await,
        Ok(EconReply::Trade { committed: true })
    );
    let EconReply::Holder { coin, items } = econ(&buyer_conn, buyer_s, buyer, EconOp::Inventory)
        .await
        .unwrap()
    else {
        panic!("inventory")
    };
    assert_eq!((coin, items.len(), items[0].id), (500, 1, sword));

    // A carry contract in this zone: only this zone decides it, once.
    let EconReply::Id(contract) = econ(
        &buyer_conn,
        buyer_s,
        buyer,
        EconOp::ContractPost {
            instance: "town".into(),
            price: 300,
            collateral: 0,
        },
    )
    .await
    .unwrap() else {
        panic!("contract")
    };
    econ(
        &smith_conn,
        smith_s,
        smith,
        EconOp::ContractAccept {
            contract,
            sellers: vec![smith],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        zone_econ(
            &other_zone,
            ZoneEconOp::ContractReport {
                contract,
                outcome: ContractOutcome::Wipe
            }
        )
        .await,
        Err(HubError::Unauthorized)
    );
    assert_eq!(
        zone_econ(
            &town,
            ZoneEconOp::ContractReport {
                contract,
                outcome: ContractOutcome::Completed
            }
        )
        .await,
        Ok(EconReply::Decided(true))
    );
    assert_eq!(
        zone_econ(
            &town,
            ZoneEconOp::ContractReport {
                contract,
                outcome: ContractOutcome::Wipe
            }
        )
        .await,
        Ok(EconReply::Decided(false))
    );
    let EconReply::Holder { coin, .. } = econ(&smith_conn, smith_s, smith, EconOp::Inventory)
        .await
        .unwrap()
    else {
        panic!("inventory")
    };
    assert_eq!(coin, 700);

    // A stall on the zone's grid: only the zone a character stands in may open it, the tile
    // is taken by constraint, and the zone sees who keeps it.
    let EconReply::Stall(stall) = zone_econ(
        &town,
        ZoneEconOp::StallOpen {
            character: buyer,
            tile_x: 3,
            tile_y: 3,
        },
    )
    .await
    .unwrap() else {
        panic!("stall")
    };
    assert_eq!((stall.tile_x, stall.tile_y, stall.owner), (3, 3, buyer));
    assert_eq!(stall.owner_name, "Buyer");
    assert_eq!(stall.frame, 0, "an ironclad is a colossus");
    assert_eq!(
        zone_econ(
            &town,
            ZoneEconOp::StallOpen {
                character: smith,
                tile_x: 3,
                tile_y: 3
            }
        )
        .await,
        Err(HubError::Taken)
    );
    assert_eq!(
        zone_econ(
            &other_zone,
            ZoneEconOp::StallOpen {
                character: smith,
                tile_x: 4,
                tile_y: 3
            }
        )
        .await,
        Err(HubError::Unauthorized),
        "a zone speaks only for the characters playing in it"
    );
    assert_eq!(
        zone_econ(&town, ZoneEconOp::Stalls).await,
        Ok(EconReply::Stalls(vec![stall.clone()]))
    );
    assert_eq!(
        zone_econ(&other_zone, ZoneEconOp::Stalls).await,
        Ok(EconReply::Stalls(Vec::new()))
    );
    let EconReply::Id(listing) = econ(
        &buyer_conn,
        buyer_s,
        buyer,
        EconOp::StallList {
            item: sword,
            price: 650,
        },
    )
    .await
    .unwrap() else {
        panic!("list")
    };
    // What the stall shows, from anywhere: the keeper, the sword, its price, and what the
    // sword does (iron 30 and oak 10, both on Slash: ITEMS.md 3.2).
    let EconReply::Listings {
        owner,
        mine,
        listings,
    } = econ(
        &smith_conn,
        smith_s,
        smith,
        EconOp::StallView { stall: stall.id },
    )
    .await
    .unwrap()
    else {
        panic!("view")
    };
    assert_eq!((owner.as_str(), mine, listings.len()), ("Buyer", false, 1));
    assert_eq!(
        (listings[0].id, listings[0].item.id, listings[0].price),
        (listing, sword, 650)
    );
    assert_eq!(listings[0].item.place, PLACE_WEAPON);
    assert_eq!(listings[0].item.edge, [40, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(listings[0].item.does, ["slash +2.0%"]);
    // Buying is done standing at the stall (ITEMS.md 5): the zone the buyer plays in asks,
    // and names the stall it saw the buyer at.
    let buy = |zone: &HubClient, stall: i64| {
        let op = ZoneEconOp::StallBuy {
            character: smith,
            stall,
            listing,
            price: 650,
        };
        let zone = zone.clone();
        async move { zone_econ(&zone, op).await }
    };
    assert_eq!(
        buy(&other_zone, stall.id).await,
        Err(HubError::Unauthorized),
        "a zone speaks only for the characters playing in it"
    );
    assert_eq!(
        buy(&town, stall.id + 1).await,
        Err(HubError::NotFound),
        "not a listing of the stall the buyer stands at"
    );
    // Answered with the reading of what the buyer carries now (MODES.md 11.7).
    assert!(matches!(buy(&town, stall.id).await, Ok(EconReply::Gear(_))));
    assert_eq!(buy(&town, stall.id).await, Err(HubError::NotFound));

    // The owner closes the stall from its own session; the tile is free again.
    assert_eq!(
        econ(&buyer_conn, buyer_s, buyer, EconOp::StallClose).await,
        Ok(EconReply::Done)
    );
    assert_eq!(
        zone_econ(&town, ZoneEconOp::Stalls).await,
        Ok(EconReply::Stalls(Vec::new()))
    );
    assert!(matches!(
        zone_econ(
            &town,
            ZoneEconOp::StallOpen {
                character: smith,
                tile_x: 3,
                tile_y: 3
            }
        )
        .await,
        Ok(EconReply::Stall(_))
    ));
    assert_eq!(
        zone_econ(&town, ZoneEconOp::StallClose { character: smith }).await,
        Ok(EconReply::Done)
    );

    // A kill is one report and pays once (ECONOMY.md 9). The smith and the buyer share it; a
    // third recipient is playing elsewhere, so its coin is not made and its component lies
    // on the town's ground. The same report again, as a zone that got no answer would send
    // it, changes nothing.
    let (_idler_conn, _idler_s, idler) =
        player(addr, &cert, &other_zone, "elsewhere", "Idler").await;
    let holds = |r: Result<EconReply, HubError>| match r {
        Ok(EconReply::Holder { coin, items }) => (coin, items.len()),
        other => panic!("inventory: {other:?}"),
    };
    let smith_before = holds(econ(&smith_conn, smith_s, smith, EconOp::Inventory).await);
    let buyer_before = holds(econ(&buyer_conn, buyer_s, buyer, EconOp::Inventory).await);
    let kill = ZoneEconOp::GrantKill {
        reference: 77,
        components: vec![
            (smith, "core/iron".to_string()),
            (buyer, "frame/ash".to_string()),
            (idler, "catalyst/basalt".to_string()),
        ],
        coin: vec![(smith, 10), (buyer, 10), (idler, 10)],
    };
    let Ok(EconReply::Ids(dropped)) = zone_econ(&town, kill.clone()).await else {
        panic!("the kill was not paid")
    };
    assert_eq!(dropped.len(), 3);
    assert_eq!(zone_econ(&town, kill.clone()).await, Ok(EconReply::Done));
    assert_eq!(
        holds(econ(&smith_conn, smith_s, smith, EconOp::Inventory).await),
        (smith_before.0 + 10, smith_before.1 + 1)
    );
    assert_eq!(
        holds(econ(&buyer_conn, buyer_s, buyer, EconOp::Inventory).await),
        (buyer_before.0 + 10, buyer_before.1 + 1)
    );
    // The idler's component is on the ground here: whoever stands in the town picks it up.
    assert_eq!(
        zone_econ(
            &town,
            ZoneEconOp::Pickup {
                character: smith,
                item: dropped[2]
            }
        )
        .await,
        Ok(EconReply::Done)
    );
    // The name of a kill is its zone's: another zone's kill 77 is another kill. And a purse
    // over the cap is refused whole, before anything is claimed.
    assert!(matches!(
        zone_econ(
            &other_zone,
            ZoneEconOp::GrantKill {
                reference: 77,
                components: vec![(idler, "core/iron".to_string())],
                coin: Vec::new(),
            }
        )
        .await,
        Ok(EconReply::Ids(_))
    ));
    assert!(matches!(
        zone_econ(
            &town,
            ZoneEconOp::GrantKill {
                reference: 78,
                components: Vec::new(),
                coin: vec![(smith, 10_001)],
            }
        )
        .await,
        Err(HubError::Invalid(_))
    ));

    // Every silver is accounted for: 900 and the two purses of the kill created, nothing
    // burned, all of it in circulation.
    let e = Economy::new(db.pool().clone());
    assert_eq!(e.audit().await.unwrap(), 0);
    let s = e.supply().await.unwrap();
    assert_eq!((s.created, s.burned, s.circulating), (920, 0, 920));
    hub.abort();
}
