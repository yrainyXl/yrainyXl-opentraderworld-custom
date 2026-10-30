//! Shared Market Quote cache — one "current market price" per instrument identity.
//!
//! This is the single shared current-quote boundary the Watchlist, Portfolio Tracker and
//! Research consume (migration 9000000107). It is deliberately *not* a trade/position
//! fact and *not* a history store: one row per `(provider, provider_id)` with the latest
//! price in USD and when it was observed. Writers upsert after their own existing fetch
//! (no extra provider calls); readers prefer the fresher of this row vs the module-local
//! last price (see the migration header for the full rationale).
//!
//! Single-user: no owner scoping, like the rest of the app.

use anyhow::Context;
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MarketQuote {
    pub provider: String,
    pub provider_id: String,
    /// Current price in USD (converted from the native quote currency at write time).
    pub price_usd: f64,
    /// Native quote currency the USD price was converted from.
    pub currency: String,
    #[serde(with = "ts")]
    pub quote_at: OffsetDateTime,
}

mod ts {
    use serde::Serializer;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    pub fn serialize<S: Serializer>(t: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }
}

const COLS: &str = "provider, provider_id, price_usd, currency, quote_at";

/// Store/refresh the current quote for one instrument identity.
pub async fn upsert(
    pool: &PgPool,
    provider: &str,
    provider_id: &str,
    price_usd: f64,
    currency: &str,
    quote_at: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO market_quotes (provider, provider_id, price_usd, currency, quote_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (provider, provider_id) \
         DO UPDATE SET price_usd = EXCLUDED.price_usd, currency = EXCLUDED.currency, \
                       quote_at = EXCLUDED.quote_at, updated_at = now()",
    )
    .bind(provider)
    .bind(provider_id)
    .bind(price_usd)
    .bind(currency)
    .bind(quote_at)
    .execute(pool)
    .await
    .context("upserting market quote")?;
    Ok(())
}

/// One quote row, if the identity has one.
pub async fn get(
    pool: &PgPool,
    provider: &str,
    provider_id: &str,
) -> anyhow::Result<Option<MarketQuote>> {
    let sql = format!("SELECT {COLS} FROM market_quotes WHERE provider = $1 AND provider_id = $2");
    Ok(sqlx::query_as::<_, MarketQuote>(sqlx::AssertSqlSafe(sql))
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .context("fetching market quote")?)
}

/// Quotes for many identities in one round-trip. Callers dedupe keys beforehand; the
/// query tolerates duplicates in the input anyway.
pub async fn get_many(
    pool: &PgPool,
    keys: &[(String, String)],
) -> anyhow::Result<Vec<MarketQuote>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let providers: Vec<&str> = keys.iter().map(|(p, _)| p.as_str()).collect();
    let ids: Vec<&str> = keys.iter().map(|(_, i)| i.as_str()).collect();
    let sql = format!(
        "SELECT {COLS} FROM market_quotes \
         WHERE (provider, provider_id) IN \
               (SELECT * FROM UNNEST($1::text[], $2::text[]))"
    );
    Ok(sqlx::query_as::<_, MarketQuote>(sqlx::AssertSqlSafe(sql))
        .bind(&providers)
        .bind(&ids)
        .fetch_all(pool)
        .await
        .context("fetching market quotes")?)
}

/// Delete an identity's quote (not used by live flows; available for cleanup/testing).
pub async fn remove(pool: &PgPool, provider: &str, provider_id: &str) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM market_quotes WHERE provider = $1 AND provider_id = $2")
        .bind(provider)
        .bind(provider_id)
        .execute(pool)
        .await
        .context("deleting market quote")?
        .rows_affected();
    Ok(n > 0)
}

/// Convert a quote age check into a simple boolean the refresh gate can use.
pub fn age_secs(q: &MarketQuote, now: OffsetDateTime) -> i64 {
    (now - q.quote_at).whole_seconds().max(0)
}
