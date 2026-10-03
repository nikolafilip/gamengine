//! Conduct (ANTICHEAT.md 4 to 8): what zones report about how accounts play, the replays
//! they send, players' reports, the reputation ledger, bans, and what a moderator asks.
//! Statistics rank and people decide: nothing here bans by itself.

use std::path::{Path, PathBuf};

use gm_hub_proto::protocol::{
    AccountId, AimRow, AimStats, CASE_KEEP_DAYS, CharacterId, HubError, MAX_OPEN_REPORTS,
    MAX_REPLAY_BYTES, REPLAY_KEEP_DAYS, ReplayRow, ReplaySummary, ReportReason, ReportRow,
    ReputationRow, Standing, Verdict,
};
use gm_replay::aim::{MIN_MOVING_SHOTS, MIN_SHOTS, Rule, wilson_lower};
use sqlx::postgres::PgPool;
use sqlx::{Postgres, Row, Transaction};

fn internal(e: sqlx::Error) -> HubError {
    tracing::error!("database: {e}");
    HubError::Internal
}

fn io(e: std::io::Error) -> HubError {
    tracing::error!("replay store: {e}");
    HubError::Internal
}

/// Reputation weights (ANTICHEAT.md 6).
pub const TEAM_KILL: i32 = -2;
pub const TEAM_KILLS_A_DAY: i64 = 5;
pub const CONTRACT_PAID: i32 = 1;
pub const CONTRACT_ABANDONED: i32 = -3;
pub const REPORT_UPHELD: i32 = -10;
pub const REPORT_HELPFUL: i32 = 1;
pub const REPORT_ABUSIVE: i32 = -3;
pub const MODEL_STRIKE: i32 = -5;
pub const CHEAT_CONFIRMED: i32 = -100;
/// Tier 1 is automatic at this much play and a reputation that is not negative.
pub const ESTABLISHED_PLAY_SECS: i64 = 10 * 3600;
/// The layout of a stored `AimStats` (`aim_weeks.layout`): raised whenever the struct
/// changes, so that an old row is begun again and not misread.
pub const AIM_LAYOUT: i32 = 2;
/// Weeks of aim numbers kept.
pub const AIM_KEEP_WEEKS: i32 = 26;
/// Analysed shots in the window below which an account is not in the aim report.
pub const REPORT_MIN_SHOTS: u32 = 40;
/// Hard shots an account needs before its hard-hit rate is scored, or counted in the
/// population the scores are against.
const MIN_HARD_SHOTS: u32 = 20;
/// A robust z-score at or above this is an outlier; two of them flag.
pub const OUTLIER_Z: f32 = 4.0;

#[derive(Clone)]
pub struct Conduct {
    pool: PgPool,
    dir: PathBuf,
}

/// A character of a banned account that is in a zone now: to be kicked.
pub struct Kicked {
    pub character: CharacterId,
    pub zone: String,
}

/// How many rows the moderators' log holds (tests and audits).
pub async fn mod_log_rows(pool: &PgPool) -> Result<i64, HubError> {
    sqlx::query("select count(*) as n from mod_log")
        .fetch_one(pool)
        .await
        .map_err(internal)?
        .try_get("n")
        .map_err(internal)
}

fn rules_text(rules: &[Rule]) -> String {
    rules.iter().map(|r| r.name()).collect::<Vec<_>>().join(",")
}

