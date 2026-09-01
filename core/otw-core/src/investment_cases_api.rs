//! HTTP API for Investment Case (Phase 1 MVP).
//!
//! Every handler performs its OWN ownership check on top of `require_auth`:
//! the authenticated user is taken from the request extensions and compared
//! against `investment_cases.user_id`. Foreign cases are answered with 404
//! (no existence leak), never with the case's data.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{ApiError, AppState};
use otw_store::investment_cases;
use otw_store::User;

const DECISION_TYPES: [&str; 6] = ["observe", "hold", "buy", "add", "reduce", "exit"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/investment-cases", post(ensure))
        .route("/api/investment-cases/{id}", get(get_one))
        .route("/api/investment-cases/{id}/live", get(live))
        .route(
            "/api/investment-cases/{id}/decisions",
            get(list_decisions).post(add_decision),
        )
}

/// Load the case and enforce ownership: returns the case only when it belongs
/// to `user_id`; anything else is a 404 (existence is not leaked).
async fn owned_case(
    pool: &sqlx::PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<investment_cases::InvestmentCase, ApiError> {
    match investment_cases::get_case(pool, id).await? {
        Some(c) if c.user_id == user_id => Ok(c),
        _ => Err(ApiError::not_found("investment case not found")),
    }
}

/// POST /api/investment-cases — ensure a case for the portfolio asset's
/// instrument. Identity fields are resolved server-side from the asset row;
/// the client only supplies `portfolio_asset_id`.
#[derive(Deserialize)]
struct EnsureBody {
    portfolio_asset_id: Uuid,
}

async fn ensure(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<EnsureBody>,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let asset = otw_store::portfolios::get_asset(&state.pool, body.portfolio_asset_id)
        .await?
        .ok_or_else(|| ApiError::not_found("portfolio asset not found"))?;
    // Single-user model: portfolios carry no user column, so every asset
    // belongs to the current user; case ownership is enforced via user_id.
    let case = investment_cases::ensure_case(&state.pool, user_id, &asset).await?;
    Ok(Json(json!({ "case": case })))
}

/// GET /api/investment-cases/{id} — the case (with its document_id; document
/// content stays with the Editor API, which is the content SSOT).
async fn get_one(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let case = owned_case(&state.pool, id, user_id).await?;
    Ok(Json(json!({ "case": case })))
}

/// GET /api/investment-cases/{id}/live — real-time aggregated position.
async fn live(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let case = owned_case(&state.pool, id, user_id).await?;
    let pos = investment_cases::instrument_position(&state.pool, &case).await?;
    // Serialize the position struct as a whole so its `#[serde(with = rfc3339)]`
    // attributes apply (field access would use OffsetDateTime's default format).
    let mut payload =
        serde_json::to_value(&pos).map_err(|e| ApiError::internal(&e.to_string()))?;
    if let Some(o) = payload.as_object_mut() {
        let position = json!({
            "quantity": o.remove("quantity"),
            "avg_cost": o.remove("avg_cost"),
            "cost_basis": o.remove("cost_basis"),
            "market_value": o.remove("market_value"),
            "unrealized_pnl": o.remove("unrealized_pnl"),
            "unrealized_pct": o.remove("unrealized_pct"),
        });
        o.insert("position".into(), position);
        o.insert(
            "instrument".into(),
            json!({
                "provider": case.provider,
                "provider_id": case.provider_id,
                "symbol": case.symbol,
                "name": case.name,
                "exchange": case.exchange,
                "currency": case.currency,
                "asset_class": case.asset_class,
            }),
        );
        o.insert("currency".into(), json!(case.currency));
    }
    Ok(Json(payload))
}

#[derive(Deserialize)]
struct DecisionBody {
    decision_type: String,
    #[serde(default = "default_confidence")]
    confidence: i16,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    counter_evidence: String,
    #[serde(default)]
    invalidation: String,
    #[serde(default)]
    watch_items: String,
}
fn default_confidence() -> i16 {
    3
}

/// POST /api/investment-cases/{id}/decisions — record a decision. The snapshot
/// is computed server-side with the same function as /live; the client never
/// supplies snapshot numbers.
async fn add_decision(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<DecisionBody>,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let case = owned_case(&state.pool, id, user_id).await?;

    if !DECISION_TYPES.contains(&body.decision_type.as_str()) {
        return Err(ApiError::bad_request(
            "decision_type must be one of observe|hold|buy|add|reduce|exit",
        ));
    }
    if !(1..=5).contains(&body.confidence) {
        return Err(ApiError::bad_request("confidence must be between 1 and 5"));
    }

    let pos = investment_cases::instrument_position(&state.pool, &case).await?;
    let decision = investment_cases::create_decision(
        &state.pool,
        &case,
        &investment_cases::NewDecision {
            decision_type: &body.decision_type,
            confidence: body.confidence,
            reason: &body.reason,
            evidence: &body.evidence,
            counter_evidence: &body.counter_evidence,
            invalidation: &body.invalidation,
            watch_items: &body.watch_items,
        },
        &pos,
    )
    .await?;
    Ok(Json(json!({ "decision": decision })))
}

/// GET /api/investment-cases/{id}/decisions — decision timeline.
async fn list_decisions(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user_id = user.id;
    let case = owned_case(&state.pool, id, user_id).await?;
    let decisions = investment_cases::list_decisions(&state.pool, case.id).await?;
    Ok(Json(json!({ "decisions": decisions })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_type_validation_matches_db_check() {
        assert_eq!(DECISION_TYPES, ["observe", "hold", "buy", "add", "reduce", "exit"]);
    }
}
