//! A bot that plays through the hub (HUB.md): register or log in, pick or create a character,
//! enter a zone with a ticket, travel when told, log out. Used by the handoff acceptance test
//! and by `gm-bot --hub`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gm_bsp::Bsp;
use gm_core::tick::TickRate;
use gm_hub_proto::protocol::{
    BuildChoice, EconOp, EconReply, HubRequest, HubResponse, SessionId, ZoneTicket,
};
use gm_hub_proto::{HubClient, HubClientError};
use gm_net::transport::client_config;
use quinn::rustls::pki_types::CertificateDer;
use tracing::info;

use crate::bot::{BotConfig, BotExit, BotReport, run_bot_with_token};

#[derive(Clone, Debug)]
pub struct HubFlowConfig {
    pub hub: SocketAddr,
    pub hub_cert_der: Vec<u8>,
    pub email: String,
    pub password: String,
    /// Register first (the account may already exist: then log in).
    pub register: bool,
    pub character: String,
    /// Preset for a character that does not exist yet.
    pub preset: String,
    pub zone: String,
    /// After this many seconds in the first zone, ask to travel to `travel_to`.
    pub travel_after: Option<Duration>,
    pub travel_to: Option<String>,
    /// Where `<map>.bsp` files live (the bot predicts against the zone's map).
    pub maps_dir: PathBuf,
    pub bot: BotConfig,
    /// Total play time before logging out; zero = do not enter a zone at all.
    pub play: Duration,
    /// List the character for hire at this price first (ECONOMY.md 11).
    pub list_for_hire: Option<i64>,
    /// Hire this many avatars from the tavern, in its order, before entering (the character
    /// pays; slots it has already filled are not hired again).
    pub hire: usize,
}

/// Timings of the round trip (HUB.md 5).
#[derive(Clone, Debug, Default)]
pub struct HubFlowReport {
    pub login_ms: f64,
    pub enter_ms: f64,
    pub travel_ms: f64,
    pub logout_ms: f64,
    pub zones: Vec<String>,
    pub reports: Vec<BotReport>,
    pub character: i64,
    /// The names of the avatars in the character's squad when it entered (COMPANIONS.md
    /// 3.3), and what it held and had passed when it logged out.
    pub squad: Vec<String>,
    pub coin: i64,
    pub items: Vec<String>,
    pub trials: Vec<String>,
}

fn load_map(dir: &Path, map: &str) -> anyhow::Result<Arc<Bsp>> {
    let path = dir.join(format!("{map}.bsp"));
    Ok(Arc::new(Bsp::load(&path).map_err(|e| {
        anyhow::anyhow!("loading {}: {e}", path.display())
    })?))
}

