//! Requires DATABASE_URL pointing at a disposable PostgreSQL database. Never skips.
//! All fixtures are unique; CI discards the entire service database after the job.

use otw_store::{
    connect_and_migrate, investment_cases, journal_fx, market_quotes, paper_trading as paper,
    paper_documents, portfolios,
};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("paper trading tests require a disposable DATABASE_URL");
    let migrated = connect_and_migrate(&url)
        .await
        .expect("migrate disposable test database");
    migrated.close().await;
    // One connection deliberately exposes nested pool acquisition and lock starvation.
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(&url)
        .await
        .unwrap()
}

async fn strategy(
    pool: &PgPool,
    cash: f64,
    weight: f64,
    cap: f64,
    min_fee: f64,
) -> paper::PaperStrategy {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1,$2,'test',FALSE)",
    )
    .bind(user)
    .bind(format!("paper-test-{user}"))
    .execute(pool)
    .await
    .unwrap();
    paper::create_strategy(
        pool,
        paper::NewStrategy {
            user_id: user,
            name: "test",
            ai_provider: "test",
            model: "test-model",
            prompt_version: "v1",
            currency: "USD",
            start_date: OffsetDateTime::now_utc().date(),
            initial_cash: cash,
            max_position_pct: weight,
            max_order_value: cap,
            slippage_bps: 0.0,
            fee_rate_bps: 0.0,
            sell_extra_fee_rate_bps: 0.0,
            min_fee,
        },
    )
    .await
    .unwrap()
}

async fn asset(
    pool: &PgPool,
    s: &paper::PaperStrategy,
    provider: &str,
    currency: &str,
) -> portfolios::Asset {
    let ticker = format!("PAPER{}", Uuid::new_v4().simple());
    let a = paper::add_strategy_asset(
        pool, s.user_id, s.id, "stock", provider, &ticker, &ticker, "test", currency,
    )
    .await
    .unwrap();
    quote(pool, &a, 10.0, 0).await;
    a
}

async fn quote(pool: &PgPool, a: &portfolios::Asset, price: f64, age: i64) {
    market_quotes::upsert(
        pool,
        &a.provider,
        &a.provider_id,
        price,
        &a.currency,
        OffsetDateTime::now_utc() - Duration::seconds(age),
    )
    .await
    .unwrap();
}

async fn run(
    pool: &PgPool,
    s: &paper::PaperStrategy,
    key: &str,
    decision: &str,
) -> paper::PaperRun {
    paper::create_run(
        pool,
        s.id,
        key,
        None,
        &s.model,
        &json!({"fact":"test"}),
        &json!({}),
        "test",
        decision,
        3,
        &json!([]),
        &json!({}),
        &json!({"cash":s.initial_cash}),
    )
    .await
    .unwrap()
}

async fn order(
    pool: &PgPool,
    s: &paper::PaperStrategy,
    r: &paper::PaperRun,
    a: &portfolios::Asset,
    key: &str,
    side: &str,
    qty: f64,
) -> Result<paper::PaperOrderView, paper::OrderError> {
    paper::execute_order(pool, s.user_id, s.id, r.id, a.id, key, side, qty, "test").await
}

