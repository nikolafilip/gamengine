//! One bot: connect, handshake, receive the content, then a fixed-step loop of predicted
//! inputs and snapshots.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use gm_bsp::Bsp;
use gm_core::build::Sheet;
use gm_core::tick::TickRate;
use gm_core::vocab::EntityId;
use gm_net::PROTOCOL_VERSION;
use gm_net::client::{ClientState, ClientStats};
use gm_net::control::WebAddr;
use gm_net::control::{self, BodyKind, BuildChoice, EncounterState, FromClient, FromZone};
use gm_net::link::{Link, web_connect};
use gm_net::transport::{SERVER_NAME, web_transport_config};
use tokio::time::Instant;
use tracing::{debug, info};

use crate::brain::{Behaviour, Brain, View, counter_pick, dominant_enemy_aspects};
use crate::raid::Raid;

/// Counter-picking bots look at the enemy every this many seconds (after the first look).
pub const COUNTER_PICK_PERIOD_S: u32 = 10;

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub name: String,
    pub seed: u64,
    pub behaviour: Behaviour,
    pub rate: TickRate,
    /// Stop after this many client ticks (0 = run until `shutdown`).
    pub run_ticks: u32,
    /// Preset build to ask for (`None`: the zone's default).
    pub build: Option<String>,
    /// Team to ask for (0 = let the zone balance).
    pub team: u8,
    /// Re-spec to the preset that counters the enemy's aspects (MATRIX.md 11).
    pub counter_pick: bool,
    /// Ask to travel to this zone after `travel_after_ticks` (hub flows only).
    pub travel_to: Option<String>,
    pub travel_after_ticks: u32,
    /// Walk to this tile of the map's market (counted across its grids) and open a stall
    /// there, or on the next free one (ECONOMY.md 7).
    pub stall_tile: Option<u32>,
    /// How the view moves (ANTICHEAT.md 10): the brain's wish at once, a hand, or a cheat.
    pub aim: crate::aim::AimModel,
    /// Report the first enemy seen after this many ticks (0 = never; ANTICHEAT.md 5).
    pub report_after_ticks: u32,
    /// Say this line in the zone's chat every so many seconds, at the zone's own tick
    /// rate (CLIENT.md 5).
    pub say: Option<(String, f32)>,
    /// What the bot does about other people (PARTY.md 9).
    pub social: Social,
}

/// A bot as a person among people: it can ask somebody into a party, join whoever asks
/// it, trade with whoever asks, and answer what is said to it.
#[derive(Clone, Debug, Default)]
pub struct Social {
    /// Ask the character of this name into a party, again every few seconds until the
    /// zone says the two are in one.
    pub invite: Option<String>,
    /// Join whoever invites, ask back whoever asks to trade, answer a party's line with
    /// a line and a whisper with a whisper.
    pub sociable: bool,
    /// Where the trades the zone says the hub opened are handed to (the bot's hub
    /// session sits elsewhere: `hub_flow`).
    pub trades: Option<tokio::sync::mpsc::UnboundedSender<i64>>,
}

/// Why the bot loop ended.
#[derive(Clone, Debug)]
pub enum BotExit {
    Done,
    /// The zone handed over a ticket for another zone.
    Travel(Box<gm_hub_proto::protocol::ZoneTicket>),
}

