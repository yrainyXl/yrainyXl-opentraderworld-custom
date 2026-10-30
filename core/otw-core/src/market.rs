//! Shared Market Quote refresh gate — the otw-core side of the `market_quotes` cache.
//!
//! Readers that surface *live* prices (Portfolio asset views, Investment Case `/live`) ask
//! [`ensure_identities`] for the instruments they are about to value. It refreshes any
//! identity whose cached quote is missing or older than [`QUOTE_TTL_SECS`] — fetching each
//! instrument exactly once per stale window (dedup by identity + an in-process in-flight
//! set, so three modules asking for china:601118 in the same tick cost one provider call),
//! CoinGecko in one batched request — and writes the result back to the shared cache.
//!
//! This is the boundary that removes "Watchlist refreshed, Portfolio/Research still show
//! the old price": whatever refreshed last (Watchlist loop, Portfolio refresh, or this
//! gate on page entry) lands in the same `market_quotes` row, and every reader prefers
//! the freshest observation.
//!
//! All futures in this module own their data (no borrowed-across-await references), which
//! keeps them trivially `Send` for the axum/tokio contexts that await them.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use futures::stream::{self, StreamExt};
use sqlx::PgPool;
use time::OffsetDateTime;

use otw_store::market_quotes;
use otw_store::portfolios::Asset;

use crate::portfolios::prices;

/// A shared quote older than this is stale: a page that depends on it refreshes on entry.
/// Matches the module's fastest meaningful cadence (Watchlist sub-minute refresh) and
/// stays inside the 5–30 s band the UI/UX wants for "current price".
pub const QUOTE_TTL_SECS: i64 = 30;

fn in_flight() -> &'static Mutex<HashSet<(String, String)>> {
    static SET: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

fn key(provider: &str, provider_id: &str) -> (String, String) {
    (provider.to_string(), provider_id.to_string())
}

fn default_keys(assets: &[Asset]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    assets
        .iter()
        .filter(|a| a.spot_provider.is_none())
        .filter(|a| seen.insert(key(&a.provider, &a.provider_id)))
        .map(|a| key(&a.provider, &a.provider_id))
        .collect()
}

/// Ensure fresh shared quotes for every default-priced asset in a portfolio.
pub async fn ensure_portfolio_assets(pool: &PgPool, assets: &[Asset]) -> anyhow::Result<()> {
    ensure_identities(pool, &default_keys(assets)).await
}

/// Ensure a fresh shared quote for one instrument identity (Investment Case `/live`).
pub async fn ensure_identity(
    pool: &PgPool,
    provider: &str,
    provider_id: &str,
) -> anyhow::Result<()> {
    ensure_identities(pool, &[key(provider, provider_id)]).await
}

/// Refresh shared quotes that are missing or older than the TTL. Never throws on a single
/// provider hiccup (that identity simply keeps its last known value); DB failures surface.
pub async fn ensure_identities(pool: &PgPool, keys: &[(String, String)]) -> anyhow::Result<()> {
    let keys = dedupe(keys);
    if keys.is_empty() {
        return Ok(());
    }
    let rows = market_quotes::get_many(pool, &keys).await?;
    let now = OffsetDateTime::now_utc();

    let mut todo: Vec<(String, String)> = Vec::new();
    for k in &keys {
        let fresh = rows.iter().any(|q| {
            q.provider == k.0 && q.provider_id == k.1 && market_quotes::age_secs(q, now) < QUOTE_TTL_SECS
        });
        if !fresh {
            todo.push(k.clone());
        }
    }
    if todo.is_empty() {
        return Ok(());
    }

    let mut claim: Vec<(String, String)> = Vec::new();
    {
        let mut set = in_flight().lock().expect("market in-flight lock");
        for k in &todo {
            if set.insert(k.clone()) {
                claim.push(k.clone());
            }
        }
    }
    if claim.is_empty() {
        return Ok(());
    }
    let result = refresh_keys(pool, claim).await;
    {
        let mut set = in_flight().lock().expect("market in-flight lock");
        for k in &todo {
            set.remove(k);
        }
    }
    result
}

fn dedupe(keys: &[(String, String)]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    keys.iter()
        .filter(|k| seen.insert((k.0.clone(), k.1.clone())))
        .cloned()
        .collect()
}

/// Fetch fresh prices for the claimed identities (owned) and upsert them into the shared
/// cache. CoinGecko is one batched call; the rest run with a small concurrency cap.
async fn refresh_keys(pool: &PgPool, keys: Vec<(String, String)>) -> anyhow::Result<()> {
    // CoinGecko batch: a single /simple/price call covers every id at once.
    let cg_ids: Vec<String> = keys
        .iter()
        .filter(|(p, _)| p == "coingecko")
        .map(|(_, id)| id.clone())
        .collect();
    if !cg_ids.is_empty() {
        if let Ok(prices) = prices::crypto_prices(&cg_ids).await {
            let now = OffsetDateTime::now_utc();
            for (p, id) in keys.iter().filter(|(p, _)| p == "coingecko") {
                if let Some(usd) = prices.get(id) {
                    market_quotes::upsert(pool, p, id, *usd, "USD", now).await?;
                }
            }
        }
    }

    let singles: Vec<(String, String)> = keys
        .into_iter()
        .filter(|(p, _)| p != "coingecko")
        .collect();
    // Small bounded fan-out (4 at a time): multi-instrument entries refresh in a few
    // seconds instead of N serialized provider requests, staying gentle on free APIs.
    let results: Vec<((String, String), Option<(f64, String)>)> = stream::iter(singles)
        .map(|k| async move {
            let (p, id) = (&k.0, &k.1);
            let quote = fetch_usd(pool, p, id).await.map(|(u, c)| (u, c.to_string()));
            (k, quote)
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let now = OffsetDateTime::now_utc();
    for ((provider, provider_id), quote) in results {
        if let Some((usd, currency)) = quote {
            market_quotes::upsert(pool, &provider, &provider_id, usd, &currency, now).await?;
        }
    }
    Ok(())
}

/// Current price in USD for one identity, using the exact same provider fetchers the
/// Portfolio Tracker refresh uses (China Market connector / Yahoo / exchanges) — no second
/// market integration. Errors and "no price" both yield None.
async fn fetch_usd(pool: &PgPool, provider: &str, provider_id: &str) -> Option<(f64, &'static str)> {
    let usd = match provider {
        "china" => prices::china_price_usd(pool, provider_id).await.ok().flatten(),
        "yahoo" => prices::stock_price_usd(pool, provider_id).await.ok().flatten(),
        "binance" => prices::binance_price(provider_id).await.ok().flatten(),
        "kraken" => prices::kraken_price(provider_id).await.ok().flatten(),
        "coinbase" => prices::coinbase_price(provider_id).await.ok().flatten(),
        _ => None,
    };
    usd.map(|u| {
        let currency = if provider == "china" { "CNY" } else { "USD" };
        (u, currency)
    })
}
