//! Paper Trading persistence and safe execution boundary.
//!
//! Strategy metadata and AI audit records live here. All actual fills are written to
//! Portfolio Tracker's portfolio_operations ledger; no second position ledger exists.

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
    #[serde(with = "date_fmt")]
    pub op_date: Date,
    pub symbol: String,
}

mod date_fmt {
    use serde::Serializer;
    use time::{format_description::well_known::Iso8601, Date};
    pub fn serialize<S: Serializer>(d: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.format(&Iso8601::DATE).map_err(serde::ser::Error::custom)?)
    }
}

pub struct NewStrategy<'a> {
    pub user_id: Uuid,
    pub name: &'a str,
    pub ai_provider: &'a str,
    pub model: &'a str,
    pub prompt_version: &'a str,
    pub currency: &'a str,
    pub initial_cash: f64,
    pub max_position_pct: f64,
    pub max_order_value: f64,
    pub slippage_bps: f64,
    pub fee_rate_bps: f64,
    pub sell_extra_fee_rate_bps: f64,
    pub min_fee: f64,
}

pub async fn create_strategy(pool: &PgPool, input: NewStrategy<'_>) -> anyhow::Result<PaperStrategy> {
    let mut tx = pool.begin().await.context("begin paper strategy creation")?;
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
         (id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, initial_cash, \
          max_position_pct, max_order_value, slippage_bps, fee_rate_bps, sell_extra_fee_rate_bps, min_fee) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
         RETURNING id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, initial_cash, \
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

    tx.commit().await.context("commit paper strategy creation")?;
    Ok(strategy)
}

const STRATEGY_COLS: &str = "id, user_id, portfolio_id, name, ai_provider, model, prompt_version, currency, initial_cash, status, max_position_pct, max_order_value, slippage_bps, fee_rate_bps, sell_extra_fee_rate_bps, min_fee, created_at, updated_at";

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
    let sql = format!("SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE id = $1 AND user_id = $2");
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

