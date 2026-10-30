//! Targeted integration tests for the shared Market Quote layer (migration 9000000107).
//!
//! These hit a real Postgres when `DATABASE_URL` is set (see the sibling
//! `price_observations.rs` for the same convention): a fresh, fully-migrated pool per
//! test, run serially. Without `DATABASE_URL` they print a note and pass vacuously.
//!
//! Coverage:
//!   1. a shared quote fresher than the asset's last price drives Portfolio asset views
//!   2. a stale/older shared quote keeps the persisted last price (fallback)
//!   3. Research `/live` (instrument_position) values every portfolio slice from the SAME
//!      fresh shared quote (multiple portfolios, one current price)
//!   4. the shared quote never changes transaction facts (quantity/basis/realized)
//!   5. one identity keeps one cache row (upsert = replace, duplicates in input tolerated)
//!   6. no shared row → the newest asset last price is the fallback (old semantics)
//!   7. updating the current quote never rewrites a previously recorded snapshot

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use otw_store::investment_cases::{self, InvestmentCase};
use otw_store::market_quotes;
use otw_store::portfolios as store;
use otw_store::connect_and_migrate;
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            eprintln!("DATABASE_URL not set — skipping shared market-quote DB tests");
            return None;
        }
    };
    Some(connect_and_migrate(&url).await.expect("test database"))
}

/// Unique ticker per test — the shared DB runs all tests concurrently, so tests must
/// never collide on the same (provider, provider_id) identity.
fn rnd_ticker() -> String {
    format!("TST{:08x}", Uuid::new_v4().as_u128() as u32)
}

/// One USD portfolio + one USD stock asset holding `qty` @ `cost` (single buy, no fees).
async fn holding_named(pool: &PgPool, qty: f64, cost: f64, ticker: &str) -> (store::Portfolio, store::Asset) {
    let name = format!("mq-{}", Uuid::new_v4());
    let pf = store::create_portfolio(pool, &name, "", "USD").await.unwrap();
    let asset = store::add_asset(pool, pf.id, "stock", "yahoo", ticker, ticker, "Apple", "USD")
        .await
        .unwrap();
    let day = (OffsetDateTime::now_utc() - Duration::days(30)).date();
    store::add_operation(pool, asset.id, "buy", day, qty, cost, 0.0, "")
        .await
        .unwrap();
    (pf, asset)
}

fn ago_secs(s: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() - Duration::seconds(s)
}

#[tokio::test]
async fn fresh_shared_quote_prices_the_asset_view() {
    let Some(pool) = pool().await else { return };
    let t = rnd_ticker();
    let (pf, asset) = holding_named(&pool, 100.0, 6.0, &t).await;
    store::set_asset_price(&pool, asset.id, 6.5).await.unwrap(); // persisted last price
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Newer shared quote (as if the Watchlist refreshed after the Portfolio did).
    market_quotes::upsert(&pool, "yahoo", &t, 7.92, "USD", OffsetDateTime::now_utc())
        .await
        .unwrap();

    let views = store::asset_views(&pool, &pf).await.unwrap();
    assert_eq!(views.len(), 1);
    let v = &views[0];
    assert!((v.price.unwrap() - 7.92).abs() < 1e-9, "price should follow shared quote");
    assert!((v.market_value.unwrap() - 792.0).abs() < 1e-9);
    // cost basis = 100 * 6 = 600 → unrealized 192
    assert!((v.unrealized.unwrap() - 192.0).abs() < 1e-9);
    // transaction facts unchanged
    assert!((v.quantity - 100.0).abs() < 1e-9);
    assert!((v.cost_basis - 600.0).abs() < 1e-9);
    assert!((v.avg_cost.unwrap() - 6.0).abs() < 1e-9);
    assert!((v.realized).abs() < 1e-9);
}

#[tokio::test]
async fn stale_shared_quote_keeps_last_price_and_fallback() {
    let Some(pool) = pool().await else { return };
    let t = rnd_ticker();
    let (pf, asset) = holding_named(&pool, 10.0, 5.0, &t).await;
    store::set_asset_price(&pool, asset.id, 6.0).await.unwrap();

    // Shared quote older than the asset's own last price → last price wins.
    market_quotes::upsert(&pool, "yahoo", &t, 9.0, "USD", ago_secs(10_000))
        .await
        .unwrap();
    let views = store::asset_views(&pool, &pf).await.unwrap();
    assert!((views[0].price.unwrap() - 6.0).abs() < 1e-9);

    // Delete the stale row entirely → still 6.0 (no market quote = last-price fallback).
    market_quotes::remove(&pool, "yahoo", &t).await.unwrap();
    let views = store::asset_views(&pool, &pf).await.unwrap();
    assert!((views[0].price.unwrap() - 6.0).abs() < 1e-9);
}