/// Register or log in. The hub hashes eight passwords at a time and answers `Busy` to the
/// ninth (HUB.md 3): a swarm waits its turn.
async fn login(hub: &HubClient, cfg: &HubFlowConfig) -> anyhow::Result<SessionId> {
    let mut register = cfg.register;
    for attempt in 0..200u32 {
        let req = if register {
            HubRequest::Register {
                email: cfg.email.clone(),
                password: cfg.password.clone(),
            }
        } else {
            HubRequest::Login {
                email: cfg.email.clone(),
                password: cfg.password.clone(),
            }
        };
        match hub.request(&req).await {
            Ok(HubResponse::Session { session, .. }) => return Ok(session),
            Ok(other) => anyhow::bail!("unexpected login answer {other:?}"),
            Err(HubClientError::Refused(gm_hub_proto::protocol::HubError::Taken)) if register => {
                register = false;
            }
            Err(HubClientError::Refused(gm_hub_proto::protocol::HubError::Busy)) => {
                tokio::time::sleep(Duration::from_millis(100 + (attempt % 7) as u64 * 40)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("the hub stayed busy")
}

/// Connect to the zone a ticket names and play until the bot exits.
async fn play_ticket(
    ticket: &ZoneTicket,
    cfg: &HubFlowConfig,
    name: &str,
    run_for: Duration,
) -> anyhow::Result<(BotReport, BotExit, String)> {
    let bind: SocketAddr = if ticket.addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_config(&[CertificateDer::from(
        ticket.cert_der.clone(),
    )])?);
    let token = bitcode::encode(&ticket.token);
    let bot = BotConfig {
        name: name.to_string(),
        ..cfg.bot.clone()
    };
    let maps_dir = cfg.maps_dir.clone();
    let (report, exit, map) = run_bot_with_token(
        &endpoint,
        ticket.addr,
        bot,
        token,
        move |map| load_map(&maps_dir, map),
        tokio::time::sleep(run_for),
    )
    .await?;
    endpoint.wait_idle().await;
    Ok((report, exit, map))
}

/// The whole trip: login → zone → (travel →) logout.
pub async fn run_hub_flow(cfg: HubFlowConfig) -> anyhow::Result<HubFlowReport> {
    let mut out = HubFlowReport::default();
    let t0 = Instant::now();
    let hub = HubClient::connect_with_cert(cfg.hub, cfg.hub_cert_der.clone()).await?;
    let session = login(&hub, &cfg).await?;
    out.login_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let characters = match hub.request(&HubRequest::Characters { session }).await? {
        HubResponse::Characters(c) => c,
        other => anyhow::bail!("unexpected characters answer {other:?}"),
    };
    let character = match characters.iter().find(|c| c.name == cfg.character) {
        Some(c) => c.id,
        None => {
            match hub
                .request(&HubRequest::CreateCharacter {
                    session,
                    name: cfg.character.clone(),
                    build: BuildChoice::Preset(cfg.preset.clone()),
                })
                .await?
            {
                HubResponse::Character(c) => c.id,
                other => anyhow::bail!("unexpected create answer {other:?}"),
            }
        }
    };
    out.character = character;
    let econ = |op: EconOp| HubRequest::Econ {
        session,
        character,
        op,
    };
    if let Some(price) = cfg.list_for_hire {
        hub.request(&econ(EconOp::HireList { price })).await?;
        info!(character = %cfg.character, price, "listed for hire");
    }
    if cfg.hire > 0 {
        let have = match hub.request(&econ(EconOp::Squad)).await? {
            HubResponse::Econ(EconReply::Squad(s)) => s,
            other => anyhow::bail!("unexpected squad answer {other:?}"),
        };
        let tavern = match hub.request(&econ(EconOp::Tavern)).await? {
            HubResponse::Econ(EconReply::Tavern(t)) => t,
            other => anyhow::bail!("unexpected tavern answer {other:?}"),
        };
        let mut hired = have.len();
        for entry in tavern {
            if hired >= cfg.hire {
                break;
            }
            if have.iter().any(|h| h.character == entry.character) {
                continue;
            }
            // Its own account's avatars and a full squad are refused; the next one then.
            match hub
                .request(&econ(EconOp::Hire {
                    avatar: entry.character,
                }))
                .await
            {
                Ok(_) => {
                    info!(avatar = %entry.name, price = entry.price, "hired");
                    hired += 1;
                }
                Err(HubClientError::Refused(why)) => {
                    info!(avatar = %entry.name, "not hired: {why}");
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    if let HubResponse::Econ(EconReply::Squad(s)) = hub.request(&econ(EconOp::Squad)).await? {
        out.squad = s.into_iter().map(|h| h.name).collect();
    }
    if cfg.play.is_zero() {
        hub.ok(&HubRequest::Logout { session }).await?;
        hub.close();
        return Ok(out);
    }

    let t1 = Instant::now();
    let ticket = match hub
        .request(&HubRequest::Enter {
            session,
            character,
            zone: cfg.zone.clone(),
        })
        .await?
    {
        HubResponse::Ticket(t) => t,
        other => anyhow::bail!("unexpected enter answer {other:?}"),
    };
    info!(zone = %ticket.zone, addr = %ticket.addr, "ticket");
    let name = cfg.character.clone();
    let mut remaining = cfg.play;
    let first_run = cfg.travel_after.unwrap_or(cfg.play).min(cfg.play);
    let mut bot_cfg = cfg.clone();
    bot_cfg.bot.travel_to = cfg.travel_to.clone();
    bot_cfg.bot.travel_after_ticks = cfg
        .travel_after
        .map(|d| (d.as_secs_f64() * TickRate::COMBAT.hz() as f64) as u32)
        .unwrap_or(0);
    let (report, exit, map) =
        play_ticket(&ticket, &bot_cfg, &name, first_run + Duration::from_secs(5)).await?;
    out.enter_ms = t1.elapsed().as_secs_f64() * 1000.0 - first_run.as_secs_f64() * 1000.0;
    out.enter_ms = out.enter_ms.max(0.0);
    out.zones.push(format!("{}:{}", ticket.zone, map));
    out.reports.push(report);
    remaining = remaining.saturating_sub(first_run);
    if let BotExit::Travel(next) = exit {
        let t2 = Instant::now();
        let mut no_travel = bot_cfg.clone();
        no_travel.bot.travel_to = None;
        no_travel.bot.travel_after_ticks = 0;
        let (report, _exit, map) = play_ticket(&next, &no_travel, &name, remaining).await?;
        out.travel_ms = t2.elapsed().as_secs_f64() * 1000.0 - remaining.as_secs_f64() * 1000.0;
        out.travel_ms = out.travel_ms.max(0.0);
        out.zones.push(format!("{}:{}", next.zone, map));
        out.reports.push(report);
    }
    // What the trip left the character with.
    if let HubResponse::Econ(EconReply::Holder { coin, items }) =
        hub.request(&econ(EconOp::Inventory)).await?
    {
        out.coin = coin;
        out.items = items
            .into_iter()
            // A material is named `layer/name` already.
            .flat_map(|i| i.components.into_iter().map(|(_, material)| material))
            .collect();
    }
    if let HubResponse::Trials(t) = hub
        .request(&HubRequest::Trials { session, character })
        .await?
    {
        out.trials = t.into_iter().map(|(key, _)| key).collect();
    }
    let t3 = Instant::now();
    hub.ok(&HubRequest::Logout { session }).await?;
    out.logout_ms = t3.elapsed().as_secs_f64() * 1000.0;
    hub.close();
    Ok(out)
}
