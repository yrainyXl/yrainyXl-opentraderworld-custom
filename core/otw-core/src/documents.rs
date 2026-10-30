//! HTTP API for the editor module's document tree (folders + pages). Single-user.
//!
//! Research documents (linked from `investment_cases.document_id`) get extra
//! guards here: ownership of the linked case must match the authenticated user
//! (GET/PATCH/DELETE). The link is a detachable reference, not a required
//! existence: deleting a referenced document succeeds and the case link is set
//! to NULL by the FK (ON DELETE SET NULL) — the case and its decision/change
//! records stay untouched. FK violations (23503) from other future RESTRICT
//! references are still mapped to a clean 409, never a raw 500.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::JsonValue;
use uuid::Uuid;

use crate::{ApiError, AppState};

/// Ownership guard for research documents: when this document is referenced by
/// an investment case, the case's user must be the authenticated user.
/// Returns `Ok(())` for non-research documents (single-user model: everything
/// else is shared). Foreign research documents answer 404 (no existence leak).
async fn research_guard(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
) -> Result<(), ApiError> {
    let Some(case) = otw_store::investment_cases::find_case_by_document_id(&state.pool, id).await?
    else {
        return Ok(());
    };
    if case.user_id != user_id {
        return Err(ApiError::not_found("document not found"));
    }
    Ok(())
}

fn is_fk_violation(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        c.downcast_ref::<sqlx::Error>()
            .and_then(|e| e.as_database_error())
            .is_some_and(|db| db.code().as_deref() == Some("23503"))
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/documents", get(list).post(create))
        .route(
            "/api/documents/{id}",
            get(get_one).patch(update).delete(delete),
        )
        .route("/api/documents/{id}/move", post(move_doc))
        .route("/api/documents/{id}/flag", post(set_flag))
}

/// GET /api/documents — the whole tree (metadata only).
async fn list(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let docs = otw_store::documents::list_tree(&state.pool).await?;
    Ok(Json(json!({ "documents": docs })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateBody {
    parent_id: Option<Uuid>,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    title: String,
}
fn default_kind() -> String {
    "page".to_string()
}

/// POST /api/documents — create a folder or page.
async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(body.kind.as_str(), "page" | "folder" | "database") {
        return Err(ApiError::bad_request(
            "kind must be 'page', 'folder' or 'database'",
        ));
    }
    let doc = otw_store::documents::create(&state.pool, body.parent_id, &body.kind, &body.title)
        .await?;
    Ok(Json(json!({ "document": doc })))
}

/// GET /api/documents/{id} — one document with content.
async fn get_one(
    State(state): State<AppState>,
    Extension(user): Extension<otw_store::User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    research_guard(&state, user.id, id).await?;
    let doc = otw_store::documents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("document not found"))?;
    Ok(Json(json!({ "document": doc })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateBody {
    title: Option<String>,
    content: Option<JsonValue>,
    layout: Option<String>,
}

/// PATCH /api/documents/{id} — update title, content and/or layout (autosave).
async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<otw_store::User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<Value>, ApiError> {
    research_guard(&state, user.id, id).await?;
    if let Some(l) = &body.layout {
        if !matches!(l.as_str(), "normal" | "wide") {
            return Err(ApiError::bad_request("layout must be 'normal' or 'wide'"));
        }
    }
    let ok = otw_store::documents::update(
        &state.pool,
        id,
        body.title.as_deref(),
        body.content.as_ref(),
        body.layout.as_deref(),
    )
    .await?;
    if !ok {
        return Err(ApiError::not_found("document not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MoveBody {
    parent_id: Option<Uuid>,
    position: f64,
}

/// POST /api/documents/{id}/move — reparent / reorder.
async fn move_doc(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveBody>,
) -> Result<Json<Value>, ApiError> {
    let ok = otw_store::documents::mv(&state.pool, id, body.parent_id, body.position).await?;
    if !ok {
        return Err(ApiError::not_found("document not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Colour flags a document may carry (TradingView-style tree markers).
const FLAGS: [&str; 6] = ["red", "orange", "yellow", "green", "blue", "purple"];

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct FlagBody {
    /// One of `FLAGS`, or null to clear.
    flag: Option<String>,
}

/// POST /api/documents/{id}/flag — set or clear the colour flag.
async fn set_flag(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<FlagBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(f) = &body.flag {
        if !FLAGS.contains(&f.as_str()) {
            return Err(ApiError::bad_request("unknown flag colour"));
        }
    }
    let ok = otw_store::documents::set_flag(&state.pool, id, body.flag.as_deref()).await?;
    if !ok {
        return Err(ApiError::not_found("document not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// DELETE /api/documents/{id} — delete (cascades to children). When the
/// document is the research page of an investment case, the case link is
/// detached (document_id → NULL) rather than blocking the delete.
async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<otw_store::User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    research_guard(&state, user.id, id).await?;
    // Deleting a case-linked research page is allowed: the FK (ON DELETE
    // SET NULL) detaches the case's document_id, keeping the case and its
    // decision/change records intact. No pre-check here anymore.
    // FK violations (23503) from other RESTRICT references are still mapped to
    // a clean 409 as the final race protection — never a raw 500.
    let result = otw_store::documents::delete(&state.pool, id).await;
    let ok = match result {
        Ok(ok) => ok,
        Err(err) if is_fk_violation(&err) => {
            return Err(ApiError::conflict(
                "document is referenced by another record; delete that record first",
            ))
        }
        Err(err) => return Err(err.into()),
    };
    if !ok {
        return Err(ApiError::not_found("document not found"));
    }
    Ok(Json(json!({ "ok": true })))
}
