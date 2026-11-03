//! Paper Trading persistence and safe execution boundary.
//!
//! Strategy metadata and AI audit records live here. All actual fills are written to
//! Portfolio Tracker's portfolio_operations ledger; no second position ledger exists.

use std::collections::HashMap;

use anyhow::{anyhow, Context};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::{journal_fx, portfolios};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PaperStrategy {
    pub id: Uuid,
    pub user_id: Uuid,
    pub portfolio_id: Uuid,
    pub name: String,
    pub ai_provider: String,
    pub model: String,
    pub prompt_version: String,
    pub currency: String,
    #[serde(with = "date_fmt")]
    pub start_date: Date,
    pub initial_cash: f64,
    pub status: String,
    pub max_position_pct: f64,
    pub max_order_value: f64,
    pub slippage_bps: f64,
    pub fee_rate_bps: f64,
    pub sell_extra_fee_rate_bps: f64,
    pub min_fee: f64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PaperRun {
    pub id: Uuid,
    pub strategy_id: Uuid,
    pub idempotency_key: String,
    #[serde(with = "time::serde::rfc3339")]
    pub scheduled_at: OffsetDateTime,
    pub model: String,
    pub market_facts: Value,
    pub market_expectations: Value,
    pub inference: String,
    pub decision: String,
    pub confidence: i16,
    pub evidence: Value,
    pub risk_assessment: Value,
    pub account_snapshot: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PaperOrderView {
    pub id: Uuid,
    pub strategy_id: Uuid,
    pub run_id: Uuid,
    pub asset_id: Uuid,
    pub idempotency_key: String,
    pub side: String,
    pub requested_quantity: f64,
    pub quote_price_usd: f64,
    #[serde(with = "time::serde::rfc3339")]
    pub quote_at: OffsetDateTime,
    pub slippage_bps: f64,
    pub operation_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Fill price and fee are read from the Portfolio operation, their sole source of truth.
    pub fill_price: f64,
    pub fee: f64,
    pub fee_currency: String,
    pub note: String,
    #[serde(with = "date_fmt")]
    pub op_date: Date,
    pub symbol: String,
}

mod date_fmt {
    use serde::Serializer;
    use time::{format_description::well_known::Iso8601, Date};
    pub fn serialize<S: Serializer>(d: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(
            &d.format(&Iso8601::DATE)
                .map_err(serde::ser::Error::custom)?,
        )
    }
}

pub struct NewStrategy<'a> {
    pub user_id: Uuid,
    pub name: &'a str,
    pub ai_provider: &'a str,
    pub model: &'a str,
    pub prompt_version: &'a str,
    pub currency: &'a str,
    pub start_date: Date,
    pub initial_cash: f64,
    pub max_position_pct: f64,
    pub max_order_value: f64,
    pub slippage_bps: f64,
    pub fee_rate_bps: f64,
    pub sell_extra_fee_rate_bps: f64,
    pub min_fee: f64,
}

pub async fn create_strategy(
    pool: &PgPool,
    input: NewStrategy<'_>,
) -> anyhow::Result<PaperStrategy> {
    if !input.initial_cash.is_finite()
        || input.initial_cash <= 0.0
        || !input.max_position_pct.is_finite()
        || input.max_position_pct <= 0.0
        || input.max_position_pct > 1.0
        || !input.max_order_value.is_finite()
        || input.max_order_value < 0.0
        || !input.min_fee.is_finite()
        || input.min_fee < 0.0
        || [
            input.slippage_bps,
            input.fee_rate_bps,
            input.sell_extra_fee_rate_bps,
        ]
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1000.0).contains(value))
    {
        return Err(anyhow!("invalid paper strategy financial limits"));
    }
    let mut tx = pool
        .begin()
        .await
        .context("begin paper strategy creation")?;
    let portfolio_id = Uuid::new_v4();
    let strategy_id = Uuid::new_v4();
    let portfolio_name = format!("Paper · {}", input.name.trim());
    let description = format!("Paper Trading account for strategy: {}", input.name.trim());

    sqlx::query(
        "INSERT INTO portfolios (id, name, description, currency, auto_refresh, position) \
         VALUES ($1, $2, $3, $4, TRUE, (SELECT COALESCE(MAX(position), 0) + 1 FROM portfolios))",
    )
    .bind(portfolio_id)
    .bind(&portfolio_name)
    .bind(&description)
    .bind(input.currency)
    .execute(&mut *tx)
    .await
    .context("creating paper portfolio")?;

    let strategy = sqlx::query_as::<_, PaperStrategy>(
        "INSERT INTO paper_trading_strategies \
         (id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, start_date, initial_cash, \
          max_position_pct, max_order_value, slippage_bps, fee_rate_bps, sell_extra_fee_rate_bps, min_fee) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) \
         RETURNING id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, start_date, initial_cash, \
                   status, max_position_pct, max_order_value, slippage_bps, fee_rate_bps, \
                   sell_extra_fee_rate_bps, min_fee, created_at, updated_at",
    )
    .bind(strategy_id)
    .bind(input.user_id)
    .bind(portfolio_id)
    .bind(input.name.trim())
    .bind(input.ai_provider.trim())
    .bind(input.model.trim())
    .bind(input.prompt_version.trim())
    .bind(input.currency)
    .bind(input.start_date)
    .bind(input.initial_cash)
    .bind(input.max_position_pct)
    .bind(input.max_order_value)
    .bind(input.slippage_bps)
    .bind(input.fee_rate_bps)
    .bind(input.sell_extra_fee_rate_bps)
    .bind(input.min_fee)
    .fetch_one(&mut *tx)
    .await
    .context("creating paper strategy")?;

    tx.commit()
        .await
        .context("commit paper strategy creation")?;
    Ok(strategy)
}

