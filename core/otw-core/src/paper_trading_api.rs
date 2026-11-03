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
use time::{
    format_description::well_known::{Iso8601, Rfc3339},
    Date, OffsetDateTime,
};
use uuid::Uuid;

use crate::{market, portfolios::prices, ApiError, AppState};
use otw_store::User;
use otw_store::{journal::CURRENCIES, paper_trading as store, paper_documents as paper_docs, portfolios};

const DECISIONS: [&str; 7] = [
    "observe", "hold", "buy", "add", "reduce", "exit", "no_trade",
];

fn empty_object() -> Value {
    json!({})
}
fn empty_array() -> Value {
    json!([])
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/paper-trading/strategies", get(list).post(create))
        .route("/api/paper-trading/search", get(search))
        .route(
            "/api/paper-trading/strategies/{id}",
            get(detail).patch(update_strategy),
        )
        .route("/api/paper-trading/strategies/{id}/context", get(context))
        .route("/api/paper-trading/strategies/{id}/assets", post(add_asset))
        .route(
            "/api/paper-trading/strategies/{id}/runs",
            get(list_runs).post(add_run),
        )
        .route(
            "/api/paper-trading/strategies/{id}/orders",
            get(list_orders).post(place_order),
        )
        .route("/api/paper-trading/strategies/{id}/days", get(list_days))
        .route("/api/paper-trading/strategies/{id}/days/{date}", get(day_detail))
        .route("/api/paper-trading/strategies/{id}/documents", get(list_documents))
        .route("/api/paper-trading/strategies/{id}/documents/{asset_id}",
            get(get_document).post(ensure_document).put(update_document))
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
    let (views, cash, account_fx_available) = store::account_values(&state.pool, strategy).await?;

    let now = OffsetDateTime::now_utc();
    let mut market_value = 0.0_f64;
    let mut complete = account_fx_available;
    let mut unpriced = if account_fx_available {
        Vec::new()
    } else {
        vec![format!("FX: USD/{} unavailable", strategy.currency)]
    };
    for view in &views {
        if view.quantity > 0.0 {
            let quote_fresh = view.asset.spot_provider.is_none()
                && view
                    .asset
                    .last_price_at
                    .is_some_and(|at| (-5..=30).contains(&(now - at).whole_seconds()));
            match view
                .market_value
                .filter(|value| quote_fresh && value.is_finite() && *value >= 0.0)
            {
                Some(value) => market_value += value,
                None => {
                    complete = false;
                    unpriced.push(view.asset.symbol.clone());
                }
            }
        }
    }
    complete = complete && cash.is_finite() && (cash + market_value).is_finite();
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
        "calculated_at": now.to_string(),
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
#[serde(deny_unknown_fields)]
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

fn default_currency() -> String {
    "CNY".into()
}
fn default_ai_provider() -> String {
    "chatgpt".into()
}
fn default_max_position_pct() -> f64 {
    0.50
}
fn default_slippage_bps() -> f64 {
    5.0
}
fn default_fee_rate_bps() -> f64 {
    3.0
}
fn default_sell_extra_fee_rate_bps() -> f64 {
    5.0
}
fn default_min_fee() -> f64 {
    5.0
}

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
    if !body.max_position_pct.is_finite()
        || body.max_position_pct <= 0.0
        || body.max_position_pct > 1.0
    {
        return Err(ApiError::bad_request(
            "max_position_pct must be greater than 0 and at most 1",
        ));
    }
    if body
        .max_order_value
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(ApiError::bad_request(
            "max_order_value must be zero or positive",
        ));
    }
    for (label, value) in [
        ("slippage_bps", body.slippage_bps),
        ("fee_rate_bps", body.fee_rate_bps),
        ("sell_extra_fee_rate_bps", body.sell_extra_fee_rate_bps),
    ] {
        if !value.is_finite() || !(0.0..=1000.0).contains(&value) {
            return Err(ApiError::bad_request(&format!(
                "{label} must be between 0 and 1000"
            )));
        }
    }
    if !body.min_fee.is_finite() || body.min_fee < 0.0 {
        return Err(ApiError::bad_request("min_fee must be zero or positive"));
    }

    let max_order_value = body.max_order_value.unwrap_or(body.initial_cash * 0.30);
    let strategy = store::create_strategy(
        &state.pool,
        store::NewStrategy {
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
        },
    )
    .await?;
    Ok(Json(json!({ "strategy": strategy })))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
        return Err(ApiError::bad_request(
            "status must be active, paused or ended",
        ));
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
    Ok(Json(
        json!({ "strategy": strategy, "portfolio": portfolio, "account": account }),
    ))
}

