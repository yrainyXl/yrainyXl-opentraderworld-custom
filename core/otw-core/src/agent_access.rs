//! Bearer-token authentication for external AI agents.
//!
//! `GET /api/agent/ping` is the Phase-1 verification endpoint: an external agent proves
//! it can reach OTW with its own credential, without a user password or a browser cookie.
//!
//! The route is mounted OUTSIDE the session-cookie middleware on purpose. The only
//! credential it accepts is `Authorization: Bearer <API_TOKEN>`; a browser session cookie
//! is ignored here, and this token is ignored by every session-protected route. The two
//! lookups read different transports and different tables, so neither can be replayed as
//! the other.
//!
//! Request identity comes from the token row alone. The handler takes no body, no query
//! parameters and no user identifier: there is no input that could claim to be another
//! user.
//!
//! Error contract:
//! * missing / malformed / unknown / expired / revoked token -> 401
//! * valid token without the required scope                    -> 403

use axum::{
    extract::State,
    http::{header, HeaderMap},
    Json,
};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{ApiError, AppState};
use otw_store::agent_api_tokens as store;

/// `GET /api/agent/ping` — read-only authentication probe.
///
/// Touches no business table. The only write is `agent_api_tokens.last_used_at` after a
/// fully successful authentication (auth metadata, not business data), so repeating the
/// request cannot change portfolios, orders, holdings or any other trading fact.
pub async fn ping(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = authenticate(&state, &headers).await?;

    if !principal.has_scope(store::SCOPE_AGENT_PING) {
        return Err(ApiError::forbidden(
            "API token does not carry the agent:ping scope",
        ));
    }

    // Real server clock, formatted as RFC 3339 with the process offset (UTC in the
    // distroless container). Never a fixed string.
    let server_time = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| {
            tracing::error!("agent ping: formatting server time: {e}");
            ApiError::internal("could not format server time")
        })?;

    // The agent label comes from the server-side token row, never from the request.
    Ok(Json(json!({
        "status": "ok",
        "agent": principal.agent,
        "serverTime": server_time,
        "authenticated": true,
    })))
}

/// Resolve the bearer token to an active token row, or the matching 401.
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<store::AgentApiToken, ApiError> {
    let token = bearer_token(headers).ok_or_else(|| {
        ApiError::unauthorized("missing or malformed bearer token")
    })?;

    // Shape check first: keeps garbage away from the database and guarantees a session
    // token or an MCP token can never be considered here.
    if !store::looks_like_token(token) {
        return Err(ApiError::unauthorized("invalid API token"));
    }

    let token_row = store::find_by_token(&state.pool, token)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid API token"))?;

    // Expiry/revocation are named only to the holder of a matching bearer value; the
    // message tells the operator what to fix instead of "check your token".
    if token_row.is_revoked() {
        return Err(ApiError::unauthorized("API token has been revoked"));
    }
    if token_row.is_expired() {
        return Err(ApiError::unauthorized("API token has expired"));
    }

    // Metadata only; a failure here must not fail an otherwise valid ping.
    if let Err(e) = store::touch_last_used(&state.pool, token_row.id).await {
        tracing::warn!("agent ping: updating token last_used_at failed: {e:#}");
    }

    // Never log the token itself — not even a prefix is written here (the prefix is
    // display metadata for the operator, not a server log field).
    tracing::debug!(
        "agent api authenticated: token_id={} agent={}",
        token_row.id,
        token_row.agent
    );
    Ok(token_row)
}

/// Extract `Bearer <token>` from the Authorization header.
///
/// The scheme is case-insensitive per RFC 7235. Exactly one non-empty token is required;
/// a malformed header is treated as "no token" (401), never as an unauthenticated
/// fallback to some other identity.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() || token.split_ascii_whitespace().count() != 1 {
        return None;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn accepts_bearer_with_optional_case_difference() {
        assert_eq!(
            bearer_token(&headers_with("Bearer otw_agent_abc")),
            Some("otw_agent_abc")
        );
        assert_eq!(
            bearer_token(&headers_with("bearer otw_agent_abc")),
            Some("otw_agent_abc")
        );
    }

    #[test]
    fn rejects_missing_or_non_bearer_credentials() {
        assert_eq!(bearer_token(&HeaderMap::new()), None);
        assert_eq!(
            bearer_token(&headers_with("Basic dXNlcjpwYXNz")),
            None
        );
        assert_eq!(bearer_token(&headers_with("Bearer")), None);
        assert_eq!(bearer_token(&headers_with("Bearer   ")), None);
        assert_eq!(
            bearer_token(&headers_with("Bearer one two")),
            None
        );
    }

    #[test]
    fn malformed_header_is_not_an_identity() {
        // A cookie-like or query-like value is just a bad header, never a user claim.
        assert_eq!(
            bearer_token(&headers_with("otw_agent_abc")),
            None
        );
    }
}