#[tokio::test]
async fn research_live_uses_one_fresh_quote_across_portfolios() {
    let Some(pool) = pool().await else { return };
    // Same instrument held in TWO portfolios with different (older) last prices.
    let t = rnd_ticker();
    let (pf1, a1) = holding_named(&pool, 100.0, 5.0, &t).await;
    let (pf2, a2) = holding_named(&pool, 200.0, 7.0, &t).await;
    store::set_asset_price(&pool, a1.id, 6.0).await.unwrap();
    store::set_asset_price(&pool, a2.id, 6.1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Newer shared quote wins for BOTH (Watchlist refreshed after the portfolios).
    market_quotes::upsert(&pool, "yahoo", &t, 7.92, "USD", OffsetDateTime::now_utc())
        .await
        .unwrap();

    let case = InvestmentCase {
        id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        provider: "yahoo".into(),
        provider_id: t.clone(),
        symbol: t.clone(),
        name: "Apple".into(),
        exchange: String::new(),
        currency: "USD".into(),
        asset_class: "stock".into(),
        document_id: None,
        current_action: None,
        current_confidence: None,
        valuation_view: None,
        current_biggest_question: String::new(),
        created_at: ago_secs(3600),
    };
    let pos = investment_cases::instrument_position(&pool, &case).await.unwrap();
    assert!((pos.price.unwrap() - 7.92).abs() < 1e-9);
    assert!((pos.quantity - 300.0).abs() < 1e-9);
    assert!((pos.market_value.unwrap() - 2376.0).abs() < 1e-9);
    // Every slice values at the SAME current price.
    let by_pf: Vec<_> = pos
        .portfolios
        .iter()
        .map(|s| (s.portfolio_id, s.market_value.unwrap()))
        .collect();
    let mv1 = by_pf.iter().find(|(id, _)| *id == pf1.id).unwrap().1;
    let mv2 = by_pf.iter().find(|(id, _)| *id == pf2.id).unwrap().1;
    assert!((mv1 - 792.0).abs() < 1e-9);
    assert!((mv2 - 1584.0).abs() < 1e-9);
}

#[tokio::test]
async fn research_falls_back_to_newest_last_price_without_shared_row() {
    let Some(pool) = pool().await else { return };
    let t = rnd_ticker();
    let (_pf1, a1) = holding_named(&pool, 100.0, 5.0, &t).await;
    let (_pf2, a2) = holding_named(&pool, 200.0, 7.0, &t).await;
    store::set_asset_price(&pool, a1.id, 6.0).await.unwrap();
    // a2 priced more recently at 6.1.
    std::thread::sleep(std::time::Duration::from_millis(5));
    store::set_asset_price(&pool, a2.id, 6.1).await.unwrap();

    let case = InvestmentCase {
        id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        provider: "yahoo".into(),
        provider_id: t.clone(),
        symbol: t.clone(),
        name: "Apple".into(),
        exchange: String::new(),
        currency: "USD".into(),
        asset_class: "stock".into(),
        document_id: None,
        current_action: None,
        current_confidence: None,
        valuation_view: None,
        current_biggest_question: String::new(),
        created_at: ago_secs(3600),
    };
    let pos = investment_cases::instrument_position(&pool, &case).await.unwrap();
    assert!((pos.price.unwrap() - 6.1).abs() < 1e-9, "newest last price is the fallback");
}

#[tokio::test]
async fn one_identity_keeps_one_cache_row() {
    let Some(pool) = pool().await else { return };
    market_quotes::upsert(&pool, "china", "601118", 1.0, "CNY", ago_secs(100)).await.unwrap();
    market_quotes::upsert(&pool, "china", "601118", 7.92, "CNY", ago_secs(10)).await.unwrap();
    // Duplicate keys in one input still resolve to the single stored row.
    let rows = market_quotes::get_many(
        &pool,
        &[("china".into(), "601118".into()), ("china".into(), "601118".into())],
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].price_usd, 7.92);
}

#[tokio::test]
async fn quote_updates_never_rewrite_frozen_user_snapshots() {
    let Some(pool) = pool().await else { return };
    // Regression guard: a later, fresher shared quote for the same instrument must never
    // reach back into user snapshots. Watchlist Price Observations freeze
    // price_snapshot and Investment Case decision/change records freeze snapshot_price;
    // the shared quote layer only ever writes market_quotes.
    let t = rnd_ticker();
    let wl = otw_store::watchlists::create_watchlist(&pool, "snap", "").await.unwrap();
    let item = otw_store::watchlists::add_item(&pool, wl.id, "stock", "yahoo", &t, &t, "Apple")
        .await
        .unwrap();
    let at = ago_secs(100).replace_nanosecond(0).unwrap();
    let obs = otw_store::watchlist_observations::add(&pool, item.id, at, 10.0, "USD", "frozen")
        .await
        .unwrap();

    // The quote moves later — the frozen user snapshot must keep its value.
    market_quotes::upsert(&pool, "yahoo", &t, 12.0, "USD", ago_secs(1))
        .await
        .unwrap();
    let rows = otw_store::watchlist_observations::list_for_item(&pool, item.id)
        .await
        .unwrap();
    assert_eq!(rows[0].id, obs.id);
    assert_eq!(rows[0].price_snapshot, 10.0);
    assert_eq!(rows[0].recorded_at, at);
}
