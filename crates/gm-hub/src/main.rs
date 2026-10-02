//! gm-hub binary: migrate the database, bind the QUIC endpoint, write the certificate, run.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use gm_hub::{Db, HubConfig, HubKey, IngestMode};
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
    models_dir: PathBuf,
    grant_moderator: Option<String>,
    auth_per_minute: f64,
    /// A WebTransport listener for browsers beside the QUIC one (WEB.md 2).
    web_listen: Option<SocketAddr>,
    web_cert: Option<PathBuf>,
    web_key: Option<PathBuf>,
    web_url: Option<String>,
    web_origins: Vec<String>,
    web_info_out: Option<PathBuf>,
}

const USAGE: &str = "gm-hub --database-url URL [--listen ADDR] [--cert-out PATH] [--key PATH] [--content DIR] \
[--zone-secret S] [--models-dir DIR] [--auth-per-minute N] [--migrate-only] [--wipe] \
[--web-listen ADDR [--web-cert PEM --web-key PEM] [--web-url https://HOST:PORT] [--web-origin ORIGIN]... [--web-info-out PATH]]   \
(env: DATABASE_URL, GM_ZONE_SECRET)\n\
       gm-hub --database-url URL --grant-moderator EMAIL     make an existing account a moderator, then exit\n\
       gm-hub ingest-worker --frame NAME --in FILE --out DIR   (run by the hub itself, MODELS.md 6.2)";

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
        models_dir: PathBuf::from("models"),
        grant_moderator: None,
        auth_per_minute: 10.0,
        web_listen: None,
        web_cert: None,
        web_key: None,
        web_url: None,
        web_origins: Vec::new(),
        web_info_out: None,
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
            "--models-dir" => a.models_dir = PathBuf::from(value("--models-dir")?),
            "--grant-moderator" => a.grant_moderator = Some(value("--grant-moderator")?),
            "--auth-per-minute" => {
                a.auth_per_minute = value("--auth-per-minute")?
                    .parse()
                    .map_err(|e| format!("--auth-per-minute: {e}"))?
            }
            "--web-listen" => {
                a.web_listen = Some(
                    value("--web-listen")?
                        .parse()
                        .map_err(|e| format!("--web-listen: {e}"))?,
                )
            }
            "--web-cert" => a.web_cert = Some(PathBuf::from(value("--web-cert")?)),
            "--web-key" => a.web_key = Some(PathBuf::from(value("--web-key")?)),
            "--web-url" => a.web_url = Some(value("--web-url")?),
            "--web-origin" => a.web_origins.push(value("--web-origin")?),
            "--web-info-out" => a.web_info_out = Some(PathBuf::from(value("--web-info-out")?)),
            "--migrate-only" => a.migrate_only = true,
            "--wipe" => a.wipe = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if a.web_cert.is_some() != a.web_key.is_some() {
        return Err("--web-cert and --web-key go together".into());
    }
    if a.database_url.is_empty() {
        return Err("--database-url (or DATABASE_URL) is required".into());
    }
    if a.zone_secret.is_empty() && !a.migrate_only && a.grant_moderator.is_none() {
        return Err("--zone-secret (or GM_ZONE_SECRET) is required".into());
    }
    Ok(a)
}

/// The ingestion worker (MODELS.md 6.2): parse one upload under resource limits and leave a
/// report. It runs in its own process so that a hostile file costs one upload and no more.
fn ingest_worker(args: &[String]) -> ! {
    let mut frame = None;
    let mut input = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frame" => frame = it.next().and_then(|f| gm_model::rig::frame_by_name(f)),
            "--in" => input = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            _ => {}
        }
    }
    let (Some(frame), Some(input), Some(out)) = (frame, input, out) else {
        eprintln!("gm-hub ingest-worker --frame NAME --in FILE --out DIR");
        std::process::exit(2);
    };
    #[cfg(unix)]
    {
        // 2 GiB of address space and 20 s of CPU: a bomb dies here, not in the hub.
        let _ = rlimit::setrlimit(rlimit::Resource::AS, 2 << 30, 2 << 30);
        let _ = rlimit::setrlimit(rlimit::Resource::CPU, 20, 20);
        let _ = rlimit::setrlimit(rlimit::Resource::CORE, 0, 0);
    }
    match gm_ingest::worker(&input, frame, &out) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("gm-hub ingest-worker: {e}");
            std::process::exit(1);
        }
    }
}

fn main() -> anyhow::Result<()> {
    // The worker is a plain synchronous process: no runtime, no logging, no database.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ingest-worker") {
        ingest_worker(&args[1..]);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
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
    if let Some(email) = &args.grant_moderator {
        if gm_hub::models::grant_moderator(db.pool(), email).await? {
            info!(%email, "is a moderator now");
            return Ok(());
        }
        anyhow::bail!("no account with the email {email:?}");
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
        auth_per_minute: args.auth_per_minute,
        templates: gm_content::items::load_items(&args.content)?.template_ids(),
        max_coin_grant: 10_000,
        models_dir: args.models_dir,
        // Uploads are parsed by this same binary in a child process.
        ingest: IngestMode::Worker(std::env::current_exe()?),
        ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
    };
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let web = match args.web_listen {
        Some(listen) => {
            let (endpoint, addr) = gm_net::link::web_listener(
                listen,
                args.web_cert.as_deref().zip(args.web_key.as_deref()),
                args.web_url.clone(),
                gm_net::transport::hub_transport_config(),
            )
            .await?;
            if args.web_origins.is_empty() {
                tracing::warn!("the web listener accepts sessions from any origin (--web-origin)");
            }
            if let Some(path) = &args.web_info_out {
                std::fs::write(path, gm_net::link::web_info_json(&addr))?;
            }
            Some((endpoint, args.web_origins.clone()))
        }
        None => None,
    };
    gm_hub::run_with_web(cfg, db, endpoint, web, shutdown).await
}