async fn counts(pool: &PgPool, s: &paper::PaperStrategy) -> (i64, i64) {
    let operations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM portfolio_operations WHERE portfolio_id = $1")
            .bind(s.portfolio_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let orders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM paper_trading_orders WHERE strategy_id = $1")
            .bind(s.id)
            .fetch_one(pool)
            .await
            .unwrap();
    (operations, orders)
}

#[tokio::test]
async fn fill_uses_server_quote_and_portfolio_ledger_cash_with_fees() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 2.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let buy = run(&p, &s, "buy", "buy").await;
    let filled = order(&p, &s, &buy, &a, "buy", "buy", 10.0).await.unwrap();
    assert_eq!(filled.fill_price, 10.0);
    assert_eq!(filled.fee, 2.0);
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 898.0);
    portfolios::set_asset_price(&p, a.id, 999999.0)
        .await
        .unwrap();
    let (views, cash, complete) = paper::account_values(&p, &s).await.unwrap();
    assert_eq!(cash, 898.0);
    assert!(complete);
    assert_eq!(views[0].quantity, 10.0);
    assert_eq!(
        views[0].price,
        Some(10.0),
        "paper marks must use shared quotes even if a local price is newer"
    );
    let op = portfolios::list_operations(&p, a.id).await.unwrap();
    assert_eq!(op[0].id, filled.operation_id);
    assert_eq!(op[0].quantity, 10.0);
    let sell = run(&p, &s, "sell", "exit").await;
    quote(&p, &a, 12.0, 0).await;
    order(&p, &s, &sell, &a, "sell", "sell", 10.0)
        .await
        .unwrap();
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 1016.0);
    assert_eq!(counts(&p, &s).await, (2, 2));
}