const STRATEGY_COLS: &str = "id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, start_date, initial_cash, status, max_position_pct, max_order_value, slippage_bps, fee_rate_bps, sell_extra_fee_rate_bps, min_fee, created_at, updated_at";

pub async fn list_strategies(pool: &PgPool, user_id: Uuid) -> anyhow::Result<Vec<PaperStrategy>> {
    let sql = format!("SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE user_id = $1 ORDER BY created_at DESC");
    Ok(sqlx::query_as::<_, PaperStrategy>(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(pool)
        .await
        .context("listing paper strategies")?)
}

pub async fn get_strategy(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<PaperStrategy>> {
    let sql = format!("SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE id = $1");
    Ok(sqlx::query_as::<_, PaperStrategy>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("fetching paper strategy")?)
}

pub async fn get_strategy_for_user(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<PaperStrategy>> {
    let sql = format!(
        "SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE id = $1 AND user_id = $2"
    );
    Ok(sqlx::query_as::<_, PaperStrategy>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .context("fetching owned paper strategy")?)
}

/// Change strategy status. Ended strategies are terminal and cannot be reactivated.
pub async fn set_strategy_status(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    status: &str,
) -> anyhow::Result<Option<PaperStrategy>> {
    let sql = format!(
        "UPDATE paper_trading_strategies SET status = $3, updated_at = now() \
         WHERE id = $1 AND user_id = $2 AND (status <> 'ended' OR $3 = 'ended') \
         RETURNING {STRATEGY_COLS}"
    );
    Ok(sqlx::query_as::<_, PaperStrategy>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .bind(status)
        .fetch_optional(pool)
        .await
        .context("updating paper strategy status")?)
}

pub async fn is_paper_portfolio(pool: &PgPool, portfolio_id: Uuid) -> anyhow::Result<bool> {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM paper_trading_strategies WHERE portfolio_id = $1)",
    )
    .bind(portfolio_id)
    .fetch_one(pool)
    .await
    .context("checking paper portfolio")?;
    Ok(exists)
}

/// All portfolio ids owned by a paper strategy. The normal Portfolio Tracker lists only real
/// portfolios; paper rows are managed exclusively from the AI Paper Trading page.
pub async fn paper_portfolio_ids(pool: &PgPool) -> anyhow::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT portfolio_id FROM paper_trading_strategies")
        .fetch_all(pool)
        .await
        .context("listing paper portfolio ids")
}

pub async fn is_paper_asset(pool: &PgPool, asset_id: Uuid) -> anyhow::Result<bool> {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM portfolio_assets a \
         JOIN paper_trading_strategies s ON s.portfolio_id = a.portfolio_id WHERE a.id = $1)",
    )
    .bind(asset_id)
    .fetch_one(pool)
    .await
    .context("checking paper asset")?;
    Ok(exists)
}

pub async fn is_paper_operation(pool: &PgPool, operation_id: Uuid) -> anyhow::Result<bool> {
    let (exists,): (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM portfolio_operations o \
         JOIN paper_trading_strategies s ON s.portfolio_id = o.portfolio_id WHERE o.id = $1)",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .context("checking paper operation")?;
    Ok(exists)
}

/// Adding an existing instrument must never re-denominate its operation ledger.
/// Serialize with orders and status changes using the same strategy lock.
pub async fn add_strategy_asset(
    pool: &PgPool,
    user_id: Uuid,
    strategy_id: Uuid,
    asset_class: &str,
    provider: &str,
    provider_id: &str,
    symbol: &str,
    name: &str,
    currency: &str,
) -> Result<portfolios::Asset, OrderError> {
    let mut tx = pool.begin().await.context("begin paper asset")?;
    let (portfolio_id, status): (Uuid, String) = sqlx::query_as(
        "SELECT portfolio_id, status FROM paper_trading_strategies WHERE id = $1 AND user_id = $2 FOR UPDATE",
    ).bind(strategy_id).bind(user_id).fetch_optional(&mut *tx).await
        .context("locking paper strategy for asset")?.ok_or(OrderError::NotFound)?;
    let existing: Option<portfolios::Asset> = sqlx::query_as(
        "SELECT * FROM portfolio_assets WHERE portfolio_id = $1 AND provider = $2 AND provider_id = $3",
    ).bind(portfolio_id).bind(provider).bind(provider_id).fetch_optional(&mut *tx).await
        .context("finding paper instrument")?;
    if let Some(asset) = existing {
        if asset.currency != currency || asset.asset_class != asset_class {
            return Err(OrderError::Conflict(
                "an existing paper instrument's currency and asset class are immutable".into(),
            ));
        }
        tx.commit().await.context("commit existing paper asset")?;
        return Ok(asset);
    }
    if status == "ended" {
        return Err(OrderError::Conflict(
            "cannot add assets to an ended strategy".into(),
        ));
    }
    let asset = sqlx::query_as(
        "INSERT INTO portfolio_assets (id, portfolio_id, asset_class, provider, provider_id, symbol, name, currency) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *",
    ).bind(Uuid::new_v4()).bind(portfolio_id).bind(asset_class).bind(provider)
        .bind(provider_id).bind(symbol).bind(name).bind(currency)
        .fetch_one(&mut *tx).await.context("inserting paper instrument")?;
    tx.commit().await.context("commit paper asset")?;
    Ok(asset)
}

