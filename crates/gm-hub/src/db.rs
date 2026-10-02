//! Postgres through sqlx (HUB.md 4). Every character move is one transaction; the schema's
//! check constraint keeps "exactly one place" true even if a code path is wrong.

use std::time::Duration;

use gm_core::build::Build;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, Postgres, Row, Transaction};

use gm_hub_proto::protocol::{
    AccountId, CharacterId, CharacterState, CharacterSummary, HubError, LocationSummary,
    MAX_CHARACTERS_PER_ACCOUNT, TRANSIT_ABANDON_SECS, ZoneId,
};

#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

/// A character row as the hub reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterRow {
    pub id: CharacterId,
    pub account_id: AccountId,
    pub name: String,
    pub build: Build,
    pub location_kind: String,
    pub location_zone: Option<String>,
    pub transit_to: Option<String>,
    pub transit_age_secs: Option<f64>,
    /// The zone whose map `position` is on; `None` = no saved position.
    pub pos_zone: Option<String>,
    pub position: [f32; 3],
    pub yaw: f32,
    pub viewport: i16,
    pub play_seconds: i32,
    /// The model the character wears (MODELS.md 6.1), whatever its status.
    pub model: Option<Vec<u8>>,
}

impl CharacterRow {
    pub fn summary(&self) -> CharacterSummary {
        CharacterSummary {
            id: self.id,
            name: self.name.clone(),
            build: self.build.clone(),
            location: match (
                self.location_kind.as_str(),
                &self.location_zone,
                &self.transit_to,
            ) {
                ("zone", Some(z), _) => LocationSummary::Zone(z.clone()),
                ("transit", Some(from), Some(to)) => LocationSummary::Transit {
                    from: from.clone(),
                    to: to.clone(),
                },
                _ => LocationSummary::Offline,
            },
            play_seconds: self.play_seconds.max(0) as u32,
            model: self.model.clone().and_then(|m| m.try_into().ok()),
        }
    }

    pub fn state(&self) -> CharacterState {
        CharacterState {
            build: self.build.clone(),
            zone: self.pos_zone.clone(),
            position: self.position,
            yaw: self.yaw,
            viewport: self.viewport.clamp(0, 255) as u8,
            play_seconds: self.play_seconds.max(0) as u32,
        }
    }
}

fn internal(e: sqlx::Error) -> HubError {
    tracing::error!("database: {e}");
    HubError::Internal
}

fn row_to_character(r: &sqlx::postgres::PgRow) -> Result<CharacterRow, HubError> {
    let build_json: serde_json::Value = r.try_get("build").map_err(internal)?;
    let build: Build = serde_json::from_value(build_json).map_err(|e| {
        tracing::error!("stored build does not parse: {e}");
        HubError::Internal
    })?;
    Ok(CharacterRow {
        id: r.try_get("id").map_err(internal)?,
        account_id: r.try_get("account_id").map_err(internal)?,
        name: r.try_get("name").map_err(internal)?,
        build,
        location_kind: r.try_get("location_kind").map_err(internal)?,
        location_zone: r.try_get("location_zone").map_err(internal)?,
        transit_to: r.try_get("transit_to").map_err(internal)?,
        transit_age_secs: r.try_get("transit_age_secs").map_err(internal)?,
        pos_zone: r.try_get("pos_zone").map_err(internal)?,
        position: [
            r.try_get("pos_x").map_err(internal)?,
            r.try_get("pos_y").map_err(internal)?,
            r.try_get("pos_z").map_err(internal)?,
        ],
        yaw: r.try_get("yaw").map_err(internal)?,
        viewport: r.try_get("viewport").map_err(internal)?,
        play_seconds: r.try_get("play_seconds").map_err(internal)?,
        model: r.try_get("model").map_err(internal)?,
    })
}

const CHARACTER_COLUMNS: &str = "id, account_id, name, build, location_kind, location_zone, transit_to, \
    extract(epoch from (now() - transit_since))::float8 as transit_age_secs, \
    pos_zone, pos_x, pos_y, pos_z, yaw, viewport, play_seconds, model";