#[tokio::test]
async fn retries_survive_stale_quotes_and_terminal_status_without_duplicate_operations() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "run", "buy").await;
    let first = order(&p, &s, &r, &a, "same", "buy", 5.0).await.unwrap();
    quote(&p, &a, 50.0, 120).await;
    paper::set_strategy_status(&p, s.id, s.user_id, "ended")
        .await
        .unwrap()
        .unwrap();
    let retry = order(&p, &s, &r, &a, "same", "buy", 5.0).await.unwrap();
    assert_eq!(first.id, retry.id);
    assert_eq!(first.fill_price, retry.fill_price);
    assert!(matches!(
        paper::execute_order(
            &p,
            s.user_id,
            s.id,
            r.id,
            a.id,
            "same",
            "buy",
            5.0,
            "changed note"
        )
        .await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert!(matches!(
        order(&p, &s, &r, &a, "same", "buy", 5.0000000001).await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert_eq!(run(&p, &s, "run", "buy").await.id, r.id);
    assert!(paper::set_strategy_status(&p, s.id, s.user_id, "active")
        .await
        .unwrap()
        .is_none());
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn runs_freeze_snapshot_and_reject_different_payload_or_schedule() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let r = run(&p, &s, "observe", "no_trade").await;
    let retry = paper::create_run(
        &p,
        s.id,
        "observe",
        None,
        &s.model,
        &json!({"fact":"test"}),
        &json!({}),
        "test",
        "no_trade",
        3,
        &json!([]),
        &json!({}),
        &json!({"cash":1}),
    )
    .await
    .unwrap();
    assert_eq!(retry.account_snapshot, r.account_snapshot);
    assert!(paper::create_run(
        &p,
        s.id,
        "observe",
        Some(r.scheduled_at + Duration::hours(1)),
        &s.model,
        &r.market_facts,
        &r.market_expectations,
        "test",
        "no_trade",
        3,
        &r.evidence,
        &r.risk_assessment,
        &json!({})
    )
    .await
    .is_err());
    assert!(paper::create_run(
        &p,
        s.id,
        "observe",
        None,
        &s.model,
        &r.market_facts,
        &r.market_expectations,
        "different",
        "no_trade",
        3,
        &r.evidence,
        &r.risk_assessment,
        &json!({})
    )
    .await
    .is_err());
    assert_eq!(paper::list_runs(&p, s.id, 200).await.unwrap().len(), 1);
    let precise = r.scheduled_at.replace_nanosecond(123456789).unwrap();
    let first = paper::create_run(
        &p,
        s.id,
        "precise",
        Some(precise),
        &s.model,
        &r.market_facts,
        &r.market_expectations,
        "test",
        "no_trade",
        3,
        &r.evidence,
        &r.risk_assessment,
        &json!({}),
    )
    .await
    .unwrap();
    let retry = paper::create_run(
        &p,
        s.id,
        "precise",
        Some(precise),
        &s.model,
        &r.market_facts,
        &r.market_expectations,
        "test",
        "no_trade",
        3,
        &r.evidence,
        &r.risk_assessment,
        &json!({}),
    )
    .await
    .unwrap();
    assert_eq!(first.id, retry.id);
    assert_eq!(counts(&p, &s).await, (0, 0));
}

#[tokio::test]
async fn cash_order_value_and_position_limits_reject_without_writing() {
    let p = pool().await;
    for (cash, weight, cap, quantity) in [
        (100.0, 1.0, 0.0, 11.0),
        (1000.0, 1.0, 50.0, 6.0),
        (1000.0, 0.5, 0.0, 51.0),
    ] {
        let s = strategy(&p, cash, weight, cap, 0.0).await;
        let a = asset(&p, &s, "yahoo", "USD").await;
        let r = run(&p, &s, "buy", "buy").await;
        assert!(matches!(
            order(&p, &s, &r, &a, "over", "buy", quantity).await,
            Err(paper::OrderError::Conflict(_))
        ));
        assert_eq!(counts(&p, &s).await, (0, 0));
    }
}

#[tokio::test]
async fn stale_missing_future_quotes_and_overflow_fail_closed() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    for age in [120, -120] {
        quote(&p, &a, 10.0, age).await;
        assert!(matches!(
            order(&p, &s, &r, &a, "stale", "buy", 1.0).await,
            Err(paper::OrderError::Conflict(_))
        ));
    }
    sqlx::query("DELETE FROM market_quotes WHERE provider = $1 AND provider_id = $2")
        .bind(&a.provider)
        .bind(&a.provider_id)
        .execute(&p)
        .await
        .unwrap();
    assert!(order(&p, &s, &r, &a, "missing", "buy", 1.0).await.is_err());
    quote(&p, &a, 10.0, 0).await;
    assert!(matches!(
        order(&p, &s, &r, &a, "overflow", "buy", f64::MAX).await,
        Err(paper::OrderError::Invalid(_))
    ));
    assert_eq!(counts(&p, &s).await, (0, 0));
}

#[tokio::test]
async fn concurrent_identical_keys_commit_one_fill_with_one_connection() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    let (one, two) = tokio::join!(
        order(&p, &s, &r, &a, "same", "buy", 5.0),
        order(&p, &s, &r, &a, "same", "buy", 5.0)
    );
    assert_eq!(one.unwrap().id, two.unwrap().id);
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn concurrent_distinct_orders_cannot_overspend_shared_cash() {
    let p = pool().await;
    // Multiple connections exercise the strategy row lock, rather than pool serialization.
    let url = std::env::var("DATABASE_URL").unwrap();
    let concurrent = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    let s = strategy(&p, 100.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let b = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    let (one, two) = tokio::join!(
        order(&concurrent, &s, &r, &a, "one", "buy", 6.0),
        order(&concurrent, &s, &r, &b, "two", "buy", 6.0)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 40.0);
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn wrong_user_run_asset_and_paused_strategy_are_rejected() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let other = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let foreign = asset(&p, &other, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    let foreign_run = run(&p, &other, "buy", "buy").await;
    assert!(paper::get_strategy_for_user(&p, s.id, other.user_id)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        paper::execute_order(
            &p,
            other.user_id,
            s.id,
            r.id,
            a.id,
            "wrong-user",
            "buy",
            1.0,
            ""
        )
        .await,
        Err(paper::OrderError::NotFound)
    ));
    assert!(matches!(
        order(&p, &s, &r, &foreign, "wrong-asset", "buy", 1.0).await,
        Err(paper::OrderError::NotFound)
    ));
    assert!(matches!(
        order(&p, &s, &foreign_run, &a, "wrong-run", "buy", 1.0).await,
        Err(paper::OrderError::NotFound)
    ));
    paper::set_strategy_status(&p, s.id, s.user_id, "paused")
        .await
        .unwrap();
    assert!(matches!(
        order(&p, &s, &r, &a, "paused", "buy", 1.0).await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert_eq!(counts(&p, &s).await, (0, 0));
    assert_eq!(counts(&p, &other).await, (0, 0));
}

#[tokio::test]
async fn duplicate_asset_cannot_rewrite_currency_or_bypass_t_plus_one() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "china", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    order(&p, &s, &r, &a, "buy", "buy", 10.0).await.unwrap();
    let changed = paper::add_strategy_asset(
        &p,
        s.user_id,
        s.id,
        "stock",
        "china",
        &a.provider_id,
        &a.symbol,
        "",
        "CNY",
    )
    .await;
    assert!(matches!(changed, Err(paper::OrderError::Conflict(_))));
    let changed_class = paper::add_strategy_asset(
        &p,
        s.user_id,
        s.id,
        "etf",
        "china",
        &a.provider_id,
        &a.symbol,
        "",
        "USD",
    )
    .await;
    assert!(matches!(changed_class, Err(paper::OrderError::Conflict(_))));
    let exit = run(&p, &s, "exit", "exit").await;
    assert!(matches!(
        order(&p, &s, &exit, &a, "same-day", "sell", 10.0).await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 900.0);
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn oversells_and_decision_mismatch_do_not_write() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let buy = run(&p, &s, "buy", "buy").await;
    order(&p, &s, &buy, &a, "buy", "buy", 5.0).await.unwrap();
    let sell = run(&p, &s, "sell", "exit").await;
    assert!(matches!(
        order(&p, &s, &sell, &a, "oversell", "sell", 6.0).await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert!(matches!(
        order(&p, &s, &sell, &a, "tiny-oversell", "sell", 5.0000000001).await,
        Err(paper::OrderError::Conflict(_))
    ));
    assert!(matches!(
        order(&p, &s, &buy, &a, "wrong-side", "sell", 1.0).await,
        Err(paper::OrderError::Invalid(_))
    ));
    let hold = run(&p, &s, "hold", "hold").await;
    assert!(matches!(
        order(&p, &s, &hold, &a, "hold", "buy", 1.0).await,
        Err(paper::OrderError::Invalid(_))
    ));
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn failure_in_order_insert_rolls_back_portfolio_fill() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    // A test-only trigger injects a failure AFTER the engine inserts the operation.
    let trigger = format!("paper_test_{}", Uuid::new_v4().simple());
    let function_sql = format!("CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.strategy_id = '{}'::uuid THEN RAISE EXCEPTION 'injected audit failure'; END IF; RETURN NEW; END $$", s.id);
    sqlx::query(sqlx::AssertSqlSafe(function_sql))
        .execute(&p)
        .await
        .unwrap();
    let trigger_sql = format!("CREATE TRIGGER {trigger} BEFORE INSERT ON paper_trading_orders FOR EACH ROW EXECUTE FUNCTION {trigger}()");
    sqlx::query(sqlx::AssertSqlSafe(trigger_sql))
        .execute(&p)
        .await
        .unwrap();
    assert!(matches!(
        order(&p, &s, &r, &a, "rollback", "buy", 5.0).await,
        Err(paper::OrderError::Internal(_))
    ));
    assert_eq!(counts(&p, &s).await, (0, 0));
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 1000.0);
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER {trigger} ON paper_trading_orders"
    )))
    .execute(&p)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP FUNCTION {trigger}()")))
        .execute(&p)
        .await
        .unwrap();
}

#[tokio::test]
async fn research_aggregates_only_normal_portfolios() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let r = run(&p, &s, "buy", "buy").await;
    order(&p, &s, &r, &a, "paper", "buy", 10.0).await.unwrap();
    let real = portfolios::create_portfolio(&p, "normal fixture", "", "USD")
        .await
        .unwrap();
    let real_asset = portfolios::add_asset(
        &p,
        real.id,
        "stock",
        "yahoo",
        &a.provider_id,
        &a.symbol,
        "",
        "USD",
    )
    .await
    .unwrap();
    portfolios::add_operation(
        &p,
        real_asset.id,
        "buy",
        OffsetDateTime::now_utc().date(),
        3.0,
        10.0,
        0.0,
        "test fixture",
    )
    .await
    .unwrap();
    let case = investment_cases::InvestmentCase {
        id: Uuid::new_v4(),
        user_id: s.user_id,
        document_id: None,
        provider: a.provider,
        provider_id: a.provider_id,
        symbol: a.symbol,
        name: "fixture".into(),
        exchange: String::new(),
        asset_class: "stock".into(),
        currency: "USD".into(),
        current_action: None,
        current_confidence: None,
        valuation_view: None,
        current_biggest_question: String::new(),
        created_at: OffsetDateTime::now_utc(),
    };
    let position = investment_cases::instrument_position(&p, &case)
        .await
        .unwrap();
    assert_eq!(position.quantity, 3.0);
    assert_eq!(counts(&p, &s).await, (1, 1));
}

#[tokio::test]
async fn account_cash_uses_operation_date_fx() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let today = OffsetDateTime::now_utc().date();
    journal_fx::upsert_rates(&p, today, &[("CNY".into(), 7.0)], "manual")
        .await
        .unwrap();
    let a = asset(&p, &s, "yahoo", "CNY").await;
    let r = run(&p, &s, "buy", "buy").await;
    let fill = order(&p, &s, &r, &a, "fx", "buy", 10.0).await.unwrap();
    assert_eq!(fill.fill_price, 70.0);
    assert_eq!(fill.fee_currency, "CNY");
    assert_eq!(paper::cash_balance(&p, &s).await.unwrap(), 900.0);
}

#[tokio::test]
async fn slippage_commission_and_sell_levy_are_applied_server_side() {
    let p = pool().await;
    let s = strategy(&p, 10_000.0, 1.0, 0.0, 5.0).await;
    sqlx::query("UPDATE paper_trading_strategies SET slippage_bps = 5, fee_rate_bps = 3, sell_extra_fee_rate_bps = 5 WHERE id = $1")
        .bind(s.id).execute(&p).await.unwrap();
    let a = asset(&p, &s, "yahoo", "USD").await;
    let buy = run(&p, &s, "buy", "buy").await;
    let fill = order(&p, &s, &buy, &a, "buy", "buy", 10.0).await.unwrap();
    assert!((fill.fill_price - 10.005).abs() < 1e-9);
    assert_eq!(fill.fee, 5.0);
    let sell = run(&p, &s, "sell", "exit").await;
    let fill = order(&p, &s, &sell, &a, "sell", "sell", 10.0)
        .await
        .unwrap();
    assert!((fill.fill_price - 9.995).abs() < 1e-9);
    assert!((fill.fee - 5.049975).abs() < 1e-9);
    assert!((paper::cash_balance(&p, &s).await.unwrap() - 9989.850025).abs() < 1e-8);
}

#[tokio::test]
async fn oauth_paper_permission_remains_opt_in_and_survives_refresh_without_widening() {
    use otw_store::oauth;
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let (client, _) = oauth::create_client(
        &p,
        "paper test",
        &["https://client.example/callback".into()],
        &[
            oauth::SCOPE_MCP_READ.into(),
            oauth::SCOPE_MCP_PAPER_TRADE.into(),
        ],
        "pre_registered",
    )
    .await
    .unwrap();
    let read = oauth::issue_tokens(
        &p,
        client.id,
        s.user_id,
        "https://otw.example/api/mcp",
        &[oauth::SCOPE_MCP_READ.into()],
    )
    .await
    .unwrap();
    let row = oauth::find_access_token(&p, &read.access_token)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.has_scope(oauth::SCOPE_MCP_PAPER_TRADE));
    let paper = oauth::issue_tokens(
        &p,
        client.id,
        s.user_id,
        "https://otw.example/api/mcp",
        &[
            oauth::SCOPE_MCP_READ.into(),
            oauth::SCOPE_MCP_PAPER_TRADE.into(),
        ],
    )
    .await
    .unwrap();
    let refreshed = oauth::rotate_refresh(
        &p,
        &paper.refresh_token,
        client.id,
        "https://otw.example/api/mcp",
    )
    .await
    .unwrap()
    .unwrap();
    let row = oauth::find_access_token(&p, &refreshed.access_token)
        .await
        .unwrap()
        .unwrap();
    assert!(row.has_scope(oauth::SCOPE_MCP_PAPER_TRADE));
    assert!(!row.has_scope("portfolios:rw"));
    assert_eq!(row.scopes.len(), 2);
}