async fn context(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let (portfolio, _views, account) = build_context(&state, &strategy).await?;
    Ok(Json(
        json!({ "strategy": strategy, "portfolio": portfolio, "account": account }),
    ))
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
#[serde(deny_unknown_fields)]
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
        return Err(ApiError::bad_request(
            "asset_class must be crypto, stock or etf",
        ));
    }
    if !matches!(body.provider.as_str(), "coingecko" | "yahoo" | "china") {
        return Err(ApiError::bad_request(
            "provider must be coingecko, yahoo or china",
        ));
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
    let currency = body
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(if body.provider == "china" {
            "CNY"
        } else {
            &strategy.currency
        });
    if !CURRENCIES.contains(&currency) {
        return Err(ApiError::bad_request("unsupported asset currency"));
    }
    let asset = store::add_strategy_asset(
        &state.pool,
        user.id,
        strategy.id,
        body.asset_class.trim(),
        body.provider.trim(),
        body.provider_id.trim(),
        body.symbol.trim(),
        body.name.trim(),
        currency,
    )
    .await
    .map_err(order_error)?;
    // The quote gate uses the same provider integration as Watchlist/Portfolio.
    if let Err(error) =
        market::ensure_identity(&state.pool, &asset.provider, &asset.provider_id).await
    {
        tracing::warn!("initial paper quote for {} failed: {error:#}", asset.symbol);
    }
    let asset = portfolios::get_asset(&state.pool, asset.id)
        .await?
        .unwrap_or(asset);
    // New paper assets get a scoped Editor thesis page automatically. This
    // is idempotent and repairable via POST /documents/{asset_id} for older
    // assets or if document creation failed after the asset committed.
    if let Err(error) = paper_docs::ensure(&state.pool, strategy.id, asset.id).await {
        tracing::warn!("paper strategy document setup failed: {error:#}");
    }
    Ok(Json(json!({ "asset": asset })))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRunBody {
    /// Stable key for this scheduled analysis cycle; retries must reuse it.
    idempotency_key: String,
    /// Optional RFC3339 scheduled time; omitted means server receipt time.
    scheduled_at: Option<String>,
    #[serde(default = "empty_object")]
    market_facts: Value,
    #[serde(default = "empty_object")]
    market_expectations: Value,
    #[serde(default)]
    inference: String,
    decision: String,
    confidence: i16,
    #[serde(default = "empty_array")]
    evidence: Value,
    #[serde(default = "empty_object")]
    risk_assessment: Value,
}

async fn add_run(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateRunBody>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let key = body.idempotency_key.trim();
    if key.is_empty() || key.len() > 128 {
        return Err(ApiError::bad_request(
            "idempotency_key must be 1-128 characters",
        ));
    }
    let decision = body.decision.trim();
    if !DECISIONS.contains(&decision) {
        return Err(ApiError::bad_request("invalid decision"));
    }
    if !body.market_facts.is_object()
        || !body.market_expectations.is_object()
        || !body.risk_assessment.is_object()
    {
        return Err(ApiError::bad_request(
            "market_facts, market_expectations and risk_assessment must be JSON objects",
        ));
    }
    if !body.evidence.is_array() {
        return Err(ApiError::bad_request("evidence must be a JSON array"));
    }
    if body.inference.len() > 20_000 {
        return Err(ApiError::bad_request(
            "inference is too long (max 20000 bytes)",
        ));
    }
    if !(1..=5).contains(&body.confidence) {
        return Err(ApiError::bad_request("confidence must be between 1 and 5"));
    }
    let scheduled_at = match body.scheduled_at.as_deref() {
        Some(raw) => Some(
            OffsetDateTime::parse(raw, &Rfc3339)
                .map_err(|_| ApiError::bad_request("scheduled_at must be RFC3339"))?,
        ),
        None => None,
    };
    let snapshot = if store::get_run_by_key(&state.pool, id, key).await?.is_some() {
        Value::Null // A retry preserves the original frozen snapshot even when quotes/FX are unavailable.
    } else {
        if strategy.status == "ended" {
            return Err(ApiError::conflict(
                "cannot record new runs for an ended strategy",
            ));
        }
        build_context(&state, &strategy).await?.2
    };
    let run = store::create_run(
        &state.pool,
        strategy.id,
        key,
        scheduled_at,
        &strategy.model,
        &body.market_facts,
        &body.market_expectations,
        body.inference.trim(),
        decision,
        body.confidence,
        &body.evidence,
        &body.risk_assessment,
        &snapshot,
    )
    .await
    .map_err(|error| {
        if error.to_string().contains("idempotency key already used")
            || error.to_string().contains("ended strategy")
        {
            ApiError::conflict(&error.to_string())
        } else {
            tracing::error!("creating paper run failed: {error:#}");
            ApiError::internal("could not create paper run")
        }
    })?;
    Ok(Json(json!({ "run": run })))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
    let side = body.side.trim();
    let key = body.idempotency_key.trim();
    if !matches!(side, "buy" | "sell") {
        return Err(ApiError::bad_request("side must be buy or sell"));
    }
    if !body.quantity.is_finite() || body.quantity <= 0.0 {
        return Err(ApiError::bad_request(
            "quantity must be a positive finite number",
        ));
    }
    if key.is_empty() || key.len() > 128 {
        return Err(ApiError::bad_request(
            "idempotency_key must be 1-128 characters",
        ));
    }
    if body.note.len() > 1000 {
        return Err(ApiError::bad_request(
            "note is too long (max 1000 characters)",
        ));
    }
    if store::get_order_by_key(&state.pool, strategy.id, key)
        .await?
        .is_none()
    {
        let assets = portfolios::list_assets(&state.pool, strategy.portfolio_id).await?;
        market::ensure_portfolio_assets(&state.pool, &assets).await?;
    }
    let order = store::execute_order(
        &state.pool,
        user.id,
        strategy.id,
        body.run_id,
        body.asset_id,
        key,
        side,
        body.quantity,
        body.note.trim(),
    )
    .await
    .map_err(order_error)?;
    Ok(Json(json!({ "order": order })))
}

fn order_error(error: store::OrderError) -> ApiError {
    match error {
        store::OrderError::NotFound => {
            ApiError::not_found("paper strategy, run or asset not found")
        }
        store::OrderError::Invalid(message) => ApiError::bad_request(&message),
        store::OrderError::Conflict(message) => ApiError::conflict(&message),
        store::OrderError::Internal(error) => {
            tracing::error!("paper order execution failed: {error:#}");
            ApiError::internal("paper order execution failed")
        }
    }
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


#[derive(Deserialize)]
struct DayPageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_days(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Query(query): Query<DayPageQuery>,
) -> Result<Json<Value>, ApiError> {
    let strategy = owned_strategy(&state.pool, id, user.id).await?;
    let limit = query.limit.unwrap_or(10).clamp(1, 31);
    let offset = query.offset.unwrap_or(0).max(0);
    let (total, days) = store::list_day_summaries(
        &state.pool, id, strategy.initial_cash, limit, offset,
    ).await?;
    Ok(Json(json!({
        "days": days, "total": total, "limit": limit, "offset": offset,
        "timezone": "Asia/Shanghai",
        "pnl_note": "Estimated difference between pre-order run equity snapshots; not official end-of-day P/L."
    })))
}

async fn day_detail(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, raw_date)): Path<(Uuid, String)>,
    Query(query): Query<DayPageQuery>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let day = Date::parse(&raw_date, &Iso8601::DATE)
        .map_err(|_| ApiError::bad_request("date must be YYYY-MM-DD"))?;
    let limit = query.limit.unwrap_or(100).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let (runs, orders) = tokio::try_join!(
        store::list_runs_for_day(&state.pool, id, day, limit, offset),
        store::list_orders_for_day(&state.pool, id, day, limit, offset),
    )?;
    Ok(Json(json!({"date": raw_date, "runs": runs, "orders": orders,
        "limit": limit, "offset": offset, "timezone": "Asia/Shanghai"})))
}

