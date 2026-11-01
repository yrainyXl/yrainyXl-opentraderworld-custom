//! HTTP API for isolated AI Paper Trading.
//!
//! Strategy setup is a session-only action. MCP exposes only the paper-trading
//! module's read endpoints and the constrained run/asset/order writes.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use time::{format_description::well_known::{Iso8601, Rfc3339}, Date, OffsetDateTime};
use uuid::Uuid;

use crate::{market, portfolios::prices, ApiError, AppState};
use otw_store::{journal::CURRENCIES, paper_trading as store, portfolios};
use otw_store::User;

const DECISIONS: [&str; 7] = ["observe", "hold", "buy", "add", "reduce", "exit", "no_trade"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/paper-trading/strategies", get(list).post(create))
        .route("/api/paper-trading/search", get(search))
        .route("/api/paper-trading/strategies/{id}", get(detail).patch(update_strategy))
        .route("/api/paper-trading/strategies/{id}/context", get(context))
        .route("/api/paper-trading/strategies/{id}/assets", post(add_asset))
        .route("/api/paper-trading/strategies/{id}/runs", get(list_runs).post(add_run))
        .route("/api/paper-trading/strategies/{id}/orders", get(list_orders).post(place_order))
}

async fn owned_strategy(
    pool: &sqlx::PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<store::PaperStrategy, ApiError> {
    store::get_strategy_for_user(pool, id, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("paper strategy not found"))
}

async fn build_context(
    state: &AppState,
    strategy: &store::PaperStrategy,
) -> Result<(portfolios::Portfolio, Vec<portfolios::AssetView>, Value), ApiError> {
    let portfolio = portfolios::get_portfolio(&state.pool, strategy.portfolio_id)
        .await?
        .ok_or_else(|| ApiError::not_found("paper portfolio not found"))?;
    let assets = portfolios::list_assets(&state.pool, strategy.portfolio_id).await?;
    market::ensure_portfolio_assets(&state.pool, &assets).await?;
    let views = portfolios::asset_views(&state.pool, &portfolio).await?;
    let cash = store::cash_balance(&state.pool, strategy).await?;

    let mut market_value = 0.0_f64;
    let mut complete = true;
    let mut unpriced = Vec::new();
    for view in &views {
        if view.quantity > 0.0 {
            match view.market_value {
                Some(value) => market_value += value,
                None => {
                    complete = false;
                    unpriced.push(view.asset.symbol.clone());
                }
            }
        }
    }
    let equity = complete.then_some(cash + market_value);
    let snapshot = json!({
        "currency": strategy.currency,
        "start_date": strategy.start_date.to_string(),
        "initial_cash": strategy.initial_cash,
        "cash": cash,
        "market_value": market_value,
        "equity": equity,
        "market_value_complete": complete,
        "unpriced_holdings": unpriced,
        "holdings": views,
        "calculated_at": OffsetDateTime::now_utc().to_string(),
    });
    Ok((portfolio, views, snapshot))
}

async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Value>, ApiError> {
    let strategies = store::list_strategies(&state.pool, user.id).await?;
    Ok(Json(json!({ "strategies": strategies })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateStrategyBody {
    name: String,
    #[serde(default = "default_currency")]
    currency: String,
    /// Strategy accounting start date, YYYY-MM-DD. Useful when initializing a paper experiment retrospectively.
    start_date: Option<String>,
    initial_cash: f64,
    #[serde(default = "default_ai_provider")]
    ai_provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    prompt_version: String,
    /// Fraction from 0 to 1; 0.5 means a 50% maximum single-asset weight.
    #[serde(default = "default_max_position_pct")]
    max_position_pct: f64,
    /// Omit for 30% of initial cash; set 0 to disable the per-order notional cap.
    max_order_value: Option<f64>,
    /// Simulated adverse execution adjustment in basis points (5 = 0.05%).
    #[serde(default = "default_slippage_bps")]
    slippage_bps: f64,
    /// Commission rate in basis points, before the minimum commission.
    #[serde(default = "default_fee_rate_bps")]
    fee_rate_bps: f64,
    /// Additional sell-side levy in basis points; no minimum is applied to this component.
    #[serde(default = "default_sell_extra_fee_rate_bps")]
    sell_extra_fee_rate_bps: f64,
    /// Minimum commission in the asset's operation currency.
    #[serde(default = "default_min_fee")]
    min_fee: f64,
}

fn default_currency() -> String { "CNY".into() }
fn default_ai_provider() -> String { "chatgpt".into() }
fn default_max_position_pct() -> f64 { 0.50 }
fn default_slippage_bps() -> f64 { 5.0 }
fn default_fee_rate_bps() -> f64 { 3.0 }
fn default_sell_extra_fee_rate_bps() -> f64 { 5.0 }
fn default_min_fee() -> f64 { 5.0 }

async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateStrategyBody>,
) -> Result<Json<Value>, ApiError> {
    let name = body.name.trim();
    let currency = body.currency.trim().to_uppercase();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request("name must be 1-100 characters"));
    }
    if !CURRENCIES.contains(&currency.as_str()) {
        return Err(ApiError::bad_request("unsupported currency"));
    }
    if !body.initial_cash.is_finite() || body.initial_cash <= 0.0 {
        return Err(ApiError::bad_request("initial_cash must be positive"));
    }
    let start_date = match body.start_date.as_deref() {
        Some(raw) => Date::parse(raw, &Iso8601::DATE)
            .map_err(|_| ApiError::bad_request("start_date must be YYYY-MM-DD"))?,
        None => OffsetDateTime::now_utc().date(),
    };
    if !body.max_position_pct.is_finite() || body.max_position_pct <= 0.0 || body.max_position_pct > 1.0 {
        return Err(ApiError::bad_request("max_position_pct must be greater than 0 and at most 1"));
    }
    if body.max_order_value.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(ApiError::bad_request("max_order_value must be zero or positive"));
    }
    for (label, value) in [
        ("slippage_bps", body.slippage_bps),
        ("fee_rate_bps", body.fee_rate_bps),
        ("sell_extra_fee_rate_bps", body.sell_extra_fee_rate_bps),
    ] {
        if !value.is_finite() || !(0.0..=1000.0).contains(&value) {
            return Err(ApiError::bad_request(&format!("{label} must be between 0 and 1000")));
        }
    }
    if !body.min_fee.is_finite() || body.min_fee < 0.0 {
        return Err(ApiError::bad_request("min_fee must be zero or positive"));
    }

    let max_order_value = body.max_order_value.unwrap_or(body.initial_cash * 0.30);
    let strategy = store::create_strategy(&state.pool, store::NewStrategy {
        user_id: user.id,
        name,
        ai_provider: body.ai_provider.trim(),
        model: body.model.trim(),
        prompt_version: body.prompt_version.trim(),
        currency: &currency,
        start_date,
        initial_cash: body.initial_cash,
        max_position_pct: body.max_position_pct,
        max_order_value,
        slippage_bps: body.slippage_bps,
        fee_rate_bps: body.fee_rate_bps,
        sell_extra_fee_rate_bps: body.sell_extra_fee_rate_bps,
        min_fee: body.min_fee,
    }).await?;
    Ok(Json(json!({ "strategy": strategy })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateStrategyBody {
    /// active | paused | ended. Ended is terminal.
    status: String,
}

async fn update_strategy(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateStrategyBody>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    if !matches!(body.status.as_str(), "active" | "paused" | "ended") {
        return Err(ApiError::bad_request("status must be active, paused or ended"));
    }
    let strategy = store::set_strategy_status(&state.pool, id, user.id, &body.status)
        .await?
        .ok_or_else(|| ApiError::conflict("an ended strategy cannot be reactivated"))?;
    Ok(Json(json!({ "strategy": strategy })))
}

async fn detail(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let (portfolio, _views, account) = build_context(&state, &strategy).await?;
    Ok(Json(json!({ "strategy": strategy, "portfolio": portfolio, "account": account })))
}

async fn context(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let (portfolio, _views, account) = build_context(&state, &strategy).await?;
    Ok(Json(json!({ "strategy": strategy, "portfolio": portfolio, "account": account })))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    kind: String,
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let hits = match query.kind.as_str() {
        "crypto" => prices::search_crypto(&state.pool, &query.q).await?,
        "stock" => prices::search_stock(&query.q, "stock").await?,
        _ => return Err(ApiError::bad_request("kind must be crypto or stock")),
    };
    Ok(Json(json!({ "results": hits })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct AddAssetBody {
    asset_class: String,
    provider: String,
    provider_id: String,
    symbol: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    currency: Option<String>,
}

async fn add_asset(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<AddAssetBody>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    if strategy.status == "ended" {
        return Err(ApiError::conflict("cannot add assets to an ended strategy"));
    }
    if !matches!(body.asset_class.as_str(), "crypto" | "stock" | "etf") {
        return Err(ApiError::bad_request("asset_class must be crypto, stock or etf"));
    }
    if !matches!(body.provider.as_str(), "coingecko" | "yahoo" | "china") {
        return Err(ApiError::bad_request("provider must be coingecko, yahoo or china"));
    }
    let class_provider_matches = match body.provider.as_str() {
        "coingecko" => body.asset_class == "crypto",
        "yahoo" => matches!(body.asset_class.as_str(), "stock" | "etf"),
        "china" => matches!(body.asset_class.as_str(), "stock" | "etf"),
        _ => false,
    };
    if !class_provider_matches {
        return Err(ApiError::bad_request("asset_class does not match provider"));
    }
    if body.provider_id.trim().is_empty() || body.symbol.trim().is_empty() {
        return Err(ApiError::bad_request("provider_id and symbol are required"));
    }
    let currency = body.currency.as_deref().map(str::trim).filter(|s| !s.is_empty())
        .unwrap_or(if body.provider == "china" { "CNY" } else { &strategy.currency });
    if !CURRENCIES.contains(&currency) {
        return Err(ApiError::bad_request("unsupported asset currency"));
    }
    let asset = portfolios::add_asset(
        &state.pool,
        strategy.portfolio_id,
        body.asset_class.trim(),
        body.provider.trim(),
        body.provider_id.trim(),
        body.symbol.trim(),
        body.name.trim(),
        currency,
    ).await?;
    // The quote gate uses the same provider integration as Watchlist/Portfolio.
    if let Err(error) = market::ensure_identity(&state.pool, &asset.provider, &asset.provider_id).await {
        tracing::warn!("initial paper quote for {} failed: {error:#}", asset.symbol);
    }
    let asset = portfolios::get_asset(&state.pool, asset.id).await?.unwrap_or(asset);
    Ok(Json(json!({ "asset": asset })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateRunBody {
    /// Stable key for this scheduled analysis cycle; retries must reuse it.
    idempotency_key: String,
    /// Optional RFC3339 scheduled time; omitted means server receipt time.
    scheduled_at: Option<String>,
    #[serde(default)]
    market_facts: Value,
    #[serde(default)]
    market_expectations: Value,
    #[serde(default)]
    inference: String,
    decision: String,
    confidence: i16,
    #[serde(default)]
    evidence: Value,
    #[serde(default)]
    risk_assessment: Value,
}

async fn add_run(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateRunBody>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    if strategy.status == "ended" {
        return Err(ApiError::conflict("cannot record new runs for an ended strategy"));
    }
    let key = body.idempotency_key.trim();
    if key.is_empty() || key.len() > 128 {
        return Err(ApiError::bad_request("idempotency_key must be 1-128 characters"));
    }
    if !DECISIONS.contains(&body.decision.as_str()) {
        return Err(ApiError::bad_request("invalid decision"));
    }
    if !(1..=5).contains(&body.confidence) {
        return Err(ApiError::bad_request("confidence must be between 1 and 5"));
    }
    let scheduled_at = match body.scheduled_at.as_deref() {
        Some(raw) => OffsetDateTime::parse(raw, &Rfc3339)
            .map_err(|_| ApiError::bad_request("scheduled_at must be RFC3339"))?,
        None => OffsetDateTime::now_utc(),
    };
    let (_portfolio, _views, snapshot) = build_context(&state, &strategy).await?;
    let run = store::create_run(
        &state.pool,
        strategy.id,
        key,
        scheduled_at,
        &strategy.model,
        &body.market_facts,
        &body.market_expectations,
        body.inference.trim(),
        body.decision.trim(),
        body.confidence,
        &body.evidence,
        &body.risk_assessment,
        &snapshot,
    ).await.map_err(|error| {
        if error.to_string().contains("idempotency key already used") {
            ApiError::conflict("idempotency key already used for a different paper run")
        } else {
            tracing::error!("creating paper run failed: {error:#}");
            ApiError::internal("could not create paper run")
        }
    })?;
    Ok(Json(json!({ "run": run })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PlaceOrderBody {
    /// Stable key for this simulated order; retries must reuse it.
    idempotency_key: String,
    /// The recorded AI run that authorized the order direction.
    run_id: Uuid,
    /// Asset id from this strategy's own paper portfolio.
    asset_id: Uuid,
    /// buy or sell; no client-supplied price is accepted.
    side: String,
    /// Requested asset units.
    quantity: f64,
    #[serde(default)]
    note: String,
}

async fn place_order(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<PlaceOrderBody>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let assets = portfolios::list_assets(&state.pool, strategy.portfolio_id).await?;
    market::ensure_portfolio_assets(&state.pool, &assets).await?;
    let order = store::execute_order(
        &state.pool,
        user.id,
        strategy.id,
        body.run_id,
        body.asset_id,
        body.idempotency_key.trim(),
        body.side.trim(),
        body.quantity,
        body.note.trim(),
    ).await.map_err(|error| match error {
        store::OrderError::NotFound => ApiError::not_found("paper strategy, run or asset not found"),
        store::OrderError::Invalid(message) => ApiError::bad_request(&message),
        store::OrderError::Conflict(message) => ApiError::conflict(&message),
        store::OrderError::Internal(error) => {
            tracing::error!("paper order execution failed: {error:#}");
            ApiError::internal("paper order execution failed")
        }
    })?;
    Ok(Json(json!({ "order": order })))
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<i64>,
}

async fn list_runs(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let runs = store::list_runs(&state.pool, id, query.limit.unwrap_or(50)).await?;
    Ok(Json(json!({ "runs": runs })))
}

async fn list_orders(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let orders = store::list_orders(&state.pool, id, query.limit.unwrap_or(50)).await?;
    Ok(Json(json!({ "orders": orders })))
}
