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
    /// Where a character that was in no zone enters (CLIENT.md 7).
    start_zone: Option<String>,
    grant_moderator: Option<String>,
    /// An operator's hand (ITEMS.md 4): coin, or a crafted item, for a character by name.
    grant_coin: Option<(String, i64)>,
    grant_item: Option<(String, String, Vec<String>)>,
    /// Where an offline character stands when it next enters: its name, the zone, the
    /// place and the way it faces.
    place: Option<(String, String, [f32; 3], f32)>,
    /// Say whether the books are sound, and what moved, and exit.
    audit: bool,
    auth_per_minute: f64,
    econ_per_second: f64,
    /// Seconds a character that left the game is still of its party (PARTY.md 2).
    party_away: u64,
    /// A WebTransport listener for browsers beside the QUIC one (WEB.md 2).
    web_listen: Option<SocketAddr>,
    web_cert: Option<PathBuf>,
    web_key: Option<PathBuf>,
    web_url: Option<String>,
    web_origins: Vec<String>,
    web_info_out: Option<PathBuf>,
}

const USAGE: &str = "gm-hub --database-url URL [--listen ADDR] [--cert-out PATH] [--key PATH] [--content DIR] \
[--zone-secret S] [--models-dir DIR] [--start-zone ID] [--auth-per-minute N] [--econ-per-second N] [--party-away SECS] [--migrate-only] [--wipe] \
[--web-listen ADDR [--web-cert PEM --web-key PEM] [--web-url https://HOST:PORT] [--web-origin ORIGIN]... [--web-info-out PATH]]   \
(env: DATABASE_URL, GM_ZONE_SECRET)\n\
       gm-hub --database-url URL --grant-moderator EMAIL     make an existing account a moderator, then exit\n\
       gm-hub --database-url URL --grant-coin CHARACTER SILVER   give a character coin (through the ledger), then exit\n\
       gm-hub --database-url URL --grant-item CHARACTER TEMPLATE MATERIAL,MATERIAL,...   give it a crafted item, then exit\n\
       gm-hub --database-url URL --place CHARACTER ZONE X,Y,Z YAW   where an offline character stands when it next enters, then exit\n\
       gm-hub --database-url URL --audit                     say whether the books are sound and what moved, then exit (1: they are not)\n\
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
        start_zone: None,
        grant_moderator: None,
        grant_coin: None,
        grant_item: None,
        place: None,
        audit: false,
        auth_per_minute: 10.0,
        econ_per_second: 5.0,
        party_away: gm_hub::party::AWAY.as_secs(),
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
            "--start-zone" => a.start_zone = Some(value("--start-zone")?),
            "--grant-moderator" => a.grant_moderator = Some(value("--grant-moderator")?),
            "--grant-coin" => {
                let name = value("--grant-coin")?;
                let silver = value("--grant-coin")?
                    .parse()
                    .map_err(|e| format!("--grant-coin: {e}"))?;
                a.grant_coin = Some((name, silver));
            }
            "--grant-item" => {
                let name = value("--grant-item")?;
                let template = value("--grant-item")?;
                let materials = value("--grant-item")?
                    .split(',')
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
                    .collect();
                a.grant_item = Some((name, template, materials));
            }
            "--audit" => a.audit = true,
            "--place" => {
                let name = value("--place")?;
                let zone = value("--place")?;
                let at: Vec<f32> = value("--place")?
                    .split(',')
                    .map(|n| n.trim().parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("--place: {e}"))?;
                let yaw: f32 = value("--place")?
                    .parse()
                    .map_err(|e| format!("--place: {e}"))?;
                let [x, y, z] = at[..] else {
                    return Err("--place: the place is X,Y,Z".into());
                };
                if !(x.is_finite() && y.is_finite() && z.is_finite() && yaw.is_finite()) {
                    return Err("--place: not a place".into());
                }
                a.place = Some((name, zone, [x, y, z], yaw));
            }
            "--auth-per-minute" => {
                a.auth_per_minute = value("--auth-per-minute")?
                    .parse()
                    .map_err(|e| format!("--auth-per-minute: {e}"))?
            }
            "--party-away" => {
                a.party_away = value("--party-away")?
                    .parse()
                    .map_err(|e| format!("--party-away: {e}"))?
            }
            "--econ-per-second" => {
                a.econ_per_second = value("--econ-per-second")?
                    .parse()
                    .map_err(|e| format!("--econ-per-second: {e}"))?
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
    let operator = a.grant_moderator.is_some()
        || a.grant_coin.is_some()
        || a.grant_item.is_some()
        || a.place.is_some()
        || a.audit;
    if a.zone_secret.is_empty() && !a.migrate_only && !operator {
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

/// An operator's hand (ITEMS.md 4): coin or a made item for a character, through the
/// ledger like every drop, under the reason `grant`, and only what the content knows; or
/// where an offline character stands when it next enters.
async fn operator(db: Db, args: &Args) -> anyhow::Result<()> {
    let econ = gm_hub::economy::Economy::new(db.pool().clone());
    let character = |name: &str| {
        let (db, name) = (db.clone(), name.to_string());
        async move {
            db.character_by_name(&name)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?
                .ok_or_else(|| anyhow::anyhow!("no character called {name:?}"))
        }
    };
    if let Some((name, silver)) = &args.grant_coin {
        let id = character(name).await?;
        econ.grant_coin_as(id, *silver, "grant", 0)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        info!(%name, silver, "coin granted");
    }
    if let Some((name, template, materials)) = &args.grant_item {
        let id = character(name).await?;
        let items = gm_content::items::load_items(&args.content)?;
        let known = items
            .templates
            .iter()
            .find(|t| &t.id == template)
            .ok_or_else(|| anyhow::anyhow!("no template called {template:?}"))?;
        // A stack (MODES.md 11.4): the third word is how many, not what of.
        if items.stack(template).is_some() {
            let quantity: u32 = match materials.as_slice() {
                [n] => n
                    .parse()
                    .map_err(|e| anyhow::anyhow!("{template}: a quantity, not {n:?}: {e}"))?,
                _ => anyhow::bail!("{template} is a stack: --grant-item NAME {template} HOW_MANY"),
            };
            let mut econ = econ;
            econ.set_stacks(&items);
            let item = econ
                .grant_stack(id, template, quantity)
                .await
                .map_err(|e| anyhow::anyhow!("the {template}: {e}"))?;
            info!(%name, %template, quantity, item, "stack granted");
            return Ok(());
        }
        if let Some(m) = materials
            .iter()
            .find(|m| !items.materials.iter().any(|x| &x.id == *m))
        {
            anyhow::bail!("no material called {m:?}");
        }
        let item = econ
            .grant_item(id, template, materials, Some(&known.layers))
            .await
            .map_err(|e| anyhow::anyhow!("the {template}: {e}"))?;
        info!(%name, %template, item, "item granted");
    }
    if let Some((name, zone, at, yaw)) = &args.place {
        let id = character(name).await?;
        if !db
            .place(id, zone, *at, *yaw)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
        {
            anyhow::bail!("{name:?} is playing: a character is placed while it is offline");
        }
        info!(%name, %zone, ?at, yaw, "placed");
    }
    Ok(())
}

/// The books in one line (ECONOMY.md 1.2, ITEMS.md 4): the coin made, in the world and
/// destroyed; how many balances disagree with the ledger or worn items are astray (0:
/// sound); how many items are worn; and the coin that moved under each reason.
async fn audit(db: Db) -> anyhow::Result<()> {
    use sqlx::Row;
    let econ = gm_hub::economy::Economy::new(db.pool().clone());
    let supply = econ.supply().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let unsound = econ.audit().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let worn: i64 = sqlx::query("select count(*) from worn")
        .fetch_one(db.pool())
        .await?
        .try_get(0)?;
    let moved = sqlx::query(
        "select reason, sum(amount)::bigint as coin from coin_ledger group by reason order by reason",
    )
    .fetch_all(db.pool())
    .await?
    .iter()
    .map(|r| Ok(format!("{}={}", r.try_get::<String, _>("reason")?, r.try_get::<i64, _>("coin")?)))
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    println!(
        "audit: created={} circulating={} burned={} unsound={unsound} worn={worn} moved: {}",
        supply.created,
        supply.circulating,
        supply.burned,
        moved.join(" ")
    );
    if unsound != 0 {
        anyhow::bail!("the books are not sound");
    }
    Ok(())
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
    if args.grant_coin.is_some() || args.grant_item.is_some() || args.place.is_some() {
        return operator(db, &args).await;
    }
    if args.audit {
        return audit(db).await;
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
        econ_per_second: args.econ_per_second,
        party_sweep: gm_hub::party::SWEEP,
        party_away: std::time::Duration::from_secs(args.party_away),
        items: gm_content::items::load_items(&args.content)?,
        max_coin_grant: 500,
        models_dir: args.models_dir,
        // Uploads are parsed by this same binary in a child process.
        ingest: IngestMode::Worker(std::env::current_exe()?),
        ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
        start_zone: args.start_zone,
        blurbs: gm_content::load_blurbs(&args.content)?,
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