// Each stock in each paper strategy gets one current thesis document in the
// existing Editor tree. Daily runs are immutable; only a meaningful new
// conclusion warrants updating this shared page.
async fn list_documents(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    Ok(Json(json!({"documents": paper_docs::list(&state.pool, id).await?})))
}

async fn get_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, asset_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let document = paper_docs::get(&state.pool, id, asset_id).await?
        .ok_or_else(|| ApiError::not_found("strategy document not created; POST to ensure it"))?;
    Ok(Json(json!({"document": document})))
}

async fn ensure_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, asset_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let document = paper_docs::ensure(&state.pool, id, asset_id).await?
        .ok_or_else(|| ApiError::not_found("asset is not in this paper strategy"))?;
    Ok(Json(json!({"document": document})))
}

/// PUT expects an explicit optimistic version. Identical content is a no-op;
/// stale agents cannot silently overwrite a human Editor change.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdatePaperDocumentBody {
    expected_updated_at: String,
    /// Full TipTap/ProseMirror JSON document; preserves rich formatting.
    content: Option<Value>,
    /// Simpler alternative for scheduled agents; converts headings, bullets and
    /// paragraphs to Editor blocks. Exactly one of content/markdown is required.
    markdown: Option<String>,
}

async fn update_document(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, asset_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdatePaperDocumentBody>,
) -> Result<Json<Value>, ApiError> {
    owned_strategy(&state.pool, id, user.id).await?;
    let at = OffsetDateTime::parse(&body.expected_updated_at, &Rfc3339)
        .map_err(|_| ApiError::bad_request("expected_updated_at must be RFC3339"))?;
    let content = match (body.content, body.markdown) {
        (Some(content), None) => content,
        (None, Some(markdown)) => markdown_to_tiptap(&markdown),
        _ => return Err(ApiError::bad_request("provide exactly one of content or markdown")),
    };
    if content.to_string().len() > 100_000 {
        return Err(ApiError::bad_request("document exceeds 100 KB"));
    }
    if !content.is_object() || content.get("type").and_then(Value::as_str) != Some("doc")
        || !content.get("content").is_some_and(Value::is_array) {
        return Err(ApiError::bad_request("content must be a TipTap document JSON object"));
    }
    let outcome = paper_docs::update(&state.pool, id, asset_id, at, &content).await?;
    match outcome {
        paper_docs::UpdateOutcome::Missing => Err(ApiError::not_found("strategy document not created")),
        paper_docs::UpdateOutcome::Stale => Err(ApiError::conflict(
            "strategy document changed; reread and re-evaluate before updating")),
        paper_docs::UpdateOutcome::Unchanged => Ok(Json(json!({"updated": false, "reason": "unchanged"}))),
        paper_docs::UpdateOutcome::Updated => {
            let document = paper_docs::get(&state.pool, id, asset_id).await?;
            Ok(Json(json!({"updated": true, "document": document})))
        }
    }
}