pub async fn create_run(
    pool: &PgPool,
    strategy_id: Uuid,
    idempotency_key: &str,
    scheduled_at: OffsetDateTime,
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
    let id = Uuid::new_v4();
    let inserted = sqlx::query_as::<_, PaperRun>(
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
    .bind(scheduled_at)
    .bind(model)
    .bind(market_facts)
    .bind(market_expectations)
    .bind(inference)
    .bind(decision)
    .bind(confidence)
    .bind(evidence)
    .bind(risk_assessment)
    .bind(account_snapshot)
    .fetch_optional(pool)
    .await
    .context("creating paper run")?;

    if let Some(run) = inserted {
        return Ok(run);
    }
    let existing = sqlx::query_as::<_, PaperRun>(
        "SELECT id, strategy_id, idempotency_key, scheduled_at, model, market_facts, market_expectations, \
                inference, decision, confidence, evidence, risk_assessment, account_snapshot, created_at \
         FROM paper_trading_runs WHERE strategy_id = $1 AND idempotency_key = $2",
    )
    .bind(strategy_id)
    .bind(idempotency_key)
    .fetch_one(pool)
    .await
    .context("loading idempotent paper run")?;
    if existing.decision != decision
        || existing.confidence != confidence
        || existing.inference != inference
        || existing.model != model
        || existing.market_facts != *market_facts
        || existing.market_expectations != *market_expectations
        || existing.evidence != *evidence
        || existing.risk_assessment != *risk_assessment
    {
        return Err(anyhow!("idempotency key already used for a different paper run"));
    }
    Ok(existing)
}

pub async fn list_runs(pool: &PgPool, strategy_id: Uuid, limit: i64) -> anyhow::Result<Vec<PaperRun>> {
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

const ORDER_VIEW_SELECT: &str = "SELECT po.id, po.strategy_id, po.run_id, po.asset_id, po.idempotency_key, po.side, po.requested_quantity, po.quote_price_usd, po.quote_at, po.slippage_bps, po.operation_id, po.created_at, op.price AS fill_price, op.fee, a.currency AS fee_currency, op.op_date, a.symbol FROM paper_trading_orders po JOIN portfolio_operations op ON op.id = po.operation_id JOIN portfolio_assets a ON a.id = po.asset_id";

pub async fn list_orders(pool: &PgPool, strategy_id: Uuid, limit: i64) -> anyhow::Result<Vec<PaperOrderView>> {
    let sql = format!("{ORDER_VIEW_SELECT} WHERE po.strategy_id = $1 ORDER BY po.created_at DESC LIMIT $2");
    Ok(sqlx::query_as::<_, PaperOrderView>(sqlx::AssertSqlSafe(sql))
        .bind(strategy_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await
        .context("listing paper orders")?)
}

pub async fn cash_balance(pool: &PgPool, strategy: &PaperStrategy) -> anyhow::Result<f64> {
    let rows: Vec<(String, Date, f64, f64, f64, String)> = sqlx::query_as(
        "SELECT o.side, o.op_date, o.quantity, o.price, o.fee, a.currency \
         FROM portfolio_operations o JOIN portfolio_assets a ON a.id = o.asset_id \
         WHERE o.portfolio_id = $1 ORDER BY o.op_date, o.created_at, o.id",
    )
    .bind(strategy.portfolio_id)
    .fetch_all(pool)
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
            .convert(pool, native_flow, &currency, &strategy.currency, date)
            .await?
            .ok_or_else(|| anyhow!("missing FX rate for {currency} to {} on {date}", strategy.currency))?;
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

/// China A-share stocks follow T+1: shares bought today are not sellable today. Same-day
/// sells reduce the quantity available from the position carried into today.
fn china_sellable_quantity(ops: &[portfolios::Operation], today: Date) -> f64 {
    let mut sellable = 0.0_f64;
    for op in ops {
        if op.op_date < today {
            if op.side == "buy" { sellable += op.quantity; }
            if op.side == "sell" { sellable -= op.quantity; }
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
        return Err(OrderError::Invalid("quantity must be a positive finite number".into()));
    }
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 128 {
        return Err(OrderError::Invalid("idempotency_key must be 1-128 characters".into()));
    }
    if note.len() > 1000 {
        return Err(OrderError::Invalid("note is too long (max 1000 characters)".into()));
    }

    let mut tx = pool.begin().await.context("begin paper order")?;
    let strategy_sql = format!("SELECT {STRATEGY_COLS} FROM paper_trading_strategies WHERE id = $1 AND user_id = $2 FOR UPDATE");
    let strategy = sqlx::query_as::<_, PaperStrategy>(&strategy_sql)
        .bind(strategy_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .context("locking paper strategy")?
        .ok_or(OrderError::NotFound)?;

    if let Some(existing) = find_order_by_key_tx(&mut tx, strategy_id, idempotency_key).await? {
        if existing.run_id != run_id
            || existing.asset_id != asset_id
            || existing.side != side
            || (existing.requested_quantity - quantity).abs() > 1e-9
        {
            return Err(OrderError::Conflict("idempotency key was already used for a different order".into()));
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
        return Err(OrderError::Invalid(format!("order side {side} does not match run decision {run_decision}")));
    }

    let asset = portfolios::get_asset(pool, asset_id).await?.ok_or(OrderError::NotFound)?;
    if asset.portfolio_id != strategy.portfolio_id {
        return Err(OrderError::NotFound);
    }
    if asset.spot_provider.is_some() {
        return Err(OrderError::Invalid("paper orders require the asset's default shared Market Quote source".into()));
    }

    let quote: Option<(f64, OffsetDateTime)> = sqlx::query_as(
        "SELECT price_usd, quote_at FROM market_quotes WHERE provider = $1 AND provider_id = $2",
    )
    .bind(&asset.provider)
    .bind(&asset.provider_id)
    .fetch_optional(&mut *tx)
    .await
    .context("loading shared market quote")?;
    let (quote_price_usd, quote_at) = quote.ok_or_else(|| OrderError::Conflict("no shared market quote is available".into()))?;
    let now = OffsetDateTime::now_utc();
    let quote_age = (now - quote_at).whole_seconds();
    if quote_age > 30 || quote_age < -5 {
        return Err(OrderError::Conflict("market quote is stale; refresh the strategy context and retry".into()));
    }

    let today = now.date();
    let quote_native = journal_fx::convert(pool, quote_price_usd, "USD", &asset.currency, today)
        .await?
        .ok_or_else(|| OrderError::Invalid(format!("missing FX rate from USD to {}", asset.currency)))?;
    let slippage = strategy.slippage_bps / 10_000.0;
    let fill_price = if side == "buy" { quote_native * (1.0 + slippage) } else { quote_native * (1.0 - slippage) };
    if !fill_price.is_finite() || fill_price <= 0.0 {
        return Err(OrderError::Invalid("computed fill price is not positive".into()));
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

    let notional_account = journal_fx::convert(pool, notional_native, &asset.currency, &strategy.currency, today)
        .await?
        .ok_or_else(|| OrderError::Invalid(format!("missing FX rate from {} to {}", asset.currency, strategy.currency)))?;
    let fee_account = journal_fx::convert(pool, fee_native, &asset.currency, &strategy.currency, today)
        .await?
        .ok_or_else(|| OrderError::Invalid(format!("missing FX rate from {} to {}", asset.currency, strategy.currency)))?;
    if strategy.max_order_value > 0.0 && notional_account > strategy.max_order_value {
        return Err(OrderError::Conflict(format!(
            "order value {:.2} {} exceeds the configured per-order limit {:.2} {}",
            notional_account, strategy.currency, strategy.max_order_value, strategy.currency
        )));
    }

    let cash = cash_balance(pool, &strategy).await?;
    let portfolio = portfolios::get_portfolio(pool, strategy.portfolio_id)
        .await?
        .ok_or_else(|| anyhow!("paper portfolio is missing"))?;
    let views = portfolios::asset_views(pool, &portfolio).await?;
    let mut market_value = 0.0_f64;
    let mut market_value_complete = true;
    let mut target_quantity = 0.0_f64;
    let mut target_market_value = 0.0_f64;
    for view in &views {
        if view.quantity > 0.0 {
            match view.market_value {
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
        return Err(OrderError::Invalid("buy decision requires no existing position; use add".into()));
    }
    if side == "buy" && run_decision == "add" && target_quantity <= 1e-9 {
        return Err(OrderError::Invalid("add decision requires an existing position; use buy".into()));
    }
    if side == "sell" {
        let sellable = if asset.provider == "china" && asset.asset_class == "stock" {
            let operations = portfolios::list_operations(pool, asset_id).await?;
            china_sellable_quantity(&operations, today)
        } else {
            target_quantity
        };
        if quantity > sellable + 1e-9 {
            return Err(OrderError::Conflict(format!(
                "cannot sell {:.4}; currently sellable paper position is {:.4}", quantity, sellable
            )));
        }
        if run_decision == "reduce" && quantity >= sellable - 1e-9 {
            return Err(OrderError::Invalid("reduce must leave a remaining sellable position; use exit to close it".into()));
        }
        if run_decision == "exit" && (sellable <= 1e-9 || (quantity - sellable).abs() > 1e-9) {
            return Err(OrderError::Invalid("exit must sell the full currently sellable position".into()));
        }
    }
    if side == "buy" {
        if cash + 1e-8 < notional_account + fee_account {
            return Err(OrderError::Conflict(format!(
                "insufficient paper cash: available {:.2} {}, required {:.2} {}",
                cash, strategy.currency, notional_account + fee_account, strategy.currency
            )));
        }
        if !market_value_complete {
            return Err(OrderError::Conflict("cannot enforce position limits while an existing holding has no current market value".into()));
        }
        let equity = cash + market_value;
        if equity <= 0.0 {
            return Err(OrderError::Conflict("paper account equity is not positive".into()));
        }
        let projected_weight = (target_market_value + notional_account) / equity;
        if projected_weight > strategy.max_position_pct + 1e-9 {
            return Err(OrderError::Conflict(format!(
                "projected position weight {:.1}% exceeds configured limit {:.1}%",
                projected_weight * 100.0,
                strategy.max_position_pct * 100.0
            )));
        }
    }

    let operation_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();
    let op_note = if note.trim().is_empty() {
        format!("Paper Trading strategy {} run {}", strategy.name, run_id)
    } else {
        format!("Paper Trading: {}", note.trim())
    };
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

    let order = find_order_by_id_tx(&mut tx, order_id).await?
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
    Ok(sqlx::query_as::<_, PaperOrderView>(&sql)
        .bind(strategy_id)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await
        .context("finding idempotent paper order")?)
}

async fn find_order_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_id: Uuid,
) -> anyhow::Result<Option<PaperOrderView>> {
    let sql = format!("{ORDER_VIEW_SELECT} WHERE po.id = $1");
    Ok(sqlx::query_as::<_, PaperOrderView>(&sql)
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await
        .context("loading paper order")?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            id: Uuid::new_v4(), asset_id: Uuid::nil(), side: side.into(), op_date: date,
            quantity, price: 1.0, fee: 0.0, note: String::new(), created_at: OffsetDateTime::now_utc(),
        };
        let ops = vec![
            op("buy", today.previous_day().unwrap(), 500.0),
            op("sell", today, 100.0),
            op("buy", today, 200.0),
        ];
        assert!((china_sellable_quantity(&ops, today) - 400.0).abs() < 1e-9);
    }
}
