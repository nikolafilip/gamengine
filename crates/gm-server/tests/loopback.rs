//! The real socket path: a zone on 127.0.0.1 and two bots over actual UDP for two seconds.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, run_bot};
use gm_bsp::Bsp;
use gm_core::tick::TickRate;
use gm_net::transport::{Identity, client_config, server_config};
use gm_server::{ZoneConfig, ZoneWorld};

const MAP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/test_room.bsp"
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zone_and_bots_over_real_udp() {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(MAP)).unwrap());
    let bsp = Arc::new(Bsp::load(Path::new(MAP)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 3),
        report_every: Duration::from_secs(1),
        ..ZoneConfig::default()
    };
    let server = tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()));

    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(client_config(std::slice::from_ref(&identity.cert)).unwrap());
    let mut bots = Vec::new();
    for (i, behaviour) in [Behaviour::Hunter, Behaviour::Hold].into_iter().enumerate() {
        let client = client.clone();
        let bsp = bsp.clone();
        bots.push(tokio::spawn(async move {
            run_bot(
                &client,
                addr,
                BotConfig {
                    name: format!("loop{i}"),
                    seed: 10 + i as u64,
                    behaviour,
                    rate: TickRate::COMBAT,
                    run_ticks: 128,
                    build: None,
                    team: 0,
                    counter_pick: false,
                },
                bsp,
                std::future::pending(),
            )
            .await
        }));
    }
    for b in bots {
        let r = b.await.unwrap().unwrap();
        println!(
            "{}: snaps {} corrections {} rtt {:.2} ms tx {:.0} rx {:.0} B/s",
            r.name,
            r.client.snapshots,
            r.client.corrections,
            r.rtt_ms,
            r.tx_bytes_per_s(),
            r.rx_bytes_per_s()
        );
        assert!(
            r.client.snapshots > 64,
            "{}: {} snapshots",
            r.name,
            r.client.snapshots
        );
        assert_eq!(r.client.decode_errors, 0);
    }
    let report = server.await.unwrap().unwrap();
    println!("server: {report:?}");
    assert_eq!(report.joins, 2);
    assert!(report.executed_frames >= 200, "{}", report.executed_frames);
}
