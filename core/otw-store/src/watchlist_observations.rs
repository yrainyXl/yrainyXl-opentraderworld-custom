//! Price observations on watchlist items.
//!
//! Persistence only — the API layer in otw-core resolves the frozen price snapshot from
//! the item's current quote at record time. A price observation is a user record ("I noted
//! this price at this moment"), never a copy of the system's market bars; see migration
//! 9000000106 for the full semantics.
//!
//! `recorded_at` keeps the exact moment (with time of day), so a single calendar day can
//! hold several observations — rows are ordered by `recorded_at DESC`. Only `note` is
//! editable afterwards; `price_snapshot` and `recorded_at` stay frozen.
//!
//! Single-user: no owner scoping.

use anyhow::Context;
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Observation {
    pub id: Uuid,
    pub item_id: Uuid,
    #[serde(with = "ts")]
    pub recorded_at: OffsetDateTime,
    pub price_snapshot: f64,
    pub currency: String,
    pub note: String,
}

mod ts {
    use serde::Serializer;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    pub fn serialize<S: Serializer>(t: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }
}

const COLS: &str = "id, item_id, recorded_at, price_snapshot, currency, note";

/// A watchlist item's observations, newest first — the modal timeline order.
pub async fn list_for_item(pool: &PgPool, item_id: Uuid) -> anyhow::Result<Vec<Observation>> {
    let sql = format!(
        "SELECT {COLS} FROM watchlist_price_observations \
         WHERE item_id = $1 ORDER BY recorded_at DESC, created_at DESC"
    );
    Ok(sqlx::query_as::<_, Observation>(sqlx::AssertSqlSafe(sql))
        .bind(item_id)
        .fetch_all(pool)
        .await
        .context("listing item price observations")?)
}

pub async fn get(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Observation>> {
    let sql = format!("SELECT {COLS} FROM watchlist_price_observations WHERE id = $1");
    Ok(sqlx::query_as::<_, Observation>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("fetching price observation")?)
}

/// Record one observation. The price snapshot and moment are decided by the caller
/// (otw-core resolves them from the item's current quote and the server clock), never
/// taken from the client.
pub async fn add(
    pool: &PgPool,
    item_id: Uuid,
    recorded_at: OffsetDateTime,
    price_snapshot: f64,
    currency: &str,
    note: &str,
) -> anyhow::Result<Observation> {
    let sql = format!(
        "INSERT INTO watchlist_price_observations \
            (id, item_id, recorded_at, price_snapshot, currency, note) \
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
    );
    Ok(sqlx::query_as::<_, Observation>(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::new_v4())
        .bind(item_id)
        .bind(recorded_at)
        .bind(price_snapshot)
        .bind(currency)
        .bind(note)
        .fetch_one(pool)
        .await
        .context("creating price observation")?)
}

/// Rewrite the note only. The frozen fields (price_snapshot, recorded_at) are deliberately
/// not updatable — a historical observation must stay factually stable.
pub async fn update_note(pool: &PgPool, id: Uuid, note: &str) -> anyhow::Result<Option<Observation>> {
    let sql = format!(
        "UPDATE watchlist_price_observations SET note = $2, updated_at = now() \
         WHERE id = $1 RETURNING {COLS}"
    );
    Ok(sqlx::query_as::<_, Observation>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(note)
        .fetch_optional(pool)
        .await
        .context("updating price observation note")?)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM watchlist_price_observations WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting price observation")?
        .rows_affected();
    Ok(n > 0)
}
