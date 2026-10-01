//! gm-hub binary: migrate the database, bind the QUIC endpoint, write the certificate, run.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use gm_hub::{Db, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config};
use tracing::info;

struct Args {
    database_url: String,
    listen: SocketAddr,
    cert_out: PathBuf,
    key: PathBuf,
    content: PathBuf,
    zone_secret: String,
    migrate_only: bool,
    wipe: bool,
}

const USAGE: &str = "gm-hub --database-url URL [--listen ADDR] [--cert-out PATH] [--key PATH] [--content DIR] \
[--zone-secret S] [--migrate-only] [--wipe]   (env: DATABASE_URL, GM_ZONE_SECRET)";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        database_url: std::env::var("DATABASE_URL").unwrap_or_default(),
        listen: "127.0.0.1:4400".parse().unwrap(),
        cert_out: PathBuf::from("hub-cert.der"),
        key: PathBuf::from("hub.key"),
        content: PathBuf::from("assets/content"),
        zone_secret: std::env::var("GM_ZONE_SECRET").unwrap_or_default(),
        migrate_only: false,
        wipe: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--database-url" => a.database_url = value("--database-url")?,
            "--listen" => {
                a.listen = value("--listen")?
                    .parse()
                    .map_err(|e| format!("--listen: {e}"))?
            }
            "--cert-out" => a.cert_out = PathBuf::from(value("--cert-out")?),
            "--key" => a.key = PathBuf::from(value("--key")?),
            "--content" => a.content = PathBuf::from(value("--content")?),
            "--zone-secret" => a.zone_secret = value("--zone-secret")?,
            "--migrate-only" => a.migrate_only = true,
            "--wipe" => a.wipe = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if a.database_url.is_empty() {
        return Err("--database-url (or DATABASE_URL) is required".into());
    }
    if a.zone_secret.is_empty() && !a.migrate_only {
        return Err("--zone-secret (or GM_ZONE_SECRET) is required".into());
    }
    Ok(a)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("gm-hub: {e}");
            std::process::exit(2);
        }
    };
    let db = Db::connect(&args.database_url).await?;
    db.migrate().await?;
    info!("database migrated");
    if args.wipe {
        db.wipe().await?;
        info!("database wiped");
    }
    if args.migrate_only {
        return Ok(());
    }
    let content = gm_content::load_dir(&args.content, gm_hub::hub::content_rate())?;
    let key = HubKey::load_or_create(&args.key)?;
    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"])?;
    std::fs::write(&args.cert_out, identity.cert_der())?;
    info!(cert = %args.cert_out.display(), key = %args.key.display(), "hub certificate and key ready");
    let endpoint = quinn::Endpoint::server(hub_server_config(&identity)?, args.listen)?;
    let cfg = HubConfig {
        zone_secret: args.zone_secret,
        content,
        key,
        session_secs: gm_hub::protocol::SESSION_SECS,
        auth_per_minute: 10.0,
    };
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    gm_hub::run(cfg, db, endpoint, shutdown).await
}