/// Minimal, deterministic Markdown → TipTap conversion for agent-authored
/// global notes. Existing manually formatted notes can instead be supplied as
/// full JSON to preserve Editor-specific blocks.
fn markdown_to_tiptap(markdown: &str) -> Value {
    fn inline(text: &str) -> Value {
        json!({"type":"paragraph","content":[{"type":"text","text":text}]})
    }
    let mut blocks = Vec::new();
    let mut bullets: Vec<Value> = Vec::new();
    let flush = |blocks: &mut Vec<Value>, bullets: &mut Vec<Value>| {
        if !bullets.is_empty() {
            blocks.push(json!({"type":"bulletList","content":std::mem::take(bullets)}));
        }
    };
    for line in markdown.lines() {
        let text = line.trim();
        if text.is_empty() {
            flush(&mut blocks, &mut bullets);
            continue;
        }
        if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
            bullets.push(json!({"type":"listItem","content":[inline(item)]}));
            continue;
        }
        flush(&mut blocks, &mut bullets);
        let depth = text.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&depth) && text.as_bytes().get(depth) == Some(&b' ') {
            let title = text[depth + 1..].trim();
            if !title.is_empty() {
                blocks.push(json!({"type":"heading","attrs":{"level":depth},
                    "content":[{"type":"text","text":title}]}));
            }
        } else {
            blocks.push(inline(text));
        }
    }
    flush(&mut blocks, &mut bullets);
    json!({"type":"doc","content":blocks})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_contract_rejects_client_prices_fees_and_portfolio_ids() {
        let valid = json!({"idempotency_key":"order-1", "run_id":Uuid::new_v4(),
            "asset_id":Uuid::new_v4(), "side":"buy", "quantity":10.0});
        assert!(serde_json::from_value::<PlaceOrderBody>(valid.clone()).is_ok());
        for field in [
            "price",
            "fill_price",
            "fee",
            "portfolio_id",
            "user_id",
            "quote_at",
        ] {
            let mut body = valid.clone();
            body[field] = json!(1);
            assert!(
                serde_json::from_value::<PlaceOrderBody>(body).is_err(),
                "accepted {field}"
            );
        }
    }

    #[test]
    fn run_contract_rejects_client_account_snapshots() {
        let mut body = json!({"idempotency_key":"run-1", "decision":"no_trade", "confidence":3});
        assert!(serde_json::from_value::<CreateRunBody>(body.clone()).is_ok());
        body["account_snapshot"] = json!({"cash":999999});
        assert!(serde_json::from_value::<CreateRunBody>(body).is_err());
    }
}