impl Db {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn connect(url: &str) -> anyhow::Result<Db> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await?;
        Ok(Db { pool })
    }

    /// Embedded migrations, in order; refuses an unknown newer schema.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Drop every row (tests).
    pub async fn wipe(&self) -> anyhow::Result<()> {
        sqlx::query("truncate model_events, model_holders, models, item_moves, coin_ledger, trade_items, trades, listings, buy_orders, stalls, contract_sellers, contracts, guild_members, guilds, hires, hire_listings, item_components, items, holders, characters, accounts, zones_log restart identity cascade")
            .execute(&self.pool)
            .await?;
        // The cascade empties `holders` too; the two singletons come back at zero.
        sqlx::query("insert into holders (kind) values ('source'), ('sink')")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_account(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<AccountId, HubError> {
        let r =
            sqlx::query("insert into accounts (email, password_hash) values ($1, $2) returning id")
                .bind(email)
                .bind(password_hash)
                .fetch_one(&self.pool)
                .await;
        match r {
            Ok(row) => row.try_get("id").map_err(internal),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(HubError::Taken),
            Err(e) => Err(internal(e)),
        }
    }

    /// `(id, password_hash)` for an email.
    pub async fn account_by_email(
        &self,
        email: &str,
    ) -> Result<Option<(AccountId, String)>, HubError> {
        let row = sqlx::query("select id, password_hash from accounts where email = $1")
            .bind(email)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?;
        row.map(|r| {
            Ok((
                r.try_get("id").map_err(internal)?,
                r.try_get("password_hash").map_err(internal)?,
            ))
        })
        .transpose()
    }

    pub async fn characters_of(&self, account: AccountId) -> Result<Vec<CharacterRow>, HubError> {
        let rows = sqlx::query(AssertSqlSafe(format!(
            "select {CHARACTER_COLUMNS} from characters where account_id = $1 order by id"
        )))
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter().map(row_to_character).collect()
    }

    pub async fn character(&self, id: CharacterId) -> Result<Option<CharacterRow>, HubError> {
        let row = sqlx::query(AssertSqlSafe(format!(
            "select {CHARACTER_COLUMNS} from characters where id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        row.as_ref().map(row_to_character).transpose()
    }

    pub async fn create_character(
        &self,
        account: AccountId,
        name: &str,
        build: &Build,
    ) -> Result<CharacterRow, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        // Lock the account so concurrent creations cannot pass the cap together.
        sqlx::query("select id from accounts where id = $1 for update")
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        let count: i64 = sqlx::query("select count(*) from characters where account_id = $1")
            .bind(account)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get(0)
            .map_err(internal)?;
        if count >= MAX_CHARACTERS_PER_ACCOUNT {
            return Err(HubError::Invalid(format!(
                "at most {MAX_CHARACTERS_PER_ACCOUNT} characters per account"
            )));
        }
        let build_json = serde_json::to_value(build).map_err(|_| HubError::Internal)?;
        let inserted = sqlx::query(AssertSqlSafe(format!(
            "insert into characters (account_id, name, build) values ($1, $2, $3) returning {CHARACTER_COLUMNS}"
        )))
        .bind(account)
        .bind(name)
        .bind(build_json)
        .fetch_one(&mut *tx)
        .await;
        let row = match inserted {
            Ok(r) => row_to_character(&r)?,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(HubError::Taken);
            }
            Err(e) => return Err(internal(e)),
        };
        tx.commit().await.map_err(internal)?;
        Ok(row)
    }

    /// Change the build of an offline character (builds in a zone change through the zone).
    pub async fn set_build(
        &self,
        account: AccountId,
        id: CharacterId,
        build: &Build,
    ) -> Result<(), HubError> {
        let build_json = serde_json::to_value(build).map_err(|_| HubError::Internal)?;
        let n = sqlx::query(
            "update characters set build = $1, updated = now() where id = $2 and account_id = $3 and location_kind = 'offline'",
        )
        .bind(build_json)
        .bind(id)
        .bind(account)
        .execute(&self.pool)
        .await
        .map_err(internal)?
        .rows_affected();
        if n == 0 {
            Err(HubError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn lock_character(
        tx: &mut Transaction<'_, Postgres>,
        id: CharacterId,
    ) -> Result<CharacterRow, HubError> {
        let row = sqlx::query(AssertSqlSafe(format!(
            "select {CHARACTER_COLUMNS} from characters where id = $1 for update"
        )))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(HubError::NotFound)?;
        row_to_character(&row)
    }

    /// `Enter` (HUB.md 3): the character must be offline, or in a transit older than the
    /// abandon age (then it is moved straight to the zone at its spawn). Marks the character
    /// in transit to `zone` from `offline`; the zone's `Claim` finishes the move.
    pub async fn begin_enter(
        &self,
        account: AccountId,
        id: CharacterId,
        zone: &ZoneId,
    ) -> Result<CharacterRow, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let row = Self::lock_character(&mut tx, id).await?;
        if row.account_id != account {
            return Err(HubError::NotFound);
        }
        let abandoned = row.location_kind == "transit"
            && row.transit_age_secs.unwrap_or(0.0) > TRANSIT_ABANDON_SECS as f64;
        if row.location_kind != "offline" && !abandoned {
            return Err(HubError::Busy);
        }
        // A fresh entry has no position in the target zone: spawn there.
        sqlx::query(
            "update characters set location_kind = 'transit', location_zone = $2, transit_to = $2, \
             transit_since = now(), updated = now() where id = $1",
        )
        .bind(id)
        .bind(zone)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(row)
    }

    /// `Claim` by `zone`: the character must be in transit to that zone. Returns the row as
    /// it was (the state the zone loads) and whether the position belongs to this zone.
    pub async fn claim(&self, id: CharacterId, zone: &ZoneId) -> Result<CharacterRow, HubError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let row = Self::lock_character(&mut tx, id).await?;
        if row.location_kind != "transit" || row.transit_to.as_deref() != Some(zone.as_str()) {
            return Err(HubError::Busy);
        }
        sqlx::query(
            "update characters set location_kind = 'zone', location_zone = $2, transit_to = null, \
             transit_since = null, updated = now() where id = $1",
        )
        .bind(id)
        .bind(zone)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(row)
    }

    /// `Save` by `zone`: only the zone the character is in may write (HUB.md 3.2). With
    /// `leaving` the character goes offline.
    pub async fn save(
        &self,
        id: CharacterId,
        zone: &ZoneId,
        state: &CharacterState,
        leaving: bool,
    ) -> Result<(), HubError> {
        let build_json = serde_json::to_value(&state.build).map_err(|_| HubError::Internal)?;
        // The zone the character is in may write; so may the origin of a transit that the
        // target never claimed (the ghost timed out). Streams are unordered: a periodic save
        // sent just before a handoff may arrive after it, so only a transit older than 5 s
        // can be reverted (the ghost timeout is 10 s).
        let n = if leaving {
            sqlx::query(
                "update characters set build = $3, pos_zone = $2, pos_x = $4, pos_y = $5, pos_z = $6, yaw = $7, viewport = $8, \
                 play_seconds = $9, location_kind = 'offline', location_zone = null, transit_to = null, \
                 transit_since = null, updated = now() where id = $1 and location_zone = $2 \
                 and (location_kind = 'zone' or (location_kind = 'transit' \
                 and extract(epoch from (now() - transit_since)) > 5))",
            )
        } else {
            sqlx::query(
                "update characters set build = $3, pos_zone = $2, pos_x = $4, pos_y = $5, pos_z = $6, yaw = $7, viewport = $8, \
                 play_seconds = $9, location_kind = 'zone', transit_to = null, transit_since = null, \
                 updated = now() where id = $1 and location_zone = $2 and (location_kind = 'zone' or (location_kind = 'transit' \
                 and extract(epoch from (now() - transit_since)) > 5))",
            )
        }
        .bind(id)
        .bind(zone)
        .bind(build_json)
        .bind(state.position[0])
        .bind(state.position[1])
        .bind(state.position[2])
        .bind(state.yaw)
        .bind(state.viewport as i16)
        .bind(state.play_seconds as i32)
        .execute(&self.pool)
        .await
        .map_err(internal)?
        .rows_affected();
        if n == 0 {
            Err(HubError::NotFound)
        } else {
            Ok(())
        }
    }

    /// `Handoff` by `from`: saves the state and marks the character in transit to `to`.
    pub async fn begin_handoff(
        &self,
        id: CharacterId,
        from: &ZoneId,
        to: &ZoneId,
        state: &CharacterState,
    ) -> Result<(), HubError> {
        let build_json = serde_json::to_value(&state.build).map_err(|_| HubError::Internal)?;
        let n = sqlx::query(
            "update characters set build = $4, pos_zone = $2, pos_x = $5, pos_y = $6, pos_z = $7, yaw = $8, viewport = $9, \
             play_seconds = $10, location_kind = 'transit', location_zone = $2, transit_to = $3, transit_since = now(), \
             updated = now() where id = $1 and location_kind = 'zone' and location_zone = $2",
        )
        .bind(id)
        .bind(from)
        .bind(to)
        .bind(build_json)
        .bind(state.position[0])
        .bind(state.position[1])
        .bind(state.position[2])
        .bind(state.yaw)
        .bind(state.viewport as i16)
        .bind(state.play_seconds as i32)
        .execute(&self.pool)
        .await
        .map_err(internal)?
        .rows_affected();
        if n == 0 {
            Err(HubError::NotFound)
        } else {
            Ok(())
        }
    }

    /// The zone a character is in right now (for `Logout` kicks).
    pub async fn zone_of(&self, id: CharacterId) -> Result<Option<ZoneId>, HubError> {
        let row = sqlx::query(
            "select location_zone from characters where id = $1 and location_kind = 'zone'",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        row.map(|r| r.try_get("location_zone").map_err(internal))
            .transpose()
    }

    /// The account's transits go offline at `Logout`; characters in a zone are saved and taken
    /// offline by that zone when it handles the kick.
    pub async fn offline_not_in_zone(
        &self,
        account: AccountId,
    ) -> Result<Vec<CharacterId>, HubError> {
        let rows = sqlx::query(
            "update characters set location_kind = 'offline', location_zone = null, transit_to = null, \
             transit_since = null, updated = now() where account_id = $1 and location_kind = 'transit' returning id",
        )
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|r| r.try_get("id").map_err(internal))
            .collect()
    }

    /// Everyone in `zone` goes offline (the zone stopped or crashed and reconnected).
    pub async fn offline_zone(&self, zone: &ZoneId) -> Result<u64, HubError> {
        Ok(sqlx::query(
            "update characters set location_kind = 'offline', location_zone = null, transit_to = null, \
             transit_since = null, updated = now() where location_kind = 'zone' and location_zone = $1",
        )
        .bind(zone)
        .execute(&self.pool)
        .await
        .map_err(internal)?
        .rows_affected())
    }

    pub async fn log(&self, zone: &str, event: &str, detail: &str) {
        let _ = sqlx::query("insert into zones_log (zone, event, detail) values ($1, $2, $3)")
            .bind(zone)
            .bind(event)
            .bind(detail)
            .execute(&self.pool)
            .await;
    }
}
