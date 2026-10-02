//! Talking to a hub from the command line (HUB.md 3, MODELS.md 6, 10): what a creator does
//! with models, what a moderator does with the queue, and seeding a hub with generated
//! avatars for the acceptance run. The same protocol the client speaks; no HTTP.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use gm_hub_proto::protocol::{
    BuildChoice, CharacterSummary, HubError, HubRequest, HubResponse, MAX_MODEL_BYTES, ModOp,
    ModelId, ModelStatus, ModelSummary, ReasonCode, SessionId, TOS_VERSION,
};
use gm_hub_proto::{HubClient, HubClientError};
use gm_model::{id_from_hex, id_hex, rig};

/// The upload terms, version 1 (MODELS.md 10).
pub const TERMS: &str = "I made this model or hold the rights to use it here. It does not depict a real person \
without their consent. It is not sexual, hateful or otherwise unlawful. I understand it may be refused or removed, \
with a stated reason, and that repeated violations end my upload privileges.";

#[derive(Args, Clone)]
pub struct HubArgs {
    /// The hub's address.
    #[arg(long, default_value = "127.0.0.1:4400")]
    pub hub: SocketAddr,
    /// The hub's certificate (`gm-hub --cert-out`).
    #[arg(long, default_value = "hub-cert.der")]
    pub hub_cert: PathBuf,
    #[arg(long)]
    pub user: String,
    /// The account's password (or `GM_PASSWORD`).
    #[arg(long, env = "GM_PASSWORD", hide_env_values = true)]
    pub password: String,
}