pub async fn create_run(
    pool: &PgPool,
    strategy_id: Uuid,
    idempotency_key: &str,
    scheduled_at: Option<OffsetDateTime>,
    model: &str,
    market_facts: &Value,
    market_expectations: &Value,
    inference: &str,
    decision: &str,
    confidence: i16,
    evidence: &Value,
    risk_assessment: &Value,
    account_snapshot: &Value,
) -> anyhow::Result<PaperRun> {
    // PostgreSQL timestamps have microsecond precision. Normalize explicitly so
    // repeating an RFC3339 request with nanoseconds compares to its stored value.
    let scheduled_at = scheduled_at.map(|at| {
        at.replace_nanosecond(at.nanosecond() / 1000 * 1000)
            .expect("valid microsecond timestamp")
    });
    let mut tx = pool.begin().await.context("begin paper run")?;
    let status: String =
        sqlx::query_scalar("SELECT status FROM paper_trading_strategies WHERE id = $1 FOR UPDATE")
            .bind(strategy_id)
            .fetch_one(&mut *tx)
            .await
            .context("locking run strategy")?;
    let existing: Option<PaperRun> = sqlx::query_as(
        "SELECT * FROM paper_trading_runs WHERE strategy_id = $1 AND idempotency_key = $2",
    )
    .bind(strategy_id)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await
    .context("loading idempotent paper run")?;
    if let Some(existing) = existing {
        if existing.decision != decision
            || existing.confidence != confidence
            || existing.inference != inference
            || existing.model != model
            || existing.market_facts != *market_facts
            || existing.market_expectations != *market_expectations
            || existing.evidence != *evidence
            || existing.risk_assessment != *risk_assessment
            || scheduled_at.is_some_and(|at| at != existing.scheduled_at)
        {
            return Err(anyhow!(
                "idempotency key already used for a different paper run"
            ));
        }
        tx.commit().await.context("commit existing paper run")?;
        return Ok(existing);
    }
    if status == "ended" {
        return Err(anyhow!("cannot record new runs for an ended strategy"));
    }
    let id = Uuid::new_v4();
    let run = sqlx::query_as::<_, PaperRun>(
        "INSERT INTO paper_trading_runs \
         (id, strategy_id, idempotency_key, scheduled_at, model, market_facts, market_expectations, \
          inference, decision, confidence, evidence, risk_assessment, account_snapshot) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
         ON CONFLICT (strategy_id, idempotency_key) DO NOTHING \
         RETURNING id, strategy_id, idempotency_key, scheduled_at, model, market_facts, market_expectations, \
                   inference, decision, confidence, evidence, risk_assessment, account_snapshot, created_at",
    )
    .bind(id)
    .bind(strategy_id)
    .bind(idempotency_key)
    .bind(scheduled_at.unwrap_or_else(OffsetDateTime::now_utc))
    .bind(model)
    .bind(market_facts)
    .bind(market_expectations)
    .bind(inference)
    .bind(decision)
    .bind(confidence)
    .bind(evidence)
    .bind(risk_assessment)
    .bind(account_snapshot)
    .fetch_one(&mut *tx)
    .await
    .context("creating paper run")?;

    tx.commit().await.context("commit paper run")?;
    Ok(run)
}