#[derive(Clone, Debug, Default)]
pub struct BotReport {
    pub name: String,
    pub entity: EntityId,
    pub team: u8,
    pub ticks: u32,
    pub secs: f64,
    pub client: ClientStats,
    pub udp_tx_bytes: u64,
    pub udp_rx_bytes: u64,
    pub rtt_ms: f64,
    pub kills_seen: u32,
    pub deaths_seen: u32,
    pub own_kills: u32,
    pub own_deaths: u32,
    pub final_health: i32,
    pub send_failures: u64,
    pub others_seen_max: usize,
    /// Re-specs requested and the build the bot ended with.
    pub respecs: u32,
    pub final_build: String,
    /// Avatar models the zone announced (MODELS.md 7): the most distinct ids worn at once,
    /// the own one at the end, the revocations heard, and whether anybody was still
    /// announced wearing a revoked model afterwards.
    pub models_seen: usize,
    pub own_model: Option<[u8; 32]>,
    pub revocations: u32,
    pub revoked_still_worn: bool,
    /// The most players the zone listed at once (roster and joins minus leaves).
    pub roster: usize,
    /// The bot opened a stall; and the stalls the zone showed when it left.
    pub stall_opened: bool,
    pub stalls_seen: usize,
    /// Ground covered, in world units (a bot that never moves is stuck).
    pub travelled: f32,
    /// A raid leader's run (COMPANIONS.md 14): the most companions it commanded at once,
    /// how many of them were hired rather than lent, the orders it gave and the ones the
    /// zone refused, the encounters it was told were cleared (name, seconds) and reset,
    /// what a kill gave it, the trials judged on it (key, passed, why not), and whether it
    /// finished the map.
    /// The party as the zone last said it (the hub's word), and the lines heard through
    /// the hub: channel, from whom, what.
    pub party: Vec<String>,
    pub lines: Vec<(u8, String, String)>,
    pub squad_max: usize,
    pub squad_hired: usize,
    pub orders: u32,
    pub orders_refused: u32,
    pub cleared: Vec<(String, u32)>,
    pub resets: u32,
    pub loot: Vec<String>,
    pub coin: u32,
    pub trials: Vec<(String, bool, String)>,
    pub raid_done: bool,
    /// The zone took this bot's report (ANTICHEAT.md 5).
    pub report_accepted: bool,
    /// Chat lines heard, the first 32 of them: the speaker's entity (0 = the zone) and
    /// the text.
    pub heard: Vec<(u32, String)>,
    /// The zone kicked the bot: why.
    pub kicked: Option<String>,
    /// Bodies the zone announced as creatures, and the most seen with their health.
    pub creatures_announced: usize,
    pub creature_health_seen: usize,
}

impl BotReport {
    pub fn tx_bytes_per_s(&self) -> f64 {
        self.udp_tx_bytes as f64 / self.secs.max(1e-3)
    }

    pub fn rx_bytes_per_s(&self) -> f64 {
        self.udp_rx_bytes as f64 / self.secs.max(1e-3)
    }
}

