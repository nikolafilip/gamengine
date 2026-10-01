//! The economy over the hub protocol (ECONOMY.md, HUB.md 3): sessions act for their own
//! characters, zones report drops and contract outcomes, and nobody else can do either.
//! Needs `GM_TEST_DATABASE_URL`; without it the test is skipped.

use std::path::Path;
use std::time::Duration;

use gm_core::tick::TickRate;
use gm_hub::economy::Economy;
use gm_hub::protocol::{
    BuildChoice, CharacterId, ContractOutcome, EconOp, EconReply, HubError, HubRequest,
    HubResponse, SessionId, ZoneEconOp,
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
            templates: items.template_ids(),
            max_coin_grant: 10_000,
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

    // The trade window, with the real 3 s cooldown.
    let EconReply::Id(trade) = econ(
        &smith_conn,
        smith_s,
        smith,
        EconOp::TradeOpen { with: buyer },
    )
    .await
    .unwrap() else {
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

    // A stall on the zone's grid, bought from over the wire.
    econ(
        &buyer_conn,
        buyer_s,
        buyer,
        EconOp::StallOpen {
            tile_x: 3,
            tile_y: 3,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        econ(
            &smith_conn,
            smith_s,
            smith,
            EconOp::StallOpen {
                tile_x: 3,
                tile_y: 3
            }
        )
        .await,
        Err(HubError::Invalid(_))
    ));
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
    assert_eq!(
        econ(
            &smith_conn,
            smith_s,
            smith,
            EconOp::StallBuy {
                listing,
                price: 650
            }
        )
        .await,
        Ok(EconReply::Done)
    );
    assert_eq!(
        econ(
            &smith_conn,
            smith_s,
            smith,
            EconOp::StallBuy {
                listing,
                price: 650
            }
        )
        .await,
        Err(HubError::NotFound)
    );

    // Every copper is accounted for: 900 created, nothing burned, 900 in circulation.
    let e = Economy::new(db.pool().clone());
    assert_eq!(e.audit().await.unwrap(), 0);
    let s = e.supply().await.unwrap();
    assert_eq!((s.created, s.burned, s.circulating), (900, 0, 900));
    hub.abort();
}