pub async fn get_run_by_key(
    pool: &PgPool,
    strategy_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<PaperRun>> {
    Ok(sqlx::query_as(
        "SELECT * FROM paper_trading_runs WHERE strategy_id = $1 AND idempotency_key = $2",
    )
    .bind(strategy_id)
    .bind(key)
    .fetch_optional(pool)
    .await?)
}

pub async fn get_order_by_key(
    pool: &PgPool,
    strategy_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<PaperOrderView>> {
    let sql = format!("{ORDER_VIEW_SELECT} WHERE po.strategy_id = $1 AND po.idempotency_key = $2");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(strategy_id)
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

pub async fn list_runs(
    pool: &PgPool,
    strategy_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<PaperRun>> {
    Ok(sqlx::query_as(
        "SELECT id, strategy_id, idempotency_key, scheduled_at, model, market_facts, market_expectations, \
                inference, decision, confidence, evidence, risk_assessment, account_snapshot, created_at \
         FROM paper_trading_runs WHERE strategy_id = $1 ORDER BY scheduled_at DESC, created_at DESC LIMIT $2",
    )
    .bind(strategy_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await
    .context("listing paper runs")?)
}

const ORDER_VIEW_SELECT: &str = "SELECT po.id, po.strategy_id, po.run_id, po.asset_id, po.idempotency_key, po.side, po.requested_quantity, po.quote_price_usd, po.quote_at, po.slippage_bps, po.operation_id, po.created_at, op.price AS fill_price, op.fee, a.currency AS fee_currency, op.note, op.op_date, a.symbol FROM paper_trading_orders po JOIN portfolio_operations op ON op.id = po.operation_id JOIN portfolio_assets a ON a.id = po.asset_id";

pub async fn list_orders(
    pool: &PgPool,
    strategy_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<PaperOrderView>> {
    let sql = format!(
        "{ORDER_VIEW_SELECT} WHERE po.strategy_id = $1 ORDER BY po.created_at DESC LIMIT $2"
    );
    Ok(
        sqlx::query_as::<_, PaperOrderView>(sqlx::AssertSqlSafe(sql))
            .bind(strategy_id)
            .bind(limit.clamp(1, 200))
            .fetch_all(pool)
            .await
            .context("listing paper orders")?,
    )
}

pub async fn cash_balance(pool: &PgPool, strategy: &PaperStrategy) -> anyhow::Result<f64> {
    cash_balance_on(&mut *pool.acquire().await?, strategy).await
}

/// A coherent paper context from the Portfolio ledger, using only shared quotes.
/// The strategy lock prevents snapshots combining holdings from before an order
/// with cash from after it; all database work uses the same connection.
pub async fn account_values(
    pool: &PgPool,
    strategy: &PaperStrategy,
) -> anyhow::Result<(Vec<portfolios::AssetView>, f64, bool)> {
    let mut tx = pool.begin().await.context("begin paper context")?;
    sqlx::query("SELECT id FROM paper_trading_strategies WHERE id = $1 AND user_id = $2 FOR SHARE")
        .bind(strategy.id)
        .bind(strategy.user_id)
        .fetch_one(&mut *tx)
        .await
        .context("locking paper context")?;
    let assets = portfolios::list_assets_on(&mut tx, strategy.portfolio_id).await?;
    let today = OffsetDateTime::now_utc().date();
    let mut fx = journal_fx::FxCache::new();
    let fx_available = fx
        .convert_on(&mut tx, 1.0, "USD", &strategy.currency, today)
        .await?
        .is_some_and(|rate| rate.is_finite() && rate > 0.0);
    let mut views = Vec::with_capacity(assets.len());
    for mut asset in assets {
        let quote: Option<(f64, OffsetDateTime)> = sqlx::query_as(
            "SELECT price_usd, quote_at FROM market_quotes WHERE provider = $1 AND provider_id = $2",
        ).bind(&asset.provider).bind(&asset.provider_id).fetch_optional(&mut *tx).await
            .context("reading paper context quote")?;
        let ops = portfolios::list_operations_on(&mut tx, asset.id).await?;
        // Portfolio fallback prices are useful for display, but are not paper marks.
        asset.last_price_usd = None;
        asset.last_price_at = None;
        views.push(
            portfolios::asset_view_on(&mut tx, asset, &ops, &strategy.currency, today, quote)
                .await?,
        );
    }
    let cash = cash_balance_on(&mut tx, strategy).await?;
    tx.commit().await.context("commit paper context")?;
    Ok((views, cash, fx_available))
}

async fn cash_balance_on(
    conn: &mut sqlx::PgConnection,
    strategy: &PaperStrategy,
) -> anyhow::Result<f64> {
    let rows: Vec<(String, Date, f64, f64, f64, String)> = sqlx::query_as(
        "SELECT o.side, o.op_date, o.quantity, o.price, o.fee, a.currency \
         FROM portfolio_operations o JOIN portfolio_assets a ON a.id = o.asset_id \
         WHERE o.portfolio_id = $1 ORDER BY o.op_date, o.created_at, o.id",
    )
    .bind(strategy.portfolio_id)
    .fetch_all(&mut *conn)
    .await
    .context("loading paper cash flows")?;

    let mut cash = strategy.initial_cash;
    let mut fx = journal_fx::FxCache::new();
    for (side, date, quantity, price, fee, currency) in rows {
        let native_flow = match side.as_str() {
            "buy" => -(quantity * price + fee),
            "sell" => quantity * price - fee,
            _ => continue,
        };
        let converted = fx
            .convert_on(conn, native_flow, &currency, &strategy.currency, date)
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "missing FX rate for {currency} to {} on {date}",
                    strategy.currency
                )
            })?;
        cash += converted;
    }
    Ok(cash)
}

#[derive(Debug)]
pub enum OrderError {
    NotFound,
    Invalid(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for OrderError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

fn fee_for_order(notional: f64, rate_bps: f64, min_fee: f64) -> f64 {
    (notional * rate_bps / 10_000.0).max(min_fee)
}

fn operation_date(provider: &str, now: OffsetDateTime) -> Date {
    if provider == "china" {
        now.to_offset(time::UtcOffset::from_hms(8, 0, 0).expect("China UTC offset"))
            .date()
    } else {
        now.date()
    }
}

/// China A-share stocks follow T+1: shares bought today are not sellable today. Same-day
/// sells reduce the quantity available from the position carried into today.
fn china_sellable_quantity(ops: &[portfolios::Operation], today: Date) -> f64 {
    let mut sellable = 0.0_f64;
    for op in ops {
        if op.op_date < today {
            if op.side == "buy" {
                sellable += op.quantity;
            }
            if op.side == "sell" {
                sellable -= op.quantity;
            }
        } else if op.op_date == today && op.side == "sell" {
            sellable -= op.quantity;
        }
    }
    sellable.max(0.0)
}

pub async fn execute_order(
    pool: &PgPool,
    user_id: Uuid,
    strategy_id: Uuid,
    run_id: Uuid,
    asset_id: Uuid,
    idempotency_key: &str,
    side: &str,
    quantity: f64,
    note: &str,
) -> Result<PaperOrderView, OrderError> {
    if !matches!(side, "buy" | "sell") {
        return Err(OrderError::Invalid("side must be buy or sell".into()));
    }
    if !quantity.is_finite() || quantity <= 0.0 {
        return Err(OrderError::Invalid(
            "quantity must be a positive finite number".into(),
        ));
    }
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 128 {
        return Err(OrderError::Invalid(
            "idempotency_key must be 1-128 characters".into(),
        ));
    }
    if note.len() > 1000 {
        return Err(OrderError::Invalid(
            "note is too long (max 1000 characters)".into(),
        ));
    }

    let mut tx = pool.begin().await.context("begin paper order")?;
    let strategy_sql = format!("SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE id = $1 AND user_id = $2 FOR UPDATE");
    let strategy = sqlx::query_as::<_, PaperStrategy>(sqlx::AssertSqlSafe(strategy_sql))
        .bind(strategy_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .context("locking paper strategy")?
        .ok_or(OrderError::NotFound)?;

    let op_note = if note.trim().is_empty() {
        format!("Paper Trading strategy {} run {}", strategy.name, run_id)
    } else {
        format!("Paper Trading: {}", note.trim())
    };

    if let Some(existing) = find_order_by_key_tx(&mut tx, strategy_id, idempotency_key).await? {
        if existing.run_id != run_id
            || existing.asset_id != asset_id
            || existing.side != side
            || existing.requested_quantity != quantity
            || existing.note != op_note
        {
            return Err(OrderError::Conflict(
                "idempotency key was already used for a different order".into(),
            ));
        }
        tx.commit().await.context("commit idempotent paper order")?;
        return Ok(existing);
    }

    if strategy.status != "active" {
        return Err(OrderError::Conflict("paper strategy is not active".into()));
    }
    let run_decision: Option<String> = sqlx::query_scalar(
        "SELECT decision FROM paper_trading_runs WHERE id = $1 AND strategy_id = $2",
    )
    .bind(run_id)
    .bind(strategy_id)
    .fetch_optional(&mut *tx)
    .await
    .context("checking paper run")?;
    let run_decision = run_decision.ok_or(OrderError::NotFound)?;
    let decision_matches = if side == "buy" {
        matches!(run_decision.as_str(), "buy" | "add")
    } else {
        matches!(run_decision.as_str(), "reduce" | "exit")
    };
    if !decision_matches {
        return Err(OrderError::Invalid(format!(
            "order side {side} does not match run decision {run_decision}"
        )));
    }

    // All ledger reads use this transaction's connection. Acquiring pool connections
    // while holding the strategy lock can exhaust the pool under concurrent orders.
    let portfolio_assets = portfolios::list_assets_on(&mut tx, strategy.portfolio_id).await?;
    let asset = portfolio_assets
        .iter()
        .find(|a| a.id == asset_id)
        .cloned()
        .ok_or(OrderError::NotFound)?;
    if asset.spot_provider.is_some() {
        return Err(OrderError::Invalid(
            "paper orders require the asset's default shared Market Quote source".into(),
        ));
    }

    let quote: Option<(f64, OffsetDateTime)> = sqlx::query_as(
        "SELECT price_usd, quote_at FROM market_quotes WHERE provider = $1 AND provider_id = $2",
    )
    .bind(&asset.provider)
    .bind(&asset.provider_id)
    .fetch_optional(&mut *tx)
    .await
    .context("loading shared market quote")?;
    let (quote_price_usd, quote_at) =
        quote.ok_or_else(|| OrderError::Conflict("no shared market quote is available".into()))?;
    let now = OffsetDateTime::now_utc();
    let quote_age = (now - quote_at).whole_seconds();
    if quote_age > 30 || quote_age < -5 {
        return Err(OrderError::Conflict(
            "market quote is stale; refresh the strategy context and retry".into(),
        ));
    }

    let today = operation_date(&asset.provider, now);
    if today < strategy.start_date {
        return Err(OrderError::Conflict(
            "paper strategy start date is in the future".into(),
        ));
    }
    let mut fx = journal_fx::FxCache::new();
    let quote_native = fx
        .convert_on(&mut tx, quote_price_usd, "USD", &asset.currency, today)
        .await?
        .ok_or_else(|| {
            OrderError::Invalid(format!("missing FX rate from USD to {}", asset.currency))
        })?;
    let slippage = strategy.slippage_bps / 10_000.0;
    let fill_price = if side == "buy" {
        quote_native * (1.0 + slippage)
    } else {
        quote_native * (1.0 - slippage)
    };
    if !fill_price.is_finite() || fill_price <= 0.0 {
        return Err(OrderError::Invalid(
            "computed fill price is not positive".into(),
        ));
    }
    let notional_native = quantity * fill_price;
    let commission = fee_for_order(notional_native, strategy.fee_rate_bps, strategy.min_fee);
    // The extra sell-side levy (e.g. A-share stamp duty) has no minimum commission.
    let sell_levy = if side == "sell" {
        notional_native * strategy.sell_extra_fee_rate_bps / 10_000.0
    } else {
        0.0
    };
    let fee_native = commission + sell_levy;

    let notional_account = fx
        .convert_on(
            &mut tx,
            notional_native,
            &asset.currency,
            &strategy.currency,
            today,
        )
        .await?
        .ok_or_else(|| {
            OrderError::Invalid(format!(
                "missing FX rate from {} to {}",
                asset.currency, strategy.currency
            ))
        })?;
    let fee_account = fx
        .convert_on(
            &mut tx,
            fee_native,
            &asset.currency,
            &strategy.currency,
            today,
        )
        .await?
        .ok_or_else(|| {
            OrderError::Invalid(format!(
                "missing FX rate from {} to {}",
                asset.currency, strategy.currency
            ))
        })?;
    if !notional_account.is_finite()
        || notional_account <= 0.0
        || !fee_account.is_finite()
        || fee_account < 0.0
    {
        return Err(OrderError::Invalid(
            "computed order amounts must be finite and positive".into(),
        ));
    }
    if strategy.max_order_value > 0.0 && notional_account > strategy.max_order_value {
        return Err(OrderError::Conflict(format!(
            "order value {:.2} {} exceeds the configured per-order limit {:.2} {}",
            notional_account, strategy.currency, strategy.max_order_value, strategy.currency
        )));
    }

    let account_fx_available = strategy.currency == "USD"
        || fx
            .convert_on(&mut tx, 1.0, "USD", &strategy.currency, today)
            .await?
            .is_some_and(|rate| rate.is_finite() && rate > 0.0);
    if !account_fx_available {
        return Err(OrderError::Conflict(format!(
            "missing USD/{} FX rate; cannot enforce account risk limits",
            strategy.currency
        )));
    }
    let cash = cash_balance_on(&mut tx, &strategy).await?;
    if !cash.is_finite() {
        return Err(OrderError::Conflict("paper cash is not finite".into()));
    }
    let mut views = Vec::new();
    let mut operations_by_asset = HashMap::new();
    for holding in portfolio_assets {
        let operations = portfolios::list_operations_on(&mut tx, holding.id).await?;
        // The target uses the exact quote used to price the fill. Other holdings
        // must also have fresh shared quotes; local Portfolio fallback is display-only.
        let current_quote: Option<(f64, OffsetDateTime)> = if holding.id == asset_id {
            Some((quote_price_usd, quote_at))
        } else {
            sqlx::query_as("SELECT price_usd, quote_at FROM market_quotes WHERE provider = $1 AND provider_id = $2")
                .bind(&holding.provider).bind(&holding.provider_id)
                .fetch_optional(&mut *tx).await.context("loading holding quote")?
        };
        let fresh_quote = current_quote.filter(|(price, at)| {
            price.is_finite()
                && *price > 0.0
                && (-5..=30).contains(&(now - *at).whole_seconds())
                && holding.spot_provider.is_none()
        });
        let mut view = portfolios::asset_view_on(
            &mut tx,
            holding,
            &operations,
            &strategy.currency,
            today,
            fresh_quote,
        )
        .await?;
        if fresh_quote.is_none() {
            view.market_value = None;
        }
        operations_by_asset.insert(view.asset.id, operations);
        views.push(view);
    }
    let mut market_value = 0.0_f64;
    let mut market_value_complete = true;
    let mut target_quantity = 0.0_f64;
    let mut target_market_value = 0.0_f64;
    for view in &views {
        if view.quantity > 0.0 {
            match view
                .market_value
                .filter(|value| value.is_finite() && *value >= 0.0)
            {
                Some(value) => market_value += value,
                None => market_value_complete = false,
            }
        }
        if view.asset.id == asset_id {
            target_quantity = view.quantity;
            target_market_value = view.market_value.unwrap_or(0.0);
        }
    }

    if side == "buy" && run_decision == "buy" && target_quantity > 1e-9 {
        return Err(OrderError::Invalid(
            "buy decision requires no existing position; use add".into(),
        ));
    }
    if side == "buy" && run_decision == "add" && target_quantity <= 1e-9 {
        return Err(OrderError::Invalid(
            "add decision requires an existing position; use buy".into(),
        ));
    }
    if side == "sell" {
        // Until Market provides authoritative settlement metadata, all China
        // instruments use conservative T+1. Client asset_class must not disable it.
        let sellable = if asset.provider == "china" {
            china_sellable_quantity(&operations_by_asset[&asset_id], today)
        } else {
            target_quantity
        };
        if quantity > sellable {
            return Err(OrderError::Conflict(format!(
                "cannot sell {:.4}; currently sellable paper position is {:.4}",
                quantity, sellable
            )));
        }
        if run_decision == "reduce" && quantity >= sellable - 1e-9 {
            return Err(OrderError::Invalid(
                "reduce must leave a remaining sellable position; use exit to close it".into(),
            ));
        }
        if run_decision == "exit" && (sellable <= 1e-9 || (quantity - sellable).abs() > 1e-9) {
            return Err(OrderError::Invalid(
                "exit must sell the full currently sellable position".into(),
            ));
        }
    }
    if side == "buy" {
        if cash + 1e-8 < notional_account + fee_account {
            return Err(OrderError::Conflict(format!(
                "insufficient paper cash: available {:.2} {}, required {:.2} {}",
                cash,
                strategy.currency,
                notional_account + fee_account,
                strategy.currency
            )));
        }
        if !market_value_complete {
            return Err(OrderError::Conflict("cannot enforce position limits while an existing holding has no current market value".into()));
        }
        let equity = cash + market_value;
        let projected_equity = equity - fee_account;
        if !projected_equity.is_finite() || projected_equity <= 0.0 {
            return Err(OrderError::Conflict(
                "projected paper account equity is not positive".into(),
            ));
        }
        let projected_weight = (target_market_value + notional_account) / projected_equity;
        if projected_weight > strategy.max_position_pct + 1e-9 {
            return Err(OrderError::Conflict(format!(
                "projected position weight {:.1}% exceeds configured limit {:.1}%",
                projected_weight * 100.0,
                strategy.max_position_pct * 100.0
            )));
        }
    }

    if side == "sell" && cash + notional_account < fee_account {
        return Err(OrderError::Conflict(
            "sale proceeds and available cash cannot cover fees".into(),
        ));
    }

    let operation_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO portfolio_operations \
         (id, asset_id, portfolio_id, side, op_date, quantity, price, fee, note) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(operation_id)
    .bind(asset_id)
    .bind(strategy.portfolio_id)
    .bind(side)
    .bind(today)
    .bind(quantity)
    .bind(fill_price)
    .bind(fee_native)
    .bind(op_note)
    .execute(&mut *tx)
    .await
    .context("recording paper fill in Portfolio ledger")?;

    sqlx::query(
        "INSERT INTO paper_trading_orders \
         (id, strategy_id, run_id, asset_id, operation_id, idempotency_key, side, requested_quantity, quote_price_usd, quote_at, slippage_bps) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(order_id)
    .bind(strategy_id)
    .bind(run_id)
    .bind(asset_id)
    .bind(operation_id)
    .bind(idempotency_key)
    .bind(side)
    .bind(quantity)
    .bind(quote_price_usd)
    .bind(quote_at)
    .bind(strategy.slippage_bps)
    .execute(&mut *tx)
    .await
    .context("recording paper order audit row")?;

    let order = find_order_by_id_tx(&mut tx, order_id)
        .await?
        .ok_or_else(|| anyhow!("paper order disappeared after insert"))?;
    tx.commit().await.context("commit paper order")?;
    Ok(order)
}

async fn find_order_by_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    strategy_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<PaperOrderView>> {
    let sql = format!("{ORDER_VIEW_SELECT} WHERE po.strategy_id = $1 AND po.idempotency_key = $2");
    Ok(
        sqlx::query_as::<_, PaperOrderView>(sqlx::AssertSqlSafe(sql))
            .bind(strategy_id)
            .bind(key)
            .fetch_optional(&mut **tx)
            .await
            .context("finding idempotent paper order")?,
    )
}

async fn find_order_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_id: Uuid,
) -> anyhow::Result<Option<PaperOrderView>> {
    let sql = format!("{ORDER_VIEW_SELECT} WHERE po.id = $1");
    Ok(
        sqlx::query_as::<_, PaperOrderView>(sqlx::AssertSqlSafe(sql))
            .bind(order_id)
            .fetch_optional(&mut **tx)
            .await
            .context("loading paper order")?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn china_operation_date_uses_market_midnight() {
        let at = time::macros::datetime!(2026-11-01 16:01 UTC);
        assert_eq!(
            operation_date("china", at),
            time::macros::date!(2026 - 11 - 02)
        );
        assert_eq!(
            operation_date("yahoo", at),
            time::macros::date!(2026 - 11 - 01)
        );
    }

    #[test]
    fn commission_uses_percentage_or_minimum_whichever_is_higher() {
        assert!((fee_for_order(10_000.0, 3.0, 5.0) - 5.0).abs() < 1e-9);
        assert!((fee_for_order(100_000.0, 3.0, 5.0) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn sell_levy_is_added_after_minimum_commission() {
        let commission = fee_for_order(10_000.0, 3.0, 5.0);
        let stamp_duty = 10_000.0 * 5.0 / 10_000.0;
        assert!((commission + stamp_duty - 10.0).abs() < 1e-9);
    }

    #[test]
    fn china_t_plus_one_excludes_same_day_buys_from_sellable_quantity() {
        let today = Date::from_calendar_date(2026, time::Month::November, 1).unwrap();
        let op = |side: &str, date: Date, quantity: f64| portfolios::Operation {
            id: Uuid::new_v4(),
            asset_id: Uuid::nil(),
            side: side.into(),
            op_date: date,
            quantity,
            price: 1.0,
            fee: 0.0,
            note: String::new(),
            created_at: OffsetDateTime::now_utc(),
        };
        let ops = vec![
            op("buy", today.previous_day().unwrap(), 500.0),
            op("sell", today, 100.0),
            op("buy", today, 200.0),
        ];
        assert!((china_sellable_quantity(&ops, today) - 400.0).abs() < 1e-9);
    }
}


// ── Day-level AI audit and paper performance (historical snapshots) ──────────
// A run snapshot is captured BEFORE any order belonging to that run. Consequently
// daily equity deltas are only indicative, not official EOD/mark-to-market P&L.
// Actual fills remain exclusively in portfolio_operations.

pub async fn list_day_summaries(
    pool: &PgPool, strategy_id: Uuid, initial_cash: f64, limit: i64, offset: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let dates = "
        FROM (
            SELECT (scheduled_at AT TIME ZONE 'Asia/Shanghai')::date AS day
            FROM paper_trading_runs WHERE strategy_id = $1
            UNION
            SELECT (created_at AT TIME ZONE 'Asia/Shanghai')::date AS day
            FROM paper_trading_orders WHERE strategy_id = $1
        ) days";
    let count_sql = format!("SELECT COUNT(*) {dates}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql))
        .bind(strategy_id).fetch_one(pool).await?;
    let sql = format!(
        "SELECT days.day,
            (SELECT COUNT(*) FROM paper_trading_runs r WHERE r.strategy_id = $1
              AND (r.scheduled_at AT TIME ZONE 'Asia/Shanghai')::date = days.day),
            (SELECT COUNT(*) FROM paper_trading_orders o WHERE o.strategy_id = $1
              AND (o.created_at AT TIME ZONE 'Asia/Shanghai')::date = days.day),
            (SELECT COALESCE(jsonb_agg(jsonb_build_object(
                 'side', o.side, 'symbol', a.symbol, 'quantity', op.quantity
               ) ORDER BY o.created_at), '[]'::jsonb)
               FROM paper_trading_orders o
               JOIN portfolio_operations op ON op.id = o.operation_id
               JOIN portfolio_assets a ON a.id = op.asset_id
               WHERE o.strategy_id = $1
                 AND (o.created_at AT TIME ZONE 'Asia/Shanghai')::date = days.day),
            (SELECT (r.account_snapshot->>'equity')::double precision
               FROM paper_trading_runs r WHERE r.strategy_id = $1
                 AND (r.scheduled_at AT TIME ZONE 'Asia/Shanghai')::date = days.day
                 AND jsonb_typeof(r.account_snapshot->'equity') = 'number'
               ORDER BY r.scheduled_at DESC, r.created_at DESC LIMIT 1),
            (SELECT (r.account_snapshot->>'equity')::double precision
               FROM paper_trading_runs r WHERE r.strategy_id = $1
                 AND (r.scheduled_at AT TIME ZONE 'Asia/Shanghai')::date < days.day
                 AND jsonb_typeof(r.account_snapshot->'equity') = 'number'
               ORDER BY r.scheduled_at DESC, r.created_at DESC LIMIT 1)
          {dates} ORDER BY days.day DESC LIMIT $2 OFFSET $3"
    );
    let rows: Vec<(Date, i64, i64, Value, Option<f64>, Option<f64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(strategy_id).bind(limit.clamp(1, 31)).bind(offset.max(0))
            .fetch_all(pool).await.context("listing paper daily summaries")?;
    let days = rows.into_iter().map(|(date, run_count, order_count, trades, equity, previous_equity)| {
        // Null means there is no complete valuation snapshot; never fabricate a return.
        serde_json::json!({
            "date": date.to_string(),
            "run_count": run_count,
            "order_count": order_count,
            "trades": trades,
            "equity": equity,
            "previous_equity": previous_equity,
            "estimated_pnl": equity.map(|value| value - previous_equity.unwrap_or(initial_cash)),
            "valuation": "run_snapshot_pre_order"
        })
    }).collect();
    Ok((total, days))
}

pub async fn list_runs_for_day(
    pool: &PgPool, strategy_id: Uuid, day: Date, limit: i64, offset: i64,
) -> anyhow::Result<Vec<PaperRun>> {
    sqlx::query_as(
        "SELECT * FROM paper_trading_runs WHERE strategy_id = $1
         AND (scheduled_at AT TIME ZONE 'Asia/Shanghai')::date = $2
         ORDER BY scheduled_at DESC, created_at DESC LIMIT $3 OFFSET $4"
    )
    .bind(strategy_id).bind(day).bind(limit.clamp(1, 200)).bind(offset.max(0))
    .fetch_all(pool).await.context("listing paper runs for day")
}

pub async fn list_orders_for_day(
    pool: &PgPool, strategy_id: Uuid, day: Date, limit: i64, offset: i64,
) -> anyhow::Result<Vec<PaperOrderView>> {
    let sql = format!(
        "{ORDER_VIEW_SELECT} WHERE po.strategy_id = $1
         AND (po.created_at AT TIME ZONE 'Asia/Shanghai')::date = $2
         ORDER BY po.created_at DESC LIMIT $3 OFFSET $4"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(strategy_id).bind(day).bind(limit.clamp(1, 200)).bind(offset.max(0))
        .fetch_all(pool).await.context("listing paper orders for day")
}