/// Connect to `server` through `endpoint` (which must trust the zone's certificate) and play.
pub async fn run_bot(
    endpoint: &quinn::Endpoint,
    server: SocketAddr,
    cfg: BotConfig,
    world: Arc<Bsp>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<BotReport> {
    let (report, _, _) = run_bot_with_token(
        endpoint,
        server,
        cfg,
        Vec::new(),
        move |_| Ok(world.clone()),
        shutdown,
    )
    .await?;
    Ok(report)
}

/// Like [`run_bot`] with a hub token; the map is loaded by name from `Welcome` through
/// `load_map`. Returns the report, why the loop ended and the map name.
pub async fn run_bot_with_token(
    endpoint: &quinn::Endpoint,
    server: SocketAddr,
    cfg: BotConfig,
    token: Vec<u8>,
    load_map: impl Fn(&str) -> anyhow::Result<Arc<Bsp>>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<(BotReport, BotExit, String)> {
    let conn = endpoint.connect(server, SERVER_NAME)?.await?;
    run_bot_on_link(Link::Quic(conn), cfg, token, load_map, shutdown).await
}

/// The same bot through a zone's WebTransport listener (WEB.md 7): what a browser's
/// session carries, without a browser.
pub async fn run_bot_web(
    web: &WebAddr,
    cfg: BotConfig,
    token: Vec<u8>,
    load_map: impl Fn(&str) -> anyhow::Result<Arc<Bsp>>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<(BotReport, BotExit, String)> {
    let (endpoint, conn) = web_connect(web, web_transport_config()).await?;
    let result = run_bot_on_link(conn, cfg, token, load_map, shutdown).await;
    endpoint.wait_idle().await;
    result
}

/// The bot on an established connection of either transport.
pub async fn run_bot_on_link(
    conn: Link,
    cfg: BotConfig,
    token: Vec<u8>,
    load_map: impl Fn(&str) -> anyhow::Result<Arc<Bsp>>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<(BotReport, BotExit, String)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    control::send(
        &mut send,
        &FromClient::Hello {
            version: PROTOCOL_VERSION as u16,
            name: cfg.name.clone(),
            token,
            build: cfg.build.clone().map(BuildChoice::Preset),
            team: cfg.team,
        },
    )
    .await?;
    let welcome = control::recv(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("server closed before Welcome"))?;
    let (entity, hz, map_name) = match welcome {
        FromZone::Welcome {
            entity, hz, map, ..
        } => (entity, hz, map),
        FromZone::Reject(reason) => anyhow::bail!("rejected: {reason}"),
        other => anyhow::bail!("unexpected handshake message {other:?}"),
    };
    let world = load_map(&map_name)?;
    let mut exit = BotExit::Done;
    let content = control::recv(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("server closed before Content"))?;
    let (pack, own, team) = match content {
        FromZone::Content {
            pack, own, team, ..
        } => (pack, own, team),
        other => anyhow::bail!("expected Content, got {other:?}"),
    };
    let rate = TickRate::new(hz as u32);
    info!(name = %cfg.name, entity, hz, team, "bot joined");
    // How often it speaks, in this zone's ticks.
    let say = cfg
        .say
        .as_ref()
        .map(|(line, secs)| (line.clone(), ((secs * hz as f32).round() as u32).max(1)));

    let mut pack = pack;
    let build_name = |pack: &gm_core::build::ContentPack, b: &gm_core::build::Build| -> String {
        pack.builds
            .iter()
            .find(|nb| &nb.build == b)
            .map_or_else(|| "custom".to_string(), |nb| nb.name.clone())
    };
    let mut current_build = build_name(&pack, &own);
    let mut client = ClientState::new(entity, rate, Sheet::new(own, &pack, team));
    let mut brain = Brain::new(cfg.seed, cfg.behaviour);
    brain.hz = rate.hz();
    let mut aimer = crate::aim::Aimer::new(cfg.aim, cfg.seed);
    let mut reported = false;
    // A raid leader thinks with `gm_ai` instead: the nav grid is flooded here, once.
    let mut raid = (cfg.behaviour == Behaviour::Raid).then(|| Raid::new(&world, cfg.seed));
    // The tick the raid was finished at: the loop runs two seconds more, for the last
    // kill's loot and verdicts to arrive.
    let mut raid_done_at: Option<u32> = None;
    // A vendor walks to its tile; a taken tile sends it to the next.
    let tiles: Vec<glam::Vec3> = world
        .stall_grids()
        .iter()
        .flat_map(|g| {
            (0..g.rows).flat_map(move |row| {
                (0..g.cols).filter_map(move |col| g.centre(g.base_x + col, g.base_y + row))
            })
        })
        .collect();
    let mut stall_tile = cfg.stall_tile.filter(|_| !tiles.is_empty());
    // The tick of the last stall request, and whether the zone has answered it.
    let mut stall_asked: Option<u32> = None;
    let mut stall_answered = true;
    let mut stalls_seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if let Some(t) = stall_tile {
        brain.goal = Some(tiles[t as usize % tiles.len()]);
    }
    // Who is here and what they wear, as the zone announces it.
    let mut wearing: std::collections::HashMap<u32, Option<[u8; 32]>> =
        std::collections::HashMap::new();
    let mut revoked: Vec<[u8; 32]> = Vec::new();
    let mut own_model = None;
    let mut next_pick = COUNTER_PICK_PERIOD_S * rate.hz();
    let mut report = BotReport {
        name: cfg.name.clone(),
        entity,
        team,
        ..Default::default()
    };
    let start = Instant::now();
    let period = rate.period();
    let mut next = start;
    let mut ticks: u32 = 0;
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        let sleep = tokio::time::sleep_until(next);
        tokio::select! {
            _ = &mut shutdown => break,
            _ = sleep => {
                ticks += 1;
                let t = client.render_tick(0.0);
                let others = client.others_at(t);
                report.others_seen_max = report.others_seen_max.max(
                    others.iter().filter(|e| e.kind == gm_net::snapshot::EntityKind::Player).count(),
                );
                let input = match raid.as_mut() {
                    Some(raid) => {
                        report.creature_health_seen = report.creature_health_seen.max(
                            others
                                .iter()
                                .filter(|e| {
                                    e.health.is_some()
                                        && matches!(
                                            raid.kinds.get(&e.id),
                                            Some(BodyKind::Creature { .. })
                                        )
                                })
                                .count(),
                        );
                        let (input, order) = raid.think(&client, &others, &world, &pack, team);
                        if let Some(order) = order {
                            report.orders += 1;
                            let _ = control::send(&mut send, &order).await;
                        }
                        if raid.done() && raid_done_at.is_none() {
                            info!(name = %cfg.name, ticks, "raid done");
                            raid_done_at = Some(ticks);
                            report.raid_done = true;
                        }
                        input
                    }
                    None => brain.think(&View {
                        me: &client.mover,
                        kit: &client.sheet.kit,
                        frame: client.sheet.build.frame,
                        team,
                        alive: client.own_alive,
                        health: client.own_health,
                        others: &others,
                        tick: client.tick.wrapping_add(1),
                    }),
                };
                // The view goes where this bot's aim model takes it.
                let input = aimer.apply(
                    input,
                    &client,
                    world.as_ref(),
                    team,
                    &others,
                    client.tick.wrapping_add(1),
                );
                if let Some((line, every)) = &say
                    && ticks.is_multiple_of(*every)
                {
                    let _ = control::send(&mut send, &FromClient::Chat(line.clone())).await;
                }
                if cfg.report_after_ticks > 0
                    && !reported
                    && ticks >= cfg.report_after_ticks
                    && let Some(enemy) = others.iter().find(|e| {
                        e.kind == gm_net::snapshot::EntityKind::Player
                            && e.alive()
                            && (team == 0 || e.team() != team)
                    })
                {
                    reported = true;
                    info!(name = %cfg.name, target = enemy.id, "reporting");
                    let report = FromClient::Report {
                        target: enemy.id,
                        reason: gm_net::control::ReportReason::Aim,
                    };
                    let _ = control::send(&mut send, &report).await;
                }
                if let Some(to) = &cfg.travel_to
                    && cfg.travel_after_ticks > 0
                    && ticks == cfg.travel_after_ticks
                {
                    info!(name = %cfg.name, %to, "asking to travel");
                    let _ = control::send(&mut send, &FromClient::Travel(to.clone())).await;
                }
                if cfg.counter_pick && ticks >= next_pick {
                    next_pick = ticks + COUNTER_PICK_PERIOD_S * rate.hz();
                    let enemy = dominant_enemy_aspects(team, &others);
                    if let Some(name) = counter_pick(&pack, &current_build, enemy) {
                        info!(name = %cfg.name, from = %current_build, to = %name, "counter-pick");
                        let _ = control::send(&mut send, &FromClient::Respec(BuildChoice::Preset(name)))
                            .await;
                        report.respecs += 1;
                    }
                }
                // On the tile (its centre may be taken by whoever got there first): ask for
                // the stall. The zone drops a request that comes within a second of the last
                // one: after a refusal the next one waits that second out, and a request that
                // got no answer is repeated after three.
                let wait = if stall_answered {
                    rate.hz() + rate.hz() / 8
                } else {
                    3 * rate.hz()
                };
                if let Some(goal) = brain.goal
                    && !report.stall_opened
                    && stall_asked.is_none_or(|t| ticks - t > wait)
                    && client.own_alive
                    && (client.mover.mv.origin - goal).truncate().abs().max_element() < 56.0
                {
                    stall_asked = Some(ticks);
                    stall_answered = false;
                    let _ = control::send(&mut send, &FromClient::StallOpen).await;
                }
                // Somebody to ask into a party: asked every five seconds until the zone
                // says the two are in one.
                if let Some(who) = &cfg.social.invite
                    && ticks % (5 * rate.hz()) == rate.hz()
                    && !report.party.iter().any(|m| m == who)
                {
                    let ask = FromClient::PartyInvite { name: who.clone() };
                    let _ = control::send(&mut send, &ask).await;
                }
                // What the zone has announced so far.
                report.roster = report.roster.max(wearing.len());
                if ticks.is_multiple_of(16) {
                    let distinct: std::collections::HashSet<&[u8; 32]> =
                        wearing.values().flatten().collect();
                    report.models_seen = report.models_seen.max(distinct.len());
                    report.revoked_still_worn |=
                        wearing.values().flatten().any(|m| revoked.contains(m));
                    own_model = wearing.get(&entity).copied().flatten();
                }
                let before = client.mover.mv.origin;
                let datagram = client.local_tick(world.as_ref(), input);
                let step = (client.mover.mv.origin - before).truncate().length();
                // A correction or a respawn is not walking.
                if step < 32.0 {
                    report.travelled += step;
                }
                if conn.send_datagram(Bytes::from(datagram.encode())).is_err() {
                    report.send_failures += 1;
                }
                client.prune(t);
                next += period;
                // Never try to catch up more than a second: reset the timeline instead.
                let now = Instant::now();
                if now.saturating_duration_since(next) > std::time::Duration::from_secs(1) {
                    next = now;
                }
                if cfg.run_ticks != 0 && ticks >= cfg.run_ticks {
                    break;
                }
                if raid_done_at.is_some_and(|at| ticks >= at + 2 * rate.hz()) {
                    break;
                }
            }
            dg = conn.read_datagram() => {
                match dg {
                    Ok(bytes) => {
                        if let Err(e) = client.on_snapshot(world.as_ref(), &bytes) {
                            debug!(name = %cfg.name, "snapshot dropped: {e}");
                        }
                    }
                    Err(e) => {
                        info!(name = %cfg.name, "connection closed: {e}");
                        break;
                    }
                }
            }
            msg = control::recv(&mut recv) => {
                match msg {
                    Ok(Some(FromZone::Killed { victim, killer })) => {
                        report.kills_seen += 1;
                        if victim == entity {
                            report.own_deaths += 1;
                        }
                        if killer == entity {
                            report.own_kills += 1;
                        }
                    }
                    Ok(Some(FromZone::BuildApplied(build))) => {
                        if build != client.sheet.build {
                            current_build = build_name(&pack, &build);
                            client.set_sheet(Sheet::new(build, &pack, team));
                        }
                    }
                    // Content tuned under the zone (GM.md 3): the same build on the new
                    // numbers; the prediction goes on.
                    Ok(Some(FromZone::Content { pack: tuned, own, .. })) => {
                        pack = tuned;
                        current_build = build_name(&pack, &own);
                        client.set_sheet(Sheet::new(own, &pack, team));
                    }
                    Ok(Some(FromZone::TravelTicket { zone, addr, cert_der, token, web })) => {
                        let addr: SocketAddr = match addr.parse() {
                            Ok(a) => a,
                            Err(e) => {
                                info!(name = %cfg.name, "bad travel address {addr}: {e}");
                                continue;
                            }
                        };
                        let Ok(token) = bitcode::decode(&token) else {
                            info!(name = %cfg.name, "bad travel token");
                            continue;
                        };
                        info!(name = %cfg.name, %zone, %addr, "travel ticket");
                        exit = BotExit::Travel(Box::new(gm_hub_proto::protocol::ZoneTicket {
                            zone,
                            addr,
                            cert_der,
                            token,
                            web,
                        }));
                        break;
                    }
                    Ok(Some(FromZone::TravelRefused(reason))) => {
                        info!(name = %cfg.name, "travel refused: {reason}");
                    }
                    Ok(Some(FromZone::Kick(reason))) => {
                        info!(name = %cfg.name, "kicked: {reason}");
                        report.kicked = Some(reason);
                        break;
                    }
                    Ok(Some(FromZone::StallResult(result))) => {
                        stall_answered = true;
                        match result {
                        Ok(()) => {
                            report.stall_opened = true;
                            info!(name = %cfg.name, "stall opened");
                        }
                        Err(why) if why.contains("already have a stall") => {
                            // The stall from before the restart stands: the bot keeps it
                            // rather than asking the zone every second.
                            report.stall_opened = true;
                            info!(name = %cfg.name, "stall stands from before");
                        }
                        Err(why) => {
                            // Somebody was faster: the next tile.
                            debug!(name = %cfg.name, "stall refused: {why}");
                            if let Some(t) = stall_tile.as_mut() {
                                *t += 1;
                                brain.goal = Some(tiles[*t as usize % tiles.len()]);
                            }
                        }
                        }
                    }
                    Ok(Some(FromZone::Stalls(list))) => {
                        stalls_seen = list.iter().map(|s| s.id).collect();
                    }
                    Ok(Some(FromZone::StallOpened(stall))) => {
                        stalls_seen.insert(stall.id);
                        // Where the bot's own stall stands, for whoever sends it a buyer.
                        if stall.owner == cfg.name {
                            info!(
                                name = %cfg.name,
                                stall = stall.id,
                                x = stall.pos[0],
                                y = stall.pos[1],
                                z = stall.pos[2],
                                yaw = stall.yaw,
                                "stall stands"
                            );
                        }
                    }
                    Ok(Some(FromZone::StallClosed(id))) => {
                        stalls_seen.remove(&id);
                    }
                    // People together (PARTY.md 4): the party as the hub has it, and
                    // what a sociable bot does when it is asked something.
                    Ok(Some(FromZone::Party(names))) => {
                        info!(name = %cfg.name, party = %names.join(","), "party");
                        report.party = names;
                    }
                    Ok(Some(FromZone::Invited { from })) => {
                        info!(name = %cfg.name, %from, "invited");
                        if cfg.social.sociable {
                            let join = FromClient::PartyAnswer { from, join: true };
                            let _ = control::send(&mut send, &join).await;
                        }
                    }
                    Ok(Some(FromZone::Heard { channel, from, text })) => {
                        info!(name = %cfg.name, channel, %from, %text, "heard through the hub");
                        let mine = from == cfg.name;
                        if cfg.social.sociable && !mine {
                            let answer = match channel {
                                control::CHANNEL_PARTY => Some(FromClient::PartySay("aye".into())),
                                control::CHANNEL_WHISPER => Some(FromClient::Whisper {
                                    to: from.replace(' ', ""),
                                    text: "psst yourself".into(),
                                }),
                                _ => None,
                            };
                            // (Its own "aye" comes back to it and is not answered.)
                            if let Some(answer) = answer.filter(|_| text != "aye") {
                                let _ = control::send(&mut send, &answer).await;
                            }
                        }
                        if report.lines.len() < 64 {
                            report.lines.push((channel, from, text));
                        }
                    }
                    Ok(Some(FromZone::TradeAsked { from })) => {
                        info!(name = %cfg.name, from, "asked to trade");
                        if cfg.social.sociable {
                            let back = FromClient::TradeAsk { with: from };
                            let _ = control::send(&mut send, &back).await;
                        }
                    }
                    Ok(Some(FromZone::TradeOpened { trade, with })) => {
                        info!(name = %cfg.name, trade, %with, "trade opened");
                        if let Some(trades) = &cfg.social.trades {
                            let _ = trades.send(trade);
                        }
                    }
                    Ok(Some(FromZone::ReportResult(result))) => {
                        info!(name = %cfg.name, ?result, "report answered");
                        report.report_accepted = result.is_ok();
                    }
                    Ok(Some(FromZone::ChatFrom { from, text })) => {
                        if from != entity && report.heard.len() < 32 {
                            // Said as it is heard: whoever scripted the run need not
                            // wait for the bot's end to know.
                            info!(name = %cfg.name, from, %text, "heard");
                            report.heard.push((from, text));
                        }
                    }
                    Ok(Some(FromZone::Roster(players))) => {
                        report.creatures_announced = players
                            .iter()
                            .filter(|p| matches!(p.kind, BodyKind::Creature { .. }))
                            .count();
                        if let Some(raid) = raid.as_mut() {
                            raid.kinds = players.iter().map(|p| (p.id, p.kind)).collect();
                        }
                        wearing = players.into_iter().map(|p| (p.id, p.model)).collect();
                    }
                    Ok(Some(FromZone::PlayerInfo { id, model, kind, .. })) => {
                        if matches!(kind, BodyKind::Creature { .. }) && !wearing.contains_key(&id) {
                            report.creatures_announced += 1;
                        }
                        if let Some(raid) = raid.as_mut() {
                            raid.kinds.insert(id, kind);
                        }
                        wearing.insert(id, model);
                    }
                    Ok(Some(FromZone::PlayerLeft(id))) => {
                        if let Some(raid) = raid.as_mut() {
                            raid.kinds.remove(&id);
                        }
                        wearing.remove(&id);
                    }
                    Ok(Some(FromZone::Squad(entries))) => {
                        report.squad_max = report.squad_max.max(entries.len());
                        report.squad_hired = report
                            .squad_hired
                            .max(entries.iter().filter(|e| !e.recruit).count());
                        if let Some(raid) = raid.as_mut() {
                            raid.squad = entries;
                        }
                    }
                    Ok(Some(FromZone::OrderRefused(why))) => {
                        report.orders_refused += 1;
                        debug!(name = %cfg.name, "order refused: {why}");
                    }
                    Ok(Some(FromZone::Encounter { name, state })) => {
                        info!(name = %cfg.name, encounter = %name, ?state, "encounter");
                        match state {
                            EncounterState::Cleared { secs } => report.cleared.push((name, secs)),
                            EncounterState::Reset => report.resets += 1,
                            EncounterState::Engaged => {}
                        }
                    }
                    Ok(Some(FromZone::Loot { items, coin, .. })) => {
                        info!(name = %cfg.name, ?items, coin, "loot");
                        report.loot.extend(items);
                        report.coin += coin;
                    }
                    Ok(Some(FromZone::Trial { key, passed, detail, secs, .. })) => {
                        info!(name = %cfg.name, trial = %key, passed, secs, %detail, "trial");
                        report.trials.push((key, passed, detail));
                    }
                    Ok(Some(FromZone::ModelRevoked(model))) => {
                        report.revocations += 1;
                        revoked.push(model);
                        for m in wearing.values_mut() {
                            if *m == Some(model) {
                                *m = None;
                            }
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }
    report.deaths_seen = report.kills_seen;
    let stats = conn.stats();
    report.ticks = ticks;
    report.secs = start.elapsed().as_secs_f64();
    report.client = client.stats;
    report.udp_tx_bytes = stats.udp_tx.bytes;
    report.udp_rx_bytes = stats.udp_rx.bytes;
    report.rtt_ms = conn.rtt().as_secs_f64() * 1000.0;
    report.final_health = client.own_health;
    report.final_build = current_build;
    report.stalls_seen = stalls_seen.len();
    report.own_model = own_model;
    let _ = control::send(&mut send, &FromClient::Bye).await;
    conn.close(0, b"done");
    // Give the Bye a moment to leave before the endpoint is dropped.
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok((report, exit, map_name))
}