/// Median and median absolute deviation.
fn median_mad(values: &mut [f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let median = values[values.len() / 2];
    let mut dev: Vec<f32> = values.iter().map(|v| (v - median).abs()).collect();
    dev.sort_by(|a, b| a.total_cmp(b));
    (median, dev[dev.len() / 2])
}

/// A robust z-score: how many scaled deviations `value` is above the median. A population
/// whose deviation is zero (most rates are zero for most people) gets a floor, so that the
/// score is large and finite rather than infinite.
fn robust_z(value: f32, median: f32, mad: f32) -> f32 {
    (value - median) / (1.4826 * mad).max(0.02)
}

impl Conduct {
    pub fn new(pool: PgPool, dir: &Path) -> std::io::Result<Conduct> {
        std::fs::create_dir_all(dir)?;
        Ok(Conduct {
            pool,
            dir: dir.to_path_buf(),
        })
    }

    async fn account_of_character(&self, character: CharacterId) -> Result<AccountId, HubError> {
        sqlx::query("select account_id from characters where id = $1")
            .bind(character)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?
            .try_get("account_id")
            .map_err(internal)
    }

    async fn account_of_email(&self, email: &str) -> Result<AccountId, HubError> {
        sqlx::query("select id from accounts where email = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?
            .try_get("id")
            .map_err(internal)
    }

    /// One reputation row and the account's sum, in the caller's transaction.
    async fn credit(
        tx: &mut Transaction<'_, Postgres>,
        account: AccountId,
        kind: &str,
        delta: i32,
        reference: &str,
        note: &str,
    ) -> Result<(), HubError> {
        sqlx::query(
            "insert into reputation (account_id, kind, delta, reference, note) values ($1, $2, $3, $4, $5)",
        )
        .bind(account)
        .bind(kind)
        .bind(delta)
        .bind(reference)
        .bind(note)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        sqlx::query("update accounts set reputation = reputation + $2 where id = $1")
            .bind(account)
            .bind(delta)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    async fn log(
        tx: &mut Transaction<'_, Postgres>,
        moderator: AccountId,
        action: &str,
        account: Option<AccountId>,
        reference: &str,
        note: &str,
    ) -> Result<(), HubError> {
        sqlx::query(
            "insert into mod_log (moderator, action, account_id, reference, note) values ($1, $2, $3, $4, $5)",
        )
        .bind(moderator)
        .bind(action)
        .bind(account)
        .bind(reference)
        .bind(note)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        Ok(())
    }

    /// The ban in force on an account: `(until, reason)`.
    pub async fn banned(&self, account: AccountId) -> Result<Option<(u64, String)>, HubError> {
        let row = sqlx::query(
            "select extract(epoch from until)::bigint as until, reason from bans \
             where account_id = $1 and lifted is null and until > now() order by until desc limit 1",
        )
        .bind(account)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        row.map(|r| {
            Ok((
                r.try_get::<i64, _>("until").map_err(internal)?.max(0) as u64,
                r.try_get("reason").map_err(internal)?,
            ))
        })
        .transpose()
    }

    /// Refuse a banned account with the reason and the end.
    pub async fn check(&self, account: AccountId) -> Result<(), HubError> {
        match self.banned(account).await? {
            Some((until, reason)) => Err(HubError::Banned { until, reason }),
            None => Ok(()),
        }
    }

    pub async fn trust_tier(&self, account: AccountId) -> Result<i16, HubError> {
        sqlx::query("select trust_tier from accounts where id = $1")
            .bind(account)
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?
            .try_get("trust_tier")
            .map_err(internal)
    }

    /// Tier 0 becomes tier 1 by itself: enough play, a reputation that is not negative, no
    /// report upheld in the last 30 days. Nothing moves the other way without a moderator.
    pub async fn promote(&self, account: AccountId) -> Result<(), HubError> {
        sqlx::query(
            "update accounts a set trust_tier = 1 where a.id = $1 and a.trust_tier = 0 and a.reputation >= 0 \
             and (select coalesce(sum(play_seconds), 0) from characters c where c.account_id = a.id) >= $2 \
             and not exists (select 1 from reports r where r.target_account = a.id and r.state = 'upheld' \
                             and r.decided > now() - interval '30 days')",
        )
        .bind(account)
        .bind(ESTABLISHED_PLAY_SECS)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(())
    }

    /// A zone's aim numbers for one character since its last report (ANTICHEAT.md 4.3).
    pub async fn aim(
        &self,
        zone: &str,
        nonce: u64,
        character: CharacterId,
        stats: &AimStats,
    ) -> Result<(), HubError> {
        let account = self.account_of_character(character).await?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        // One report of an account at a time: the week's row may not exist yet to be
        // locked, and the day's team kills are counted before they are added to. The
        // account's own row is the lock (as an update of it would take it: inserts that
        // refer to the account do not wait).
        sqlx::query("select 1 from accounts where id = $1 for no key update")
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        let fresh = sqlx::query(
            "insert into aim_reports (zone, nonce) values ($1, $2) on conflict do nothing",
        )
        .bind(zone)
        .bind(nonce as i64)
        .execute(&mut *tx)
        .await
        .map_err(internal)?
        .rows_affected();
        if fresh == 0 {
            // The zone repeated a report whose answer it lost.
            return Ok(());
        }
        let week: i32 = sqlx::query("select floor(extract(epoch from now()) / 604800)::int as w")
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get("w")
            .map_err(internal)?;
        let row = sqlx::query(
            "select stats from aim_weeks where account_id = $1 and week = $2 and layout = $3 for update",
        )
        .bind(account)
        .bind(week)
        .bind(AIM_LAYOUT)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let mut total: AimStats = match row {
            Some(r) => {
                let bytes: Vec<u8> = r.try_get("stats").map_err(internal)?;
                // A row that says it has this layout and does not decode is damage, and
                // is not written over.
                bitcode::decode(&bytes).map_err(|e| {
                    tracing::error!(account, week, "aim week does not decode: {e}");
                    HubError::Internal
                })?
            }
            // No row yet, or one of an older layout: the week begins again.
            None => AimStats::default(),
        };
        total.add(stats);
        sqlx::query(
            "insert into aim_weeks (account_id, week, analysed, layout, stats) values ($1, $2, $3, $4, $5) \
             on conflict (account_id, week) do update set analysed = excluded.analysed, \
               layout = excluded.layout, stats = excluded.stats",
        )
        .bind(account)
        .bind(week)
        .bind(total.analysed as i32)
        .bind(AIM_LAYOUT)
        .bind(bitcode::encode(&total))
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        // A rule broken by this report alone (one session, or five minutes of one) or by the
        // week: the flag is the same row either way, one per rule and week.
        // The detail is the numbers that break it: the session's, or the week's.
        let mut rules: Vec<(Rule, String)> = stats
            .rules()
            .into_iter()
            .map(|r| (r, stats.line()))
            .collect();
        for r in total.rules() {
            if !rules.iter().any(|(have, _)| *have == r) {
                rules.push((r, total.line()));
            }
        }
        for (rule, detail) in rules {
            sqlx::query(
                "insert into flags (account_id, rule, week, detail) values ($1, $2, $3, $4) \
                 on conflict (account_id, rule, week) do nothing",
            )
            .bind(account)
            .bind(rule.name())
            .bind(week)
            .bind(detail)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        }
        // Team kills are reputation, a few a day at most (a bad evening is not a career).
        if stats.team_kills > 0 {
            let today: i64 = sqlx::query(
                "select count(*) as n from reputation where account_id = $1 and kind = 'team_kill' \
                 and at > now() - interval '1 day'",
            )
            .bind(account)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get("n")
            .map_err(internal)?;
            let count = (stats.team_kills as i64).min((TEAM_KILLS_A_DAY - today).max(0));
            for _ in 0..count {
                Self::credit(&mut tx, account, "team_kill", TEAM_KILL, zone, "").await?;
            }
        }
        tx.commit().await.map_err(internal)?;
        self.promote(account).await
    }

    /// Files in the store that no row names and that are older than an hour: an upload
    /// whose row failed, a part that was never finished.
    async fn orphans(&self) -> Result<(), HubError> {
        let named: std::collections::HashSet<String> = sqlx::query("select sha256 from replays")
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?
            .iter()
            .filter_map(|r| r.try_get::<Vec<u8>, _>("sha256").ok())
            .filter_map(|sha| <[u8; 32]>::try_from(sha).ok())
            .map(|sha| format!("{}.gmr", gm_model::id_hex(&sha)))
            .collect();
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return;
            };
            for e in entries.flatten() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > std::time::Duration::from_secs(3600));
                if old && !named.contains(e.file_name().to_string_lossy().as_ref()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        })
        .await
        .map_err(|_| HubError::Internal)
    }

    fn path(&self, sha: &[u8; 32]) -> PathBuf {
        self.dir.join(format!("{}.gmr", gm_model::id_hex(sha)))
    }

    /// Store a replay a zone wrote (ANTICHEAT.md 3.3) and say which id it has.
    pub async fn replay_put(
        &self,
        zone: &str,
        summary: &ReplaySummary,
        bytes: Vec<u8>,
    ) -> Result<i64, HubError> {
        if bytes.is_empty() || bytes.len() > MAX_REPLAY_BYTES as usize {
            return Err(HubError::Invalid("replay size".into()));
        }
        // The hub believes the bytes, not the zone: at least it must be a replay.
        gm_replay::read_header(&bytes).map_err(|e| HubError::Invalid(format!("replay: {e}")))?;
        let sha = gm_model::model_id(&bytes);
        // The same bytes again (the zone lost the answer and sent them twice): the same
        // replay.
        if let Some(row) = sqlx::query("select id from replays where sha256 = $1")
            .bind(&sha[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
        {
            return row.try_get("id").map_err(internal);
        }
        let path = self.path(&sha);
        let len = bytes.len() as i32;
        // Whole or not there: under another name until it is written. A file whose row
        // then fails is an orphan the sweep removes.
        let partial = path.with_extension(format!("part{:x}", rand::random::<u64>()));
        tokio::task::spawn_blocking(move || {
            std::fs::write(&partial, bytes).and_then(|()| std::fs::rename(&partial, path))
        })
        .await
        .map_err(|_| HubError::Internal)?
        .map_err(io)?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let flagged = summary
            .participants
            .iter()
            .any(|(_, s)| !s.rules().is_empty());
        let inserted = sqlx::query(
            "insert into replays (zone, sha256, bytes, started, seconds, reported, kills, damage, keep) \
             values ($1, $2, $3, to_timestamp($4), $5, $6, $7, $8, $9) \
             on conflict (sha256) do nothing returning id",
        )
        .bind(zone)
        .bind(&sha[..])
        .bind(len)
        .bind(summary.started_unix as f64)
        .bind(summary.seconds)
        .bind(summary.reported)
        .bind(summary.kills as i32)
        .bind(summary.damage as i64)
        .bind(summary.reported || flagged)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?;
        let Some(row) = inserted else {
            // Stored by the same upload arriving twice at once.
            drop(tx);
            return sqlx::query("select id from replays where sha256 = $1")
                .bind(&sha[..])
                .fetch_one(&self.pool)
                .await
                .map_err(internal)?
                .try_get("id")
                .map_err(internal);
        };
        let id: i64 = row.try_get("id").map_err(internal)?;
        for (character, stats) in &summary.participants {
            let rules = stats.rules();
            // A character that was deleted meanwhile is simply not listed.
            let done = sqlx::query(
                "insert into replay_participants (replay_id, character_id, account_id, analysed, rules) \
                 select $1, c.id, c.account_id, $3, $4 from characters c where c.id = $2 \
                 on conflict do nothing returning account_id",
            )
            .bind(id)
            .bind(character)
            .bind(stats.analysed as i32)
            .bind(rules_text(&rules))
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
            let Some(row) = done else { continue };
            let account: AccountId = row.try_get("account_id").map_err(internal)?;
            // A fight in which somebody's numbers break a rule is evidence: flag and keep.
            for rule in rules {
                sqlx::query(
                    "insert into flags (account_id, rule, week, detail, replay_id) \
                     values ($1, $2, floor(extract(epoch from now()) / 604800)::int, $3, $4) \
                     on conflict (account_id, rule, week) do update set replay_id = coalesce(flags.replay_id, excluded.replay_id)",
                )
                .bind(account)
                .bind(rule.name())
                .bind(stats.line())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
            }
        }
        for report in &summary.reports {
            sqlx::query("update reports set replay_id = $2 where id = $1 and zone = $3")
                .bind(*report)
                .bind(id)
                .bind(zone)
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
        }
        tx.commit().await.map_err(internal)?;
        Ok(id)
    }

    /// A player's report (ANTICHEAT.md 5): the row first, within the reporter's limits; the
    /// replay follows.
    pub async fn report_open(
        &self,
        zone: &str,
        reporter: CharacterId,
        target: CharacterId,
        reason: ReportReason,
    ) -> Result<i64, HubError> {
        let reporter_account = self.account_of_character(reporter).await?;
        let target_account = self.account_of_character(target).await?;
        if reporter_account == target_account {
            return Err(HubError::Invalid("nobody reports themselves".into()));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let open: i64 = sqlx::query(
            "select count(*) as n from reports where reporter_account = $1 and state = 'open'",
        )
        .bind(reporter_account)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?
        .try_get("n")
        .map_err(internal)?;
        if open >= MAX_OPEN_REPORTS {
            return Err(HubError::Busy);
        }
        let r = sqlx::query(
            "insert into reports (reporter_account, reporter_character, target_account, target_character, zone, reason) \
             values ($1, $2, $3, $4, $5, $6) returning id",
        )
        .bind(reporter_account)
        .bind(reporter)
        .bind(target_account)
        .bind(target)
        .bind(zone)
        .bind(reason.name())
        .fetch_one(&mut *tx)
        .await;
        let id: i64 = match r {
            Ok(row) => row.try_get("id").map_err(internal)?,
            // Already an open report of this reporter on this target.
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(HubError::Taken);
            }
            Err(e) => return Err(internal(e)),
        };
        tx.commit().await.map_err(internal)?;
        Ok(id)
    }

    /// A moderator's word on a report. Only an open report can be decided, once.
    pub async fn report_verdict(
        &self,
        moderator: AccountId,
        id: i64,
        verdict: Verdict,
        note: &str,
    ) -> Result<(), HubError> {
        let state = match verdict {
            Verdict::Upheld => "upheld",
            Verdict::NotProven => "not_proven",
            Verdict::Abusive => "abusive",
        };
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let row = sqlx::query(
            "update reports set state = $2, note = $3, decided_by = $4, decided = now() \
             where id = $1 and state = 'open' returning reporter_account, target_account",
        )
        .bind(id)
        .bind(state)
        .bind(note)
        .bind(moderator)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or(HubError::NotFound)?;
        let reporter: AccountId = row.try_get("reporter_account").map_err(internal)?;
        let target: AccountId = row.try_get("target_account").map_err(internal)?;
        // A moderator does not judge a case it is part of.
        if moderator == reporter || moderator == target {
            return Err(HubError::Unauthorized);
        }
        let reference = format!("report {id}");
        match verdict {
            Verdict::Upheld => {
                Self::credit(
                    &mut tx,
                    target,
                    "report_upheld",
                    REPORT_UPHELD,
                    &reference,
                    note,
                )
                .await?;
                Self::credit(
                    &mut tx,
                    reporter,
                    "report_helpful",
                    REPORT_HELPFUL,
                    &reference,
                    "",
                )
                .await?;
            }
            Verdict::NotProven => {}
            Verdict::Abusive => {
                Self::credit(
                    &mut tx,
                    reporter,
                    "report_abusive",
                    REPORT_ABUSIVE,
                    &reference,
                    note,
                )
                .await?;
            }
        }
        Self::log(
            &mut tx,
            moderator,
            "report_verdict",
            Some(target),
            &reference,
            state,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    /// Ban an account; returns its characters that are in zones now.
    pub async fn ban(
        &self,
        moderator: AccountId,
        email: &str,
        days: u32,
        reason: &str,
        cheat: bool,
    ) -> Result<(AccountId, Vec<Kicked>), HubError> {
        if reason.trim().is_empty() {
            return Err(HubError::Invalid("a ban names its reason".into()));
        }
        let account = self.account_of_email(email).await?;
        if account == moderator {
            return Err(HubError::Invalid("nobody bans themselves".into()));
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let ban: i64 = sqlx::query(
            "insert into bans (account_id, until, reason, by_account) \
             values ($1, now() + make_interval(days => $2), $3, $4) returning id",
        )
        .bind(account)
        .bind(days.clamp(1, 36500) as i32)
        .bind(reason)
        .bind(moderator)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?
        .try_get("id")
        .map_err(internal)?;
        let reference = format!("ban {ban}");
        if cheat {
            Self::credit(
                &mut tx,
                account,
                "cheat_confirmed",
                CHEAT_CONFIRMED,
                &reference,
                reason,
            )
            .await?;
        }
        Self::log(&mut tx, moderator, "ban", Some(account), &reference, reason).await?;
        let rows = sqlx::query(
            "select id, location_zone from characters where account_id = $1 and location_kind <> 'offline'",
        )
        .bind(account)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        // Those on their way somewhere are nowhere from now on: a ticket in hand opens
        // no door (the claim asks again), and its character is not left between two.
        sqlx::query(
            "update characters set location_kind = 'offline', location_zone = null, transit_to = null, \
             transit_since = null, updated = now() where account_id = $1 and location_kind = 'transit'",
        )
        .bind(account)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        let mut kicked = Vec::new();
        for r in rows {
            if let Some(zone) = r
                .try_get::<Option<String>, _>("location_zone")
                .map_err(internal)?
            {
                kicked.push(Kicked {
                    character: r.try_get("id").map_err(internal)?,
                    zone,
                });
            }
        }
        Ok((account, kicked))
    }

    /// Lift every ban in force on the account: the rows stay, marked.
    pub async fn unban(
        &self,
        moderator: AccountId,
        email: &str,
        note: &str,
    ) -> Result<(), HubError> {
        let account = self.account_of_email(email).await?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let lifted = sqlx::query(
            "update bans set lifted = now(), lifted_by = $2 where account_id = $1 and lifted is null and until > now()",
        )
        .bind(account)
        .bind(moderator)
        .execute(&mut *tx)
        .await
        .map_err(internal)?
        .rows_affected();
        if lifted == 0 {
            return Err(HubError::NotFound);
        }
        Self::log(&mut tx, moderator, "unban", Some(account), "", note).await?;
        tx.commit().await.map_err(internal)
    }

    /// A reputation row by hand.
    pub async fn adjust(
        &self,
        moderator: AccountId,
        email: &str,
        delta: i32,
        note: &str,
    ) -> Result<(), HubError> {
        if note.trim().is_empty() {
            return Err(HubError::Invalid("an adjustment names its reason".into()));
        }
        let account = self.account_of_email(email).await?;
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::credit(
            &mut tx,
            account,
            "moderator",
            delta,
            &format!("by {moderator}"),
            note,
        )
        .await?;
        Self::log(
            &mut tx,
            moderator,
            "adjust",
            Some(account),
            &delta.to_string(),
            note,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    /// A contract ended (ECONOMY.md 8): paid, or abandoned by its sellers.
    pub async fn contract_decided(&self, contract: i64, paid: bool) -> Result<(), HubError> {
        let buyer: CharacterId = sqlx::query("select buyer_character from contracts where id = $1")
            .bind(contract)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?
            .try_get("buyer_character")
            .map_err(internal)?;
        let sellers: Vec<CharacterId> =
            sqlx::query("select character_id from contract_sellers where contract_id = $1")
                .bind(contract)
                .fetch_all(&self.pool)
                .await
                .map_err(internal)?
                .iter()
                .filter_map(|r| r.try_get("character_id").ok())
                .collect();
        if paid {
            self.contract_paid(contract, buyer, &sellers).await
        } else {
            self.contract_abandoned(contract, &sellers).await
        }
    }

    /// A contract was paid (ECONOMY.md 8): each seller's reputation, once per buyer account
    /// a week, so that alts buying one-copper carries bank nothing.
    pub async fn contract_paid(
        &self,
        contract: i64,
        buyer: CharacterId,
        sellers: &[CharacterId],
    ) -> Result<(), HubError> {
        let buyer_account = self.account_of_character(buyer).await?;
        let reference = format!("buyer {buyer_account}");
        let mut tx = self.pool.begin().await.map_err(internal)?;
        for seller in sellers {
            let account = self.account_of_character(*seller).await?;
            if account == buyer_account {
                continue;
            }
            let seen: i64 = sqlx::query(
                "select count(*) as n from reputation where account_id = $1 and kind = 'contract_paid' \
                 and reference = $2 and at > now() - interval '7 days'",
            )
            .bind(account)
            .bind(&reference)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get("n")
            .map_err(internal)?;
            if seen == 0 {
                Self::credit(
                    &mut tx,
                    account,
                    "contract_paid",
                    CONTRACT_PAID,
                    &reference,
                    &format!("contract {contract}"),
                )
                .await?;
            }
        }
        tx.commit().await.map_err(internal)
    }

    /// The sellers abandoned a contract.
    pub async fn contract_abandoned(
        &self,
        contract: i64,
        sellers: &[CharacterId],
    ) -> Result<(), HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        for seller in sellers {
            let account = self.account_of_character(*seller).await?;
            Self::credit(
                &mut tx,
                account,
                "contract_abandoned",
                CONTRACT_ABANDONED,
                &format!("contract {contract}"),
                "",
            )
            .await?;
        }
        tx.commit().await.map_err(internal)
    }

    /// The aim report (ANTICHEAT.md 4.4): accounts with enough shots in the last `weeks`,
    /// most suspicious first. Looking is logged.
    pub async fn aim_report(
        &self,
        moderator: AccountId,
        weeks: u8,
        min_shots: u32,
    ) -> Result<Vec<AimRow>, HubError> {
        let rows = sqlx::query(
            "select w.account_id, a.email, w.stats from aim_weeks w join accounts a on a.id = w.account_id \
             where w.week > floor(extract(epoch from now()) / 604800)::int - $1 and w.layout = $2 \
             order by w.account_id",
        )
        .bind(weeks.max(1) as i32)
        .bind(AIM_LAYOUT)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut accounts: Vec<(AccountId, String, AimStats)> = Vec::new();
        for r in rows {
            let account: AccountId = r.try_get("account_id").map_err(internal)?;
            let bytes: Vec<u8> = r.try_get("stats").map_err(internal)?;
            // A week of another layout, or a damaged one, is left out of the report.
            let Ok(stats) = bitcode::decode::<AimStats>(&bytes) else {
                continue;
            };
            match accounts.last_mut() {
                Some(last) if last.0 == account => last.2.add(&stats),
                _ => accounts.push((account, r.try_get("email").map_err(internal)?, stats)),
            }
        }
        accounts.retain(|a| a.2.analysed >= min_shots.max(1));
        // The population's middle and spread, per signal, over the accounts whose sample
        // carries that signal (the same minimum a score needs below): an account with no
        // hard shots has no hard-hit rate, and a population of such zeros would put the
        // median at nothing and give everyone who took a hard shot a score.
        let spread = |carries: &dyn Fn(&AimStats) -> bool, f: &dyn Fn(&AimStats) -> f32| {
            let mut values: Vec<f32> = accounts
                .iter()
                .filter(|a| carries(&a.2))
                .map(|a| f(&a.2))
                .collect();
            median_mad(&mut values)
        };
        // A rate is believed as far as its sample carries it (the Wilson lower bound):
        // one lock in three shots is not a rate of a third.
        let lock_rate = |s: &AimStats| wilson_lower(s.locks, s.moving_shots);
        let flick_rate = |s: &AimStats| wilson_lower(s.flicks, s.analysed);
        let hard = spread(&|s| s.hard_shots >= MIN_HARD_SHOTS, &|s| s.hard_hit_rate());
        let lock = spread(&|s| s.moving_shots >= MIN_MOVING_SHOTS, &lock_rate);
        let flick = spread(&|s| s.analysed >= MIN_SHOTS, &flick_rate);
        let mut out = Vec::with_capacity(accounts.len());
        for (account, email, stats) in accounts {
            let z_hard_hits = if stats.hard_shots >= MIN_HARD_SHOTS {
                robust_z(stats.hard_hit_rate(), hard.0, hard.1)
            } else {
                0.0
            };
            let z_lock = if stats.moving_shots >= MIN_MOVING_SHOTS {
                robust_z(lock_rate(&stats), lock.0, lock.1)
            } else {
                0.0
            };
            let z_flick = if stats.analysed >= MIN_SHOTS {
                robust_z(flick_rate(&stats), flick.0, flick.1)
            } else {
                0.0
            };
            let mut rules: Vec<String> =
                stats.rules().iter().map(|r| r.name().to_string()).collect();
            if [z_hard_hits, z_lock, z_flick]
                .iter()
                .filter(|z| **z >= OUTLIER_Z)
                .count()
                >= 2
            {
                rules.push(Rule::Outlier.name().to_string());
            }
            let replays = sqlx::query(
                "select replay_id from replay_participants where account_id = $1 order by replay_id desc limit 5",
            )
            .bind(account)
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?
            .iter()
            .filter_map(|r| r.try_get("replay_id").ok())
            .collect();
            out.push(AimRow {
                account,
                email,
                stats,
                rules,
                z_hard_hits,
                z_lock,
                z_flick,
                replays,
            });
        }
        // Rule breakers first, then by how far out they stand.
        out.sort_by(|a, b| {
            let key = |r: &AimRow| (r.rules.len(), r.z_lock.max(r.z_flick).max(r.z_hard_hits));
            let (ka, kb) = (key(a), key(b));
            kb.0.cmp(&ka.0).then(kb.1.total_cmp(&ka.1))
        });
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::log(
            &mut tx,
            moderator,
            "aim_report",
            None,
            &format!("{weeks} weeks"),
            "",
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        Ok(out)
    }

    /// Replays, newest first.
    pub async fn replays(
        &self,
        moderator: AccountId,
        email: Option<&str>,
        reported: bool,
        flagged: bool,
        limit: u32,
    ) -> Result<Vec<ReplayRow>, HubError> {
        let account = match email {
            Some(e) => Some(self.account_of_email(e).await?),
            None => None,
        };
        let rows = sqlx::query(
            "select r.id, r.zone, extract(epoch from r.started)::bigint as started, r.seconds, r.reported, \
                    r.kills, r.damage, r.bytes from replays r \
             where ($1::bigint is null or exists (select 1 from replay_participants p where p.replay_id = r.id and p.account_id = $1)) \
               and (not $2 or r.reported) \
               and (not $3 or exists (select 1 from replay_participants p where p.replay_id = r.id and p.rules <> '')) \
             order by r.id desc limit $4",
        )
        .bind(account)
        .bind(reported)
        .bind(flagged)
        .bind(limit.clamp(1, 200) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let id: i64 = r.try_get("id").map_err(internal)?;
            let participants = sqlx::query(
                "select c.name, p.rules from replay_participants p join characters c on c.id = p.character_id \
                 where p.replay_id = $1 order by c.name",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?
            .iter()
            .filter_map(|p| Some((p.try_get("name").ok()?, p.try_get("rules").ok()?)))
            .collect();
            out.push(ReplayRow {
                id,
                zone: r.try_get("zone").map_err(internal)?,
                started_unix: r.try_get::<i64, _>("started").map_err(internal)?.max(0) as u64,
                seconds: r.try_get("seconds").map_err(internal)?,
                reported: r.try_get("reported").map_err(internal)?,
                kills: r.try_get::<i32, _>("kills").map_err(internal)?.max(0) as u32,
                damage: r.try_get::<i64, _>("damage").map_err(internal)?.max(0) as u64,
                bytes: r.try_get::<i32, _>("bytes").map_err(internal)?.max(0) as u32,
                participants,
            });
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::log(&mut tx, moderator, "replays", account, "", "").await?;
        tx.commit().await.map_err(internal)?;
        Ok(out)
    }

    /// The bytes of a replay, verified against their name. Fetching one is logged: it is
    /// other people's play.
    pub async fn replay_get(&self, moderator: AccountId, id: i64) -> Result<Vec<u8>, HubError> {
        let sha: Vec<u8> = sqlx::query("select sha256 from replays where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?
            .try_get("sha256")
            .map_err(internal)?;
        let sha: [u8; 32] = sha.try_into().map_err(|_| HubError::Internal)?;
        let path = self.path(&sha);
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(|_| HubError::Internal)?
            .map_err(|_| HubError::Gone)?;
        if gm_model::model_id(&bytes) != sha {
            tracing::error!(id, "a stored replay does not match its hash");
            return Err(HubError::Internal);
        }
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::log(
            &mut tx,
            moderator,
            "replay_get",
            None,
            &format!("replay {id}"),
            "",
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        Ok(bytes)
    }

    pub async fn reports(
        &self,
        moderator: AccountId,
        open_only: bool,
    ) -> Result<Vec<ReportRow>, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::log(&mut tx, moderator, "reports", None, "", "").await?;
        tx.commit().await.map_err(internal)?;
        let rows = sqlx::query(
            "select r.id, ra.email as reporter, ta.email as target, r.zone, r.reason, r.state, r.replay_id, \
                    extract(epoch from r.created)::bigint as created, r.note \
             from reports r join accounts ra on ra.id = r.reporter_account join accounts ta on ta.id = r.target_account \
             where (not $1 or r.state = 'open') order by r.id desc limit 200",
        )
        .bind(open_only)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|r| {
                Ok(ReportRow {
                    id: r.try_get("id").map_err(internal)?,
                    reporter: r.try_get("reporter").map_err(internal)?,
                    target: r.try_get("target").map_err(internal)?,
                    zone: r.try_get("zone").map_err(internal)?,
                    reason: r.try_get("reason").map_err(internal)?,
                    state: r.try_get("state").map_err(internal)?,
                    replay: r.try_get("replay_id").map_err(internal)?,
                    created_unix: r.try_get::<i64, _>("created").map_err(internal)?.max(0) as u64,
                    note: r.try_get("note").map_err(internal)?,
                })
            })
            .collect()
    }

    /// An account as a moderator sees it.
    pub async fn standing(&self, moderator: AccountId, email: &str) -> Result<Standing, HubError> {
        let account = self.account_of_email(email).await?;
        let a = sqlx::query("select reputation, trust_tier from accounts where id = $1")
            .bind(account)
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?;
        let flags = sqlx::query(
            "select rule, week, detail from flags where account_id = $1 and closed is null order by id desc limit 50",
        )
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?
        .iter()
        .filter_map(|f| {
            Some((
                f.try_get("rule").ok()?,
                f.try_get::<i32, _>("week").ok()?.to_string(),
                f.try_get("detail").ok()?,
            ))
        })
        .collect();
        let ledger = sqlx::query(
            "select kind, delta, reference, note, extract(epoch from at)::bigint as at from reputation \
             where account_id = $1 order by id desc limit 100",
        )
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?
        .iter()
        .filter_map(|r| {
            Some(ReputationRow {
                kind: r.try_get("kind").ok()?,
                delta: r.try_get("delta").ok()?,
                reference: r.try_get("reference").ok()?,
                note: r.try_get("note").ok()?,
                at_unix: r.try_get::<i64, _>("at").ok()?.max(0) as u64,
            })
        })
        .collect();
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::log(&mut tx, moderator, "standing", Some(account), "", "").await?;
        tx.commit().await.map_err(internal)?;
        Ok(Standing {
            reputation: a.try_get("reputation").map_err(internal)?,
            trust_tier: a.try_get("trust_tier").map_err(internal)?,
            ban: self.banned(account).await?,
            flags,
            ledger,
        })
    }

    /// Retention (ANTICHEAT.md 3.3): replays nobody points to go after `REPLAY_KEEP_DAYS`;
    /// kept ones `CASE_KEEP_DAYS` after their last open case closed; aim weeks after
    /// `AIM_KEEP_WEEKS`; nonces after a day. Returns the replays deleted.
    pub async fn sweep(&self) -> Result<u64, HubError> {
        // A flag nobody acted on closes by itself: its replay is evidence for so long.
        sqlx::query(
            "update flags set closed = now() where closed is null and at < now() - make_interval(days => $1)",
        )
        .bind(CASE_KEEP_DAYS)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        let gone = sqlx::query(
            "delete from replays r where \
               (not r.keep and r.stored < now() - make_interval(days => $1)) \
               or (r.keep and r.stored < now() - make_interval(days => $2) \
                   and not exists (select 1 from reports p where p.replay_id = r.id and p.state = 'open') \
                   and not exists (select 1 from flags f where f.replay_id = r.id and f.closed is null)) \
             returning sha256",
        )
        .bind(REPLAY_KEEP_DAYS)
        .bind(REPLAY_KEEP_DAYS + CASE_KEEP_DAYS)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        for r in &gone {
            let sha: Vec<u8> = r.try_get("sha256").map_err(internal)?;
            if let Ok(sha) = <[u8; 32]>::try_from(sha) {
                // Another row may name the same bytes (the same file sent twice).
                let still: i64 = sqlx::query("select count(*) as n from replays where sha256 = $1")
                    .bind(&sha[..])
                    .fetch_one(&self.pool)
                    .await
                    .map_err(internal)?
                    .try_get("n")
                    .map_err(internal)?;
                if still == 0 {
                    let _ = std::fs::remove_file(self.path(&sha));
                }
            }
        }
        sqlx::query(
            "delete from aim_weeks where week < floor(extract(epoch from now()) / 604800)::int - $1",
        )
        .bind(AIM_KEEP_WEEKS)
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        sqlx::query("delete from aim_reports where at < now() - interval '1 day'")
            .execute(&self.pool)
            .await
            .map_err(internal)?;
        self.orphans().await?;
        Ok(gone.len() as u64)
    }
}