#[derive(Subcommand)]
pub enum HubModelCmd {
    /// Upload a .glb for a frame. Run `gm-tools model ingest` first: it is the same check.
    Upload {
        file: PathBuf,
        #[arg(long)]
        frame: String,
        /// Certify the upload terms (printed without this flag).
        #[arg(long)]
        certify: bool,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// The models this account holds, with their status and any stated reason.
    List {
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Put a model on an offline character, or take it off with --none.
    Wear {
        #[arg(long)]
        character: String,
        /// Model id in hex.
        #[arg(long, conflicts_with = "none")]
        model: Option<String>,
        #[arg(long)]
        none: bool,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Stop holding a model.
    Drop {
        model: String,
        #[command(flatten)]
        hub: HubArgs,
    },
}

#[derive(Subcommand)]
pub enum ModCmd {
    /// Pending models, oldest first.
    Queue {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Save a model's preview image and its .gmm for inspection.
    Fetch {
        model: String,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[command(flatten)]
        hub: HubArgs,
    },
    Approve {
        model: String,
        #[command(flatten)]
        hub: HubArgs,
    },
    Reject {
        model: String,
        /// copyright, likeness, sexual, hateful or other.
        #[arg(long)]
        code: String,
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Take an active model down: every wearer reverts to the mannequin at once.
    Takedown {
        model: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        reason: String,
        /// The notice this answers.
        #[arg(long, default_value = "")]
        reference: String,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Undo a takedown (a counter-notice).
    Reinstate {
        model: String,
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Let an account upload, or stop it with --revoke.
    Uploads {
        email: String,
        #[arg(long)]
        revoke: bool,
        #[command(flatten)]
        hub: HubArgs,
    },
    /// Set an account's trust tier (2 and above skip the queue).
    Trust {
        email: String,
        tier: u8,
        #[command(flatten)]
        hub: HubArgs,
    },
    ClearStrikes {
        email: String,
        #[command(flatten)]
        hub: HubArgs,
    },
}

#[derive(Subcommand)]
pub enum HubCmd {
    /// Create an account.
    Register {
        #[command(flatten)]
        hub: HubArgs,
    },
    /// The acceptance run's setup (MODELS.md 11): for each .glb in DIR an account, a character
    /// of the model's frame, the upload, a moderator's approval and the model on the character.
    /// `--user` is the moderator.
    SeedAvatars {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 100)]
        count: usize,
        /// Password of the generated accounts `avatar-NNN@bots.test`.
        #[arg(long, default_value = "avatar-password")]
        bot_password: String,
        #[command(flatten)]
        hub: HubArgs,
    },
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

fn refusal(e: HubClientError) -> anyhow::Error {
    match e {
        HubClientError::Refused(HubError::Invalid(text)) => anyhow::anyhow!("refused:\n{text}"),
        HubClientError::Refused(e) => anyhow::anyhow!("refused: {e}"),
        e => e.into(),
    }
}

async fn connect(hub: &HubArgs) -> Result<HubClient> {
    let cert = std::fs::read(&hub.hub_cert)
        .with_context(|| format!("reading {}", hub.hub_cert.display()))?;
    Ok(HubClient::connect_with_cert(hub.hub, cert).await?)
}

/// Log in, waiting when the hub is busy hashing other passwords (HUB.md 3).
async fn session(
    client: &HubClient,
    email: &str,
    password: &str,
    register: bool,
) -> Result<SessionId> {
    let mut register = register;
    for attempt in 0..200u32 {
        let req = if register {
            HubRequest::Register {
                email: email.into(),
                password: password.into(),
            }
        } else {
            HubRequest::Login {
                email: email.into(),
                password: password.into(),
            }
        };
        match client.request(&req).await {
            Ok(HubResponse::Session { session, .. }) => return Ok(session),
            Ok(other) => bail!("unexpected answer {other:?}"),
            Err(HubClientError::Refused(HubError::Taken)) if register => register = false,
            Err(HubClientError::Refused(HubError::Busy)) => {
                tokio::time::sleep(Duration::from_millis(100 + (attempt % 5) as u64 * 50)).await;
            }
            Err(e) => return Err(refusal(e)),
        }
    }
    bail!("the hub stayed busy")
}

async fn login(hub: &HubArgs) -> Result<(HubClient, SessionId)> {
    let client = connect(hub).await?;
    let s = session(&client, &hub.user, &hub.password, false).await?;
    Ok((client, s))
}

fn parse_id(hex: &str) -> Result<ModelId> {
    id_from_hex(hex).context("a model id is 64 hex digits")
}

fn parse_code(code: &str) -> Result<ReasonCode> {
    ReasonCode::from_name(code)
        .filter(|c| *c != ReasonCode::None)
        .context("the code is one of copyright, likeness, sexual, hateful, other")
}

fn print_model(m: &ModelSummary) {
    println!(
        "{}  {:<11} {:<8} {:>4} tris  {}x{}  {} bytes{}",
        id_hex(&m.id),
        rig::frame_from_index(m.frame).map_or("?", rig::frame_name),
        m.status.name(),
        m.triangles,
        m.texture[0],
        m.texture[1],
        m.bytes,
        if m.code == ReasonCode::None {
            String::new()
        } else {
            format!("  [{}: {}]", m.code.name(), m.reason)
        }
    );
}

async fn upload(
    client: &HubClient,
    session: SessionId,
    frame: u8,
    bytes: &[u8],
) -> Result<(ModelId, ModelStatus), HubClientError> {
    let req = HubRequest::ModelUpload {
        session,
        frame,
        tos_version: TOS_VERSION,
        len: bytes.len() as u32,
    };
    // Two uploads are ingested at a time; the third is told to come back (MODELS.md 6.2).
    for attempt in 0..600u32 {
        match client.upload(&req, bytes).await {
            Ok(HubResponse::ModelAccepted { model, status }) => return Ok((model, status)),
            Ok(_) => return Err(HubClientError::Unexpected),
            Err(HubClientError::Refused(HubError::Busy)) if attempt < 599 => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(HubClientError::Unexpected)
}

async fn mod_op(hub: &HubArgs, op: ModOp) -> Result<HubResponse> {
    let (client, session) = login(hub).await?;
    client
        .request(&HubRequest::Mod { session, op })
        .await
        .map_err(refusal)
}

pub fn model(cmd: HubModelCmd) -> Result<()> {
    runtime()?.block_on(async {
        match cmd {
            HubModelCmd::Upload {
                file,
                frame,
                certify,
                hub,
            } => {
                let frame = crate::model::parse_frame(&frame)?;
                if !certify {
                    println!("The upload terms (version {TOS_VERSION}):\n\n  {TERMS}\n\nRepeat the command with --certify to certify them.");
                    std::process::exit(2);
                }
                let bytes =
                    std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
                let (client, session) = login(&hub).await?;
                let (id, status) = upload(&client, session, rig::frame_index(frame), &bytes)
                    .await
                    .map_err(refusal)?;
                println!("{} is {}", id_hex(&id), status.name());
                if status == ModelStatus::Pending {
                    println!("a moderator will look at it; `gm-tools model list` shows the decision");
                }
            }
            HubModelCmd::List { hub } => {
                let (client, session) = login(&hub).await?;
                let HubResponse::Models(models) = client
                    .request(&HubRequest::ModelList { session })
                    .await
                    .map_err(refusal)?
                else {
                    bail!("unexpected answer");
                };
                for m in &models {
                    print_model(m);
                }
                println!("{} models", models.len());
            }
            HubModelCmd::Wear {
                character,
                model,
                none,
                hub,
            } => {
                let model = match (model, none) {
                    (Some(hex), false) => Some(parse_id(&hex)?),
                    (None, true) => None,
                    _ => bail!("give --model ID or --none"),
                };
                let (client, session) = login(&hub).await?;
                let c = find_character(&client, session, &character).await?;
                client
                    .ok(&HubRequest::SetModel {
                        session,
                        character: c.id,
                        model,
                    })
                    .await
                    .map_err(refusal)?;
                println!("{} wears {}", c.name, model.map_or("nothing".into(), |m| id_hex(&m)));
            }
            HubModelCmd::Drop { model, hub } => {
                let model = parse_id(&model)?;
                let (client, session) = login(&hub).await?;
                client
                    .ok(&HubRequest::ModelDrop { session, model })
                    .await
                    .map_err(refusal)?;
                println!("dropped");
            }
        }
        Ok(())
    })
}

async fn find_character(
    client: &HubClient,
    session: SessionId,
    name: &str,
) -> Result<CharacterSummary> {
    let HubResponse::Characters(list) = client
        .request(&HubRequest::Characters { session })
        .await
        .map_err(refusal)?
    else {
        bail!("unexpected answer");
    };
    list.into_iter()
        .find(|c| c.name == name)
        .with_context(|| format!("no character named {name:?} on this account"))
}

pub fn moderate(cmd: ModCmd) -> Result<()> {
    runtime()?.block_on(async {
        match cmd {
            ModCmd::Queue { limit, hub } => {
                let HubResponse::ModQueue(queue) = mod_op(&hub, ModOp::Queue { limit }).await?
                else {
                    bail!("unexpected answer");
                };
                for e in &queue {
                    print_model(&e.model);
                    println!(
                        "    by {}, {} holders, waiting {} min",
                        e.uploader,
                        e.holders,
                        e.waiting_secs / 60
                    );
                }
                println!("{} pending", queue.len());
            }
            ModCmd::Fetch { model, out, hub } => {
                let id = parse_id(&model)?;
                let (client, session) = login(&hub).await?;
                let preview = client
                    .download(
                        &HubRequest::Mod {
                            session,
                            op: ModOp::Preview { model: id },
                        },
                        MAX_MODEL_BYTES as usize,
                    )
                    .await
                    .map_err(refusal)?;
                let gmm = client
                    .download(
                        &HubRequest::ModelGet { session, model: id },
                        MAX_MODEL_BYTES as usize,
                    )
                    .await
                    .map_err(refusal)?;
                std::fs::create_dir_all(&out)?;
                let (png, file) = (
                    out.join(format!("{model}.png")),
                    out.join(format!("{model}.gmm")),
                );
                std::fs::write(&png, preview)?;
                std::fs::write(&file, gmm)?;
                println!(
                    "wrote {} and {} (view it with gm-client --avatar)",
                    png.display(),
                    file.display()
                );
            }
            ModCmd::Approve { model, hub } => {
                mod_op(
                    &hub,
                    ModOp::Decide {
                        model: parse_id(&model)?,
                        approve: true,
                        code: ReasonCode::None,
                        reason: String::new(),
                    },
                )
                .await?;
                println!("approved");
            }
            ModCmd::Reject {
                model,
                code,
                reason,
                hub,
            } => {
                mod_op(
                    &hub,
                    ModOp::Decide {
                        model: parse_id(&model)?,
                        approve: false,
                        code: parse_code(&code)?,
                        reason,
                    },
                )
                .await?;
                println!("rejected");
            }
            ModCmd::Takedown {
                model,
                code,
                reason,
                reference,
                hub,
            } => {
                mod_op(
                    &hub,
                    ModOp::Takedown {
                        model: parse_id(&model)?,
                        code: parse_code(&code)?,
                        reason,
                        reference,
                    },
                )
                .await?;
                println!("taken down; every zone has been told");
            }
            ModCmd::Reinstate { model, reason, hub } => {
                mod_op(
                    &hub,
                    ModOp::Reinstate {
                        model: parse_id(&model)?,
                        reason,
                    },
                )
                .await?;
                println!("reinstated");
            }
            ModCmd::Uploads { email, revoke, hub } => {
                mod_op(
                    &hub,
                    ModOp::SetUpload {
                        email,
                        allow: !revoke,
                    },
                )
                .await?;
                println!("done");
            }
            ModCmd::Trust { email, tier, hub } => {
                mod_op(&hub, ModOp::SetTrust { email, tier }).await?;
                println!("done");
            }
            ModCmd::ClearStrikes { email, hub } => {
                mod_op(&hub, ModOp::ClearStrikes { email }).await?;
                println!("done");
            }
        }
        Ok(())
    })
}

/// The preset whose frame a model is for (assets/content/builds.toml).
fn preset_for(frame: u8) -> &'static str {
    match frame {
        0 => "ironclad",
        2 => "frostweaver",
        3 => "shade",
        _ => "blade",
    }
}

/// The frame a generated avatar's file name ends with (`avatar-007-caster.glb`).
fn frame_of(path: &Path) -> Option<u8> {
    let stem = path.file_stem()?.to_str()?;
    rig::frame_by_name(stem.rsplit('-').next()?).map(rig::frame_index)
}

pub fn hub(cmd: HubCmd) -> Result<()> {
    runtime()?.block_on(async {
        match cmd {
            HubCmd::Register { hub } => {
                let client = connect(&hub).await?;
                session(&client, &hub.user, &hub.password, true).await?;
                println!("{} is registered", hub.user);
            }
            HubCmd::SeedAvatars {
                dir,
                count,
                bot_password,
                hub,
            } => {
                let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .with_context(|| format!("reading {}", dir.display()))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "glb"))
                    .collect();
                files.sort();
                if files.len() < count {
                    bail!("{} holds {} .glb files; {count} are needed", dir.display(), files.len());
                }
                let (moderator, mod_session) = login(&hub).await?;
                let started = std::time::Instant::now();
                let (mut uploaded, mut ingested) = (0u64, 0u64);
                for (i, file) in files.iter().take(count).enumerate() {
                    let frame = frame_of(file)
                        .with_context(|| format!("{} does not end in a frame name", file.display()))?;
                    let email = format!("avatar-{i:03}@bots.test");
                    let name = format!("Avatar{i:03}");
                    let client = connect(&hub).await?;
                    let s = session(&client, &email, &bot_password, true).await?;
                    moderator
                        .request(&HubRequest::Mod {
                            session: mod_session,
                            op: ModOp::SetUpload {
                                email: email.clone(),
                                allow: true,
                            },
                        })
                        .await
                        .map_err(refusal)?;
                    let character = match find_character(&client, s, &name).await {
                        Ok(c) => c.id,
                        Err(_) => match client
                            .request(&HubRequest::CreateCharacter {
                                session: s,
                                name: name.clone(),
                                build: BuildChoice::Preset(preset_for(frame).into()),
                            })
                            .await
                            .map_err(refusal)?
                        {
                            HubResponse::Character(c) => c.id,
                            other => bail!("unexpected answer {other:?}"),
                        },
                    };
                    let bytes = std::fs::read(file)?;
                    let (model, status) = upload(&client, s, frame, &bytes)
                        .await
                        .map_err(refusal)
                        .with_context(|| format!("uploading {}", file.display()))?;
                    uploaded += bytes.len() as u64;
                    if status == ModelStatus::Pending {
                        moderator
                            .request(&HubRequest::Mod {
                                session: mod_session,
                                op: ModOp::Decide {
                                    model,
                                    approve: true,
                                    code: ReasonCode::None,
                                    reason: String::new(),
                                },
                            })
                            .await
                            .map_err(refusal)?;
                    }
                    client
                        .ok(&HubRequest::SetModel {
                            session: s,
                            character,
                            model: Some(model),
                        })
                        .await
                        .map_err(refusal)?;
                    let gmm = client
                        .download(&HubRequest::ModelGet { session: s, model }, MAX_MODEL_BYTES as usize)
                        .await
                        .map_err(refusal)?;
                    ingested += gmm.len() as u64;
                    let _ = client.ok(&HubRequest::Logout { session: s }).await;
                    client.close();
                }
                println!(
                    "seeded {count} avatars in {:.1} s: {:.1} MB uploaded, {:.1} MB ingested ({:.0} KB each)",
                    started.elapsed().as_secs_f64(),
                    uploaded as f64 / 1e6,
                    ingested as f64 / 1e6,
                    ingested as f64 / 1e3 / count.max(1) as f64
                );
            }
        }
        Ok(())
    })
}
