//! Targeted integration tests for watchlist price observations (migration 9000000106).
//!
//! These hit a real Postgres when `DATABASE_URL` is set (run from the deployment network,
//! pointing at a disposable test database — the suite applies the migrations itself). When
//! `DATABASE_URL` is absent the tests print a note and pass vacuously, so a plain
//! `cargo test` without a database still succeeds.
//!
//! Coverage per the Price Observation requirements:
//!   1. creating an observation persists price_snapshot / recorded_at / note / currency
//!   2. reading observations back, newest first
//!   3. editing updates the note only — price_snapshot and recorded_at stay frozen even
//!      when the item's quote changes afterwards (the snapshot is a user record, not a
//!      mirror of the live quote)
//!   4. deleting an observation
//!   5. deleting the watchlist item cascades its observations away
//!   6. deleting the whole watchlist cascades too
//!   7. different items never share observations

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use otw_store::watchlist_observations as obs;
use otw_store::watchlists as store;
use otw_store::connect_and_migrate;
use sqlx::PgPool;

/// A private, fully-migrated pool per test. Each `#[tokio::test]` runs on its own
/// runtime and owns its pool, so no connection ever crosses runtime boundaries (a shared
/// static pool across short-lived test runtimes stalls on sqlx acquire).
async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            eprintln!("DATABASE_URL not set — skipping price-observation DB tests");
            return None;
        }
    };
    Some(connect_and_migrate(&url).await.expect("test database"))
}

/// A fresh watchlist + item pair, unique per call.
async fn fixture(pool: &PgPool) -> (store::Watchlist, store::Item) {
    let name = format!("obs-test-{}", Uuid::new_v4());
    let wl = store::create_watchlist(pool, &name, "").await.unwrap();
    let item = store::add_item(pool, wl.id, "stock", "yahoo", "TEST", "TEST", "Test Corp")
        .await
        .unwrap();
    (wl, item)
}

fn recorded(seconds_ago: i64) -> OffsetDateTime {
    // Whole seconds: Postgres timestamptz keeps microseconds, so nanosecond-precision
    // instants would not round-trip exactly.
    (OffsetDateTime::now_utc() - Duration::seconds(seconds_ago)).replace_nanosecond(0).unwrap()
}

#[tokio::test]
async fn create_persists_the_snapshot() {
    let Some(pool) = pool().await else { return };
    let (_wl, item) = fixture(&pool).await;
    let at = recorded(60);
    let o = obs::add(&pool, item.id, at, 7.92, "CNY", "继续上涨，观察能否站稳 8 元")
        .await
        .unwrap();
    let rows = obs::list_for_item(&pool, item.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, o.id);
    assert_eq!(rows[0].item_id, item.id);
    assert_eq!(rows[0].price_snapshot, 7.92);
    assert_eq!(rows[0].currency, "CNY");
    assert_eq!(rows[0].note, "继续上涨，观察能否站稳 8 元");
    assert_eq!(rows[0].recorded_at, at);
}

#[tokio::test]
async fn list_is_newest_first() {
    let Some(pool) = pool().await else { return };
    let (_wl, item) = fixture(&pool).await;
    let t0 = recorded(300);
    let t1 = recorded(200);
    let t2 = recorded(100);
    obs::add(&pool, item.id, t0, 6.46, "CNY", "明显回撤").await.unwrap();
    obs::add(&pool, item.id, t1, 7.50, "CNY", "高位附近开始关注").await.unwrap();
    obs::add(&pool, item.id, t2, 7.92, "CNY", "").await.unwrap();
    let rows = obs::list_for_item(&pool, item.id).await.unwrap();
    let times: Vec<OffsetDateTime> = rows.iter().map(|r| r.recorded_at).collect();
    assert_eq!(times, vec![t2, t1, t0]); // recorded_at DESC
}

#[tokio::test]
async fn different_items_do_not_share_observations() {
    let Some(pool) = pool().await else { return };
    let (_w1, i1) = fixture(&pool).await;
    let (_w2, i2) = fixture(&pool).await;
    obs::add(&pool, i1.id, recorded(10), 1.0, "USD", "item one").await.unwrap();
    obs::add(&pool, i1.id, recorded(5), 1.1, "USD", "item one again").await.unwrap();
    obs::add(&pool, i2.id, recorded(7), 2.0, "USD", "item two").await.unwrap();
    let a = obs::list_for_item(&pool, i1.id).await.unwrap();
    let b = obs::list_for_item(&pool, i2.id).await.unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 1);
    assert!(a.iter().all(|o| o.item_id == i1.id));
    assert!(b.iter().all(|o| o.item_id == i2.id && o.note == "item two"));
}

#[tokio::test]
async fn note_edit_keeps_the_snapshot_frozen_even_after_quote_changes() {
    let Some(pool) = pool().await else { return };
    let (_wl, item) = fixture(&pool).await;
    let at = recorded(30);
    let o = obs::add(&pool, item.id, at, 7.92, "CNY", "original").await.unwrap();

    // Later quotes move on — the observation must not follow them.
    store::set_item_quote(&pool, item.id, &serde_json::json!({ "price_usd": 8.77, "currency": "CNY" }), Some("SSE"))
        .await
        .unwrap();
    let frozen = obs::get(&pool, o.id).await.unwrap().unwrap();
    assert_eq!(frozen.price_snapshot, 7.92);
    assert_eq!(frozen.recorded_at, at);

    // Editing only ever touches the note.
    let updated = obs::update_note(&pool, o.id, "revised note").await.unwrap().unwrap();
    assert_eq!(updated.note, "revised note");
    assert_eq!(updated.price_snapshot, 7.92);
    assert_eq!(updated.recorded_at, at);
    assert_eq!(updated.currency, "CNY");
}

#[tokio::test]
async fn delete_removes_and_reports_missing() {
    let Some(pool) = pool().await else { return };
    let (_wl, item) = fixture(&pool).await;
    let o = obs::add(&pool, item.id, recorded(10), 3.0, "USD", "note").await.unwrap();
    assert!(obs::delete(&pool, o.id).await.unwrap());
    assert!(!obs::delete(&pool, o.id).await.unwrap());
    assert!(obs::list_for_item(&pool, item.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_the_item_cascades_observations() {
    let Some(pool) = pool().await else { return };
    let (_wl, item) = fixture(&pool).await;
    obs::add(&pool, item.id, recorded(10), 5.0, "USD", "").await.unwrap();
    obs::add(&pool, item.id, recorded(20), 5.1, "USD", "").await.unwrap();
    assert!(store::delete_item(&pool, item.id).await.unwrap());
    assert!(store::get_item(&pool, item.id).await.unwrap().is_none());
    // The cascade removed the observations with the item.
    assert!(obs::list_for_item(&pool, item.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_the_watchlist_cascades_items_and_observations() {
    let Some(pool) = pool().await else { return };
    let (wl, item) = fixture(&pool).await;
    obs::add(&pool, item.id, recorded(10), 6.0, "USD", "").await.unwrap();
    assert!(store::delete_watchlist(&pool, wl.id).await.unwrap());
    // The existing watchlists → items cascade then runs items → observations.
    let orphans = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM watchlist_price_observations WHERE item_id = $1",
    )
    .bind(item.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphans, 0);
}