#[tokio::test]
async fn strategy_editor_thesis_is_idempotent_versioned_and_recoverable() {
    let p = pool().await;
    let s = strategy(&p, 10_000.0, 1.0, 0.0, 0.0).await;
    let a = asset(&p, &s, "yahoo", "USD").await;
    let created = paper_documents::ensure(&p, s.id, a.id).await.unwrap().unwrap();
    let repeat = paper_documents::ensure(&p, s.id, a.id).await.unwrap().unwrap();
    assert_eq!(created.id, repeat.id);
    assert_eq!(paper_documents::list(&p, s.id).await.unwrap().len(), 1);

    let revised = json!({"type":"doc","content":[{"type":"paragraph",
        "content":[{"type":"text","text":"Buy only after verified earnings."}]}]});
    assert_eq!(
        paper_documents::update(&p, s.id, a.id, created.updated_at, &revised)
            .await.unwrap(),
        paper_documents::UpdateOutcome::Updated
    );
    assert_eq!(
        paper_documents::update(&p, s.id, a.id, created.updated_at, &revised)
            .await.unwrap(),
        paper_documents::UpdateOutcome::Unchanged
    );
    let newer = json!({"type":"doc","content":[{"type":"paragraph",
        "content":[{"type":"text","text":"Hold cash."}]}]});
    assert_eq!(
        paper_documents::update(&p, s.id, a.id, created.updated_at, &newer)
            .await.unwrap(),
        paper_documents::UpdateOutcome::Stale
    );
    let current = paper_documents::get(&p, s.id, a.id).await.unwrap().unwrap();
    assert_eq!(current.content, Some(revised));

    otw_store::documents::delete(&p, created.id).await.unwrap();
    assert!(paper_documents::get(&p, s.id, a.id).await.unwrap().is_none());
    let restored = paper_documents::ensure(&p, s.id, a.id).await.unwrap().unwrap();
    assert_ne!(created.id, restored.id);
}

#[tokio::test]
async fn daily_activity_pages_use_historical_run_equity_not_current_cash() {
    let p = pool().await;
    let s = strategy(&p, 1000.0, 1.0, 0.0, 0.0).await;
    let old = OffsetDateTime::now_utc() - Duration::days(2);
    paper::create_run(&p, s.id, "day-old", Some(old), &s.model,
        &json!({}), &json!({}), "wait", "no_trade", 3, &json!([]),
        &json!({}), &json!({"equity":1000.0})).await.unwrap();
    paper::create_run(&p, s.id, "day-new", Some(OffsetDateTime::now_utc()), &s.model,
        &json!({}), &json!({}), "still wait", "no_trade", 3, &json!([]),
        &json!({}), &json!({"equity":1012.0})).await.unwrap();

    let (total, first) = paper::list_day_summaries(&p, s.id, s.initial_cash, 1, 0)
        .await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["estimated_pnl"], json!(12.0));
    assert_eq!(first[0]["run_count"], json!(1));
    let (_, older) = paper::list_day_summaries(&p, s.id, s.initial_cash, 1, 1)
        .await.unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0]["estimated_pnl"], json!(0.0));
}
