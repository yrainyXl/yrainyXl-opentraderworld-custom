//! OAuth 2.1 authorization server for the MCP endpoint.
//!
//! This is the browser-facing half of the ChatGPT/Codex connector flow: discovery
//! metadata, the authorization + consent page, dynamic client registration, the token
//! endpoint and revocation. The resource-server half lives in [`crate::mcp`], which
//! accepts both fixed `otw_mcp_` tokens (local Codex, unchanged) and these OAuth access
//! tokens (ChatGPT).
//!
//! Security properties enforced here:
//! * PKCE S256 is mandatory; `plain` and missing challenges are rejected.
//! * `redirect_uri`, `client_id`, `resource` and scopes are bound to the authorization
//!   code and re-checked at exchange time.
//! * The consent page is CSRF-protected with a one-time hashed token; the request row is
//!   consumed atomically, so a decision cannot be replayed.
//! * Discovery URLs are built only from `OTW_PUBLIC_BASE_URL`, never from Host or
//!   X-Forwarded-* headers.
//! * No code, verifier, token or cookie value is ever logged.
//! * `mcp:read` is read-only. The separate `mcp:paper-trade` opt-in grants write access
//!   only to the isolated Paper Trading module; normal Portfolio remains read-only.
//!
//! OTW's normal business data is single-user: authorization is limited to admin accounts and
//! normal Portfolio rows do not carry ownership. Paper Trading strategies are user-bound and
//! expose only their linked isolated paper portfolio through the dedicated module. See C-018.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Form, Json, Router,
};
use axum_extra::extract::cookie::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{ApiError, AppState, SESSION_COOKIE};
use otw_store::mcp::hash_token;
use otw_store::oauth as store;
use otw_store::oauth::{OAuthClient, SCOPE_MCP_READ, SCOPE_MCP_PAPER_TRADE, SCOPE_OFFLINE_ACCESS};

const REGISTER_LIMIT: u32 = 20;
const REGISTER_WINDOW: Duration = Duration::from_secs(60);
const MAX_CLIENTS: i64 = 200;

static REGISTER_ATTEMPTS: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// Public routes: discovery, authorize/consent, token, register, revoke. Mounted OUTSIDE
/// the session middleware; the authorization handlers inspect the cookie themselves
/// because a missing session must produce the consent/login page, not a bare 401.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected_resource_metadata))
        .route("/.well-known/oauth-protected-resource/{*rest}", get(protected_resource_metadata))
        .route("/.well-known/oauth-authorization-server", get(authorization_server_metadata))
        .route("/.well-known/oauth-authorization-server/{*rest}", get(authorization_server_metadata))
        .route("/.well-known/openid-configuration", get(well_known_not_found))
        .route("/.well-known/openid-configuration/{*rest}", get(well_known_not_found))
        .route("/api/oauth/authorize", get(authorize))
        .route("/api/oauth/authorize/decision", post(authorize_decision))
        .route("/api/oauth/token", post(token))
        .route("/api/oauth/register", post(register))
        .route("/api/oauth/revoke", post(revoke))
}

/// Session-protected routes: the user-facing list/revoke surface for grants. There is no
/// dedicated Settings UI yet; the endpoints and the host CLI are the revocation paths.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/oauth/grants", get(list_grants))
        .route("/api/oauth/grants/{client_id}", delete(revoke_grant))
}

// ── Configuration ────────────────────────────────────────────────────────────

/// Normalize `OTW_PUBLIC_BASE_URL`. Returns an origin (`https://host[:port]`) or `None`.
///
/// Only HTTPS is accepted, except for loopback HTTP so local tests can run without TLS.
/// A path/query/fragment is rejected: the value is used to mint security URLs, so it must
/// not be able to point the client at an attacker-selected path.
pub fn parse_public_base(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let url = reqwest::Url::parse(raw).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    if url.path() != "/" && !url.path().is_empty() {
        return None;
    }
    let host = url.host_str()?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]";
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !scheme_ok {
        return None;
    }
    let mut base = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        base.push_str(&format!(":{port}"));
    }
    Some(base)
}

fn resource_url(base: &str) -> String {
    format!("{base}/api/mcp")
}

fn require_base(state: &AppState) -> Result<String, Response> {
    state.public_base_url.clone().ok_or_else(|| {
        oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "OAuth is not configured: set OTW_PUBLIC_BASE_URL",
        )
    })
}

// ── Discovery ────────────────────────────────────────────────────────────────

async fn protected_resource_metadata(State(state): State<AppState>) -> Response {
    let Some(base) = state.public_base_url.clone() else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "OAuth is not configured",
        );
    };
    json_response(
        StatusCode::OK,
        json!({
            "resource": resource_url(&base),
            "authorization_servers": [base],
            "scopes_supported": [SCOPE_MCP_READ, SCOPE_MCP_PAPER_TRADE, SCOPE_OFFLINE_ACCESS],
            "bearer_methods_supported": ["header"],
            "resource_name": "OpenTraderWorld MCP",
            "resource_documentation": base,
        }),
    )
}

async fn authorization_server_metadata(State(state): State<AppState>) -> Response {
    let Some(base) = state.public_base_url.clone() else {
        return oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "OAuth is not configured",
        );
    };
    json_response(
        StatusCode::OK,
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/api/oauth/authorize"),
            "token_endpoint": format!("{base}/api/oauth/token"),
            "registration_endpoint": format!("{base}/api/oauth/register"),
            "revocation_endpoint": format!("{base}/api/oauth/revoke"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "revocation_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": [SCOPE_MCP_READ, SCOPE_MCP_PAPER_TRADE, SCOPE_OFFLINE_ACCESS],
            // RFC 9207: the client must verify `iss` on the callback.
            "authorization_response_iss_parameter_supported": true,
            "service_documentation": base,
        }),
    )
}

async fn well_known_not_found() -> Response {
    oauth_error(StatusCode::NOT_FOUND, "not_found", "no such well-known document")
}

// ── Authorization endpoint ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<String>,
    scope: Option<String>,
}

async fn authorize(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    let base = match require_base(&state) {
        Ok(b) => b,
        Err(r) => return r,
    };

    // Validate the client and redirect URI before any redirect is possible; an
    // unregistered redirect_uri must never receive an error redirect (open redirect).
    let Some(client_public_id) = q.client_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
    else {
        return html_error(StatusCode::BAD_REQUEST, "Invalid request", "client_id is required.");
    };
    let client = match store::find_client(&state.pool, client_public_id).await {
        Ok(Some(c)) if !c.is_disabled() => c,
        Ok(Some(_)) => {
            return html_error(StatusCode::FORBIDDEN, "Client disabled", "This client is disabled.");
        }
        Ok(None) => {
            return html_error(StatusCode::BAD_REQUEST, "Unknown client", "This client is not registered.");
        }
        Err(e) => {
            tracing::error!("oauth authorize: client lookup failed: {e:#}");
            return html_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error", "Could not load the client.");
        }
    };
    let Some(redirect_uri) = q.redirect_uri.as_deref() else {
        return html_error(StatusCode::BAD_REQUEST, "Invalid request", "redirect_uri is required.");
    };
    if !client.grant_types.iter().any(|g| g == "authorization_code") {
        return html_error(
            StatusCode::BAD_REQUEST,
            "Client not allowed",
            "This client is not registered for the authorization_code grant.",
        );
    }
    if !valid_redirect_uri(redirect_uri) || !client.allows_redirect(redirect_uri) {
        return html_error(
            StatusCode::BAD_REQUEST,
            "Invalid redirect_uri",
            "This redirect URI is not registered for the client.",
        );
    }
    let state_param = q.state.as_deref().filter(|s| !s.is_empty());
    if state_param.is_some_and(|s| !valid_state(s)) {
        return html_error(StatusCode::BAD_REQUEST, "Invalid state", "state is too long or contains control characters.");
    }

    if q.response_type.as_deref() != Some("code") {
        return redirect_error(&base, redirect_uri, state_param, "unsupported_response_type", "only response_type=code is supported");
    }
    let Some(challenge) = q.code_challenge.as_deref().filter(|c| valid_pkce_challenge(c)) else {
        return redirect_error(&base, redirect_uri, state_param, "invalid_request", "a valid PKCE code_challenge is required");
    };
    if q.code_challenge_method.as_deref() != Some("S256") {
        return redirect_error(&base, redirect_uri, state_param, "invalid_request", "code_challenge_method must be S256");
    }
    let resource = q.resource.as_deref().unwrap_or("");
    if resource != resource_url(&base) {
        return redirect_error(&base, redirect_uri, state_param, "invalid_target", "resource must identify this MCP server");
    }
    let scopes = match parse_scopes(q.scope.as_deref(), &client) {
        Ok(s) => s,
        Err(e) => return redirect_error(&base, redirect_uri, state_param, "invalid_scope", &e),
    };

    // The resource owner must already be signed in to OTW in this browser. We do not
    // invent a login redirect: the user signs in and retries the connector.
    let user = match jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) {
        Some(token) => match otw_store::user_for_session(&state.pool, &token).await {
            Ok(Some(u)) => u,
            Ok(None) => {
                return html_error(
                    StatusCode::UNAUTHORIZED,
                    "Sign in required",
                    "Sign in to OpenTraderWorld in this browser, then retry the connector authorization.",
                );
            }
            Err(e) => {
                tracing::error!("oauth authorize: session lookup failed: {e:#}");
                return html_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error", "Could not verify the session.");
            }
        },
        None => {
            return html_error(
                StatusCode::UNAUTHORIZED,
                "Sign in required",
                "Sign in to OpenTraderWorld in this browser, then retry the connector authorization.",
            );
        }
    };
    if !user.is_admin {
        return html_error(
            StatusCode::FORBIDDEN,
            "Not allowed",
            "Only the OpenTraderWorld owner account can authorize MCP access. The current OTW data model is single-user.",
        );
    }

    let mcp_enabled = otw_store::settings::get_or(&state.pool, "mcp_enabled", "false").await;
    if !matches!(mcp_enabled.as_deref(), Ok("true")) {
        return html_error(
            StatusCode::FORBIDDEN,
            "MCP disabled",
            "Enable MCP in OpenTraderWorld Settings → AI agents before connecting a client.",
        );
    }

    let csrf = match store::random_hex(24) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("oauth authorize: csrf generation failed: {e:#}");
            return html_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error", "Could not start the consent flow.");
        }
    };
    let pending_id = match store::create_pending(
        &state.pool,
        user.id,
        client.id,
        redirect_uri,
        resource,
        &scopes,
        state_param,
        challenge,
        "S256",
        &hash_token(&csrf),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("oauth authorize: pending creation failed: {e:#}");
            return html_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error", "Could not start the consent flow.");
        }
    };

    tracing::info!(
        "oauth consent requested: user_id={} client_id={} scopes=[{}]",
        user.id,
        client.client_id,
        scopes.join(", ")
    );
    let html = consent_html(&user.username, &client, &scopes, resource, pending_id, &csrf);
    html_response(StatusCode::OK, html)
}

#[derive(Deserialize)]
struct DecisionForm {
    request_id: uuid::Uuid,
    csrf: String,
    decision: String,
}

async fn authorize_decision(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<DecisionForm>,
) -> Response {
    let base = match require_base(&state) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let user = match jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) {
        Some(token) => match otw_store::user_for_session(&state.pool, &token).await {
            Ok(Some(u)) => u,
            _ => return html_error(StatusCode::UNAUTHORIZED, "Sign in required", "Your OTW session has expired."),
        },
        None => return html_error(StatusCode::UNAUTHORIZED, "Sign in required", "Your OTW session has expired."),
    };

    let pending = match store::consume_pending(
        &state.pool,
        form.request_id,
        user.id,
        &hash_token(&form.csrf),
    )
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return html_error(
                StatusCode::BAD_REQUEST,
                "Request expired",
                "This authorization request expired or was already used. Start the connection again.",
            );
        }
        Err(e) => {
            tracing::error!("oauth decision: pending consume failed: {e:#}");
            return html_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error", "Could not process the decision.");
        }
    };

    let client = match store::find_client_by_uuid(&state.pool, pending.client_id).await {
        Ok(Some(c)) => c,
        _ => return html_error(StatusCode::BAD_REQUEST, "Unknown client", "The client no longer exists."),
    };

    if form.decision != "allow" {
        tracing::info!("oauth consent denied: user_id={} client_id={}", user.id, client.client_id);
        return redirect_error(
            &base,
            &pending.redirect_uri,
            pending.state.as_deref(),
            "access_denied",
            "the resource owner denied the request",
        );
    }

    let code = match store::create_code(&state.pool, &pending).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("oauth decision: code creation failed: {e:#}");
            return redirect_error(
                &base,
                &pending.redirect_uri,
                pending.state.as_deref(),
                "server_error",
                "could not issue an authorization code",
            );
        }
    };
    if let Err(e) = store::touch_client(&state.pool, client.id).await {
        tracing::warn!("oauth decision: touch client failed: {e:#}");
    }
    tracing::info!(
        "oauth consent granted: user_id={} client_id={} scopes=[{}]",
        user.id,
        client.client_id,
        pending.scopes.join(", ")
    );

    let mut params: Vec<(&str, String)> = vec![("code", code), ("iss", base)];
    if let Some(s) = pending.state.as_deref() {
        params.push(("state", s.to_string()));
    }
    redirect_with(&pending.redirect_uri, &params)
}

// ── Token endpoint ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    client_id: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

async fn token(State(state): State<AppState>, Form(form): Form<TokenForm>) -> Response {
    let base = match require_base(&state) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(client_public_id) = form.client_id.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "client_id is required");
    };
    let client = match store::find_client(&state.pool, client_public_id).await {
        Ok(Some(c)) if !c.is_disabled() => c,
        Ok(Some(_)) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_client", "client is disabled"),
        Ok(None) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_client", "unknown client"),
        Err(e) => {
            tracing::error!("oauth token: client lookup failed: {e:#}");
            return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not load the client");
        }
    };
    let expected_resource = resource_url(&base);
    if form.resource.as_deref() != Some(expected_resource.as_str()) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_target", "resource must identify this MCP server");
    }

    match form.grant_type.as_str() {
        "authorization_code" => {
            if !client.grant_types.iter().any(|g| g == "authorization_code") {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "unauthorized_client",
                    "client is not registered for the authorization_code grant",
                );
            }
            let (Some(code), Some(redirect_uri), Some(verifier)) = (
                form.code.as_deref().filter(|s| !s.is_empty()),
                form.redirect_uri.as_deref(),
                form.code_verifier.as_deref(),
            ) else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "code, redirect_uri and code_verifier are required",
                );
            };
            if !valid_code_verifier(verifier) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "invalid code_verifier");
            }
            let code = match store::consume_code(&state.pool, code, client.id, redirect_uri, &expected_resource).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", code_error_text(e)),
                Err(e) => {
                    tracing::error!("oauth token: code exchange failed: {e:#}");
                    return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not exchange the code");
                }
            };
            if code.code_challenge_method != "S256" || pkce_s256(verifier) != code.code_challenge {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "PKCE verification failed");
            }
            let issued = match store::issue_tokens(
                &state.pool,
                client.id,
                code.user_id,
                &expected_resource,
                &code.scopes,
            )
            .await
            {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!("oauth token: issue failed: {e:#}");
                    return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not issue tokens");
                }
            };
            tracing::info!(
                "oauth token issued: client_id={} user_id={} grant=authorization_code",
                client.client_id,
                code.user_id
            );
            token_response(&issued, &expected_resource)
        }
        "refresh_token" => {
            if !client.grant_types.iter().any(|g| g == "refresh_token") {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "unauthorized_client",
                    "client is not registered for the refresh_token grant",
                );
            }
            let Some(refresh) = form.refresh_token.as_deref().filter(|s| !s.is_empty()) else {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "refresh_token is required");
            };
            match store::rotate_refresh(&state.pool, refresh, client.id, &expected_resource).await {
                Ok(Ok(issued)) => {
                    tracing::info!(
                        "oauth token issued: client_id={} grant=refresh_token",
                        client.client_id
                    );
                    token_response(&issued, &expected_resource)
                }
                Ok(Err(store::RefreshError::Replay)) => {
                    tracing::warn!(
                        "oauth refresh token replay detected; family revoked: client_id={}",
                        client.client_id
                    );
                    oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token replay detected")
                }
                Ok(Err(e)) => oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", refresh_error_text(e)),
                Err(e) => {
                    tracing::error!("oauth token: refresh failed: {e:#}");
                    oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not rotate the refresh token")
                }
            }
        }
        _ => oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "only authorization_code and refresh_token are supported"),
    }
}

fn token_response(issued: &store::IssuedTokens, resource: &str) -> Response {
    let now = time::OffsetDateTime::now_utc();
    let expires_in = (issued.access_expires_at - now).whole_seconds().max(0);
    let mut resp = json_response(
        StatusCode::OK,
        json!({
            "access_token": issued.access_token,
            "token_type": "Bearer",
            "expires_in": expires_in,
            "refresh_token": issued.refresh_token,
            "scope": issued.scopes.join(" "),
            "resource": resource,
        }),
    );
    resp.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp
}

fn code_error_text(e: store::CodeError) -> &'static str {
    match e {
        store::CodeError::Invalid => "unknown authorization code",
        store::CodeError::Expired => "authorization code has expired",
        store::CodeError::Reused => "authorization code has already been used",
        store::CodeError::ClientMismatch => "authorization code was issued to another client",
        store::CodeError::RedirectMismatch => "redirect_uri does not match the authorization request",
        store::CodeError::ResourceMismatch => "resource does not match the authorization request",
    }
}

fn refresh_error_text(e: store::RefreshError) -> &'static str {
    match e {
        store::RefreshError::Invalid => "invalid refresh token",
        store::RefreshError::Expired => "refresh token has expired",
        store::RefreshError::Replay => "refresh token replay detected",
        store::RefreshError::ClientMismatch => "refresh token was issued to another client",
        store::RefreshError::ResourceMismatch => "refresh token was issued for another resource",
    }
}

// ── Dynamic client registration ─────────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    token_endpoint_auth_method: Option<String>,
    scope: Option<String>,
}

async fn register(State(state): State<AppState>, Json(input): Json<RegisterRequest>) -> Response {
    if let Err(r) = require_base(&state) {
        return r;
    }
    if registration_throttled() {
        return oauth_error(StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable", "too many registrations; retry later");
    }
    record_registration();

    if input.redirect_uris.is_empty() || input.redirect_uris.len() > 5 {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_redirect_uri", "redirect_uris must contain 1-5 entries");
    }
    for uri in &input.redirect_uris {
        if !valid_redirect_uri(uri) {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
                "redirect URIs must be HTTPS, or HTTP on localhost/127.0.0.1, without fragments or userinfo",
            );
        }
    }
    let name = input
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("MCP client");
    if name.chars().count() > 100 {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", "client_name is too long");
    }
    let grant_types = input
        .grant_types
        .unwrap_or_else(|| vec!["authorization_code".into(), "refresh_token".into()]);
    if !grant_types.iter().any(|g| g == "authorization_code")
        || grant_types
            .iter()
            .any(|g| g != "authorization_code" && g != "refresh_token")
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "grant_types must contain authorization_code and may contain refresh_token",
        );
    }
    let response_types = input.response_types.unwrap_or_else(|| vec!["code".into()]);
    if response_types.len() != 1 || response_types[0] != "code" {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", "response_types must be [\"code\"]");
    }
    if input.token_endpoint_auth_method.as_deref().unwrap_or("none") != "none" {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "only public clients (token_endpoint_auth_method=none) are supported",
        );
    }
    let scopes = match input.scope.as_deref() {
        Some(raw) => match parse_scope_list(raw) {
            Ok(s) => s,
            Err(e) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_scope", &e),
        },
        None => vec![SCOPE_MCP_READ.to_string(), SCOPE_OFFLINE_ACCESS.to_string()],
    };

    match store::count_clients(&state.pool).await {
        Ok(n) if n >= MAX_CLIENTS => {
            return oauth_error(StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable", "client limit reached");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("oauth register: client count failed: {e:#}");
            return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not register the client");
        }
    }

    let (row, client_id) = match store::create_client(
        &state.pool,
        name,
        &input.redirect_uris,
        &scopes,
        "dcr",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("oauth register: create failed: {e:#}");
            return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not register the client");
        }
    };
    tracing::info!("oauth client registered: client_id={} name={}", client_id, row.client_name);
    let mut resp = json_response(
        StatusCode::CREATED,
        json!({
            "client_id": row.client_id,
            "client_id_issued_at": row.created_at.unix_timestamp(),
            "client_name": row.client_name,
            "redirect_uris": row.redirect_uris,
            "grant_types": row.grant_types,
            "response_types": row.response_types,
            "token_endpoint_auth_method": row.token_endpoint_auth_method,
            "scope": row.scopes.join(" "),
        }),
    );
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn registration_throttled() -> bool {
    let guard = REGISTER_ATTEMPTS.lock().unwrap();
    matches!(*guard, Some((start, n)) if n >= REGISTER_LIMIT && start.elapsed() < REGISTER_WINDOW)
}

fn record_registration() {
    let mut guard = REGISTER_ATTEMPTS.lock().unwrap();
    *guard = match *guard {
        Some((start, n)) if start.elapsed() < REGISTER_WINDOW => Some((start, n + 1)),
        _ => Some((Instant::now(), 1)),
    };
}

// ── Revocation ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RevokeForm {
    token: String,
    #[allow(dead_code)]
    token_type_hint: Option<String>,
}

async fn revoke(State(state): State<AppState>, Form(form): Form<RevokeForm>) -> Response {
    let token = form.token.trim();
    let result = if store::looks_like_refresh_token(token) {
        store::revoke_refresh_token(&state.pool, token).await
    } else {
        store::revoke_access_token(&state.pool, token).await
    };
    match result {
        Ok(_) => json_response(StatusCode::OK, json!({})),
        Err(e) => {
            tracing::error!("oauth revoke failed: {e:#}");
            oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not revoke the token")
        }
    }
}

// ── User-facing grants (session-protected) ───────────────────────────────────

async fn list_grants(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<otw_store::User>,
) -> Result<Json<Value>, ApiError> {
    let grants = store::list_grants(&state.pool, user.id).await?;
    Ok(Json(json!({ "grants": grants })))
}

async fn revoke_grant(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<otw_store::User>,
    Path(client_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let revoked = store::revoke_user_client(&state.pool, user.id, &client_id).await?;
    tracing::info!(
        "oauth grant revoked by user: user_id={} client_id={} tokens={}",
        user.id,
        client_id,
        revoked
    );
    Ok(Json(json!({ "ok": true, "revoked": revoked })))
}

// ── PKCE / validation helpers ────────────────────────────────────────────────

fn pkce_s256(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn valid_pkce_challenge(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn valid_code_verifier(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

fn valid_state(s: &str) -> bool {
    s.len() <= 1024 && !s.chars().any(|c| c.is_control())
}

pub(crate) fn valid_redirect_uri(raw: &str) -> bool {
    if raw.len() > 2048 {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if url.fragment().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]";
    match url.scheme() {
        "https" => true,
        "http" => loopback,
        _ => false,
    }
}

fn parse_scope_list(raw: &str) -> Result<Vec<String>, String> {
    let mut scopes: Vec<String> = raw
        .split_whitespace()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect();
    if scopes.is_empty() {
        scopes.push(SCOPE_MCP_READ.to_string());
    }
    for s in &scopes {
        if !store::SUPPORTED_SCOPES.contains(&s.as_str()) {
            return Err(format!("unsupported scope: {s}"));
        }
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

fn parse_scopes(raw: Option<&str>, client: &OAuthClient) -> Result<Vec<String>, String> {
    let mut scopes = parse_scope_list(raw.unwrap_or(SCOPE_MCP_READ))?;
    for s in &scopes {
        if !client.allows_scope(s) {
            return Err(format!("scope was not registered for this client: {s}"));
        }
    }
    if !scopes.iter().any(|s| s == SCOPE_MCP_READ) {
        return Err("mcp:read is required".into());
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

// ── Response helpers ─────────────────────────────────────────────────────────

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    json_response(
        status,
        json!({ "error": error, "error_description": description }),
    )
}

fn html_error(status: StatusCode, title: &str, message: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <title>{title}</title>{style}</head><body><main><h1>{title}</h1>\
         <p>{message}</p><p><a href=\"/login\">Open OpenTraderWorld</a></p></main></body></html>",
        title = esc(title),
        message = esc(message),
        style = PAGE_STYLE,
    );
    html_response(status, body)
}

fn html_response(status: StatusCode, body: String) -> Response {
    let mut resp = Html(body).into_response();
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    resp.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

fn redirect_with(uri: &str, params: &[(&str, String)]) -> Response {
    let Ok(mut url) = reqwest::Url::parse(uri) else {
        return html_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid redirect",
            "The registered redirect URI could not be built.",
        );
    };
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    let mut resp = Redirect::to(url.as_str()).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn redirect_error(
    base: &str,
    redirect_uri: &str,
    state: Option<&str>,
    error: &str,
    description: &str,
) -> Response {
    let mut params: Vec<(&str, String)> = vec![
        ("error", error.to_string()),
        ("error_description", description.to_string()),
        ("iss", base.to_string()),
    ];
    if let Some(s) = state {
        params.push(("state", s.to_string()));
    }
    redirect_with(redirect_uri, &params)
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const PAGE_STYLE: &str = "<style>\
    :root{color-scheme:light dark}\
    body{margin:0;font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;background:#0b0e14;color:#e6e6e6}\
    main{max-width:560px;margin:8vh auto;padding:28px;background:#151a23;border:1px solid #2a3242;border-radius:14px;box-shadow:0 10px 40px rgba(0,0,0,.35)}\
    h1{font-size:1.35rem;margin:0 0 8px}\
    p{margin:10px 0;color:#b9c2d0}\
    .box{padding:12px 14px;background:#0f141c;border:1px solid #2a3242;border-radius:10px;margin:14px 0}\
    .k{color:#8b96a8;font-size:.82rem;text-transform:uppercase;letter-spacing:.04em}\
    .v{color:#e6e6e6;word-break:break-word}\
    .scopes{display:flex;flex-wrap:wrap;gap:6px;margin-top:6px}\
    .scope{padding:2px 8px;border:1px solid #35506e;border-radius:999px;color:#9cc7ff;font-size:.85rem}\
    .actions{display:flex;gap:10px;margin-top:20px}\
    button{font:inherit;padding:10px 18px;border-radius:9px;border:1px solid #2a3242;cursor:pointer}\
    button.allow{background:#1f6feb;border-color:#1f6feb;color:#fff}\
    button.deny{background:#202634;color:#e6e6e6}\
    a{color:#9cc7ff}\
    </style>";

fn consent_html(
    username: &str,
    client: &OAuthClient,
    scopes: &[String],
    resource: &str,
    pending_id: uuid::Uuid,
    csrf: &str,
) -> String {
    let scope_tags: String = scopes
        .iter()
        .map(|s| format!("<span class=\"scope\">{}</span>", esc(s)))
        .collect();
    let permissions = if scopes.iter().any(|s| s == SCOPE_MCP_PAPER_TRADE) {
        "This grants read access to OTW data plus write access only to isolated Paper Trading accounts. It cannot change normal Portfolio holdings or send broker orders."
    } else {
        "This grants read-only access. It cannot write, delete, trade, or change your holdings."
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <title>Authorize {client}</title>{style}</head><body><main>\
         <h1>Authorize {client}</h1>\
         <p><strong>{client}</strong> wants access to your OpenTraderWorld data.</p>\
         <div class=\"box\"><div class=\"k\">Signed in as</div><div class=\"v\">{user}</div></div>\
         <div class=\"box\"><div class=\"k\">MCP resource</div><div class=\"v\">{resource}</div></div>\
         <div class=\"box\"><div class=\"k\">Scopes</div><div class=\"scopes\">{scopes}</div></div>\
         <p>{permissions}</p>\
         <form method=\"post\" action=\"/api/oauth/authorize/decision\" autocomplete=\"off\">\
         <input type=\"hidden\" name=\"request_id\" value=\"{pending}\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
         <div class=\"actions\">\
         <button class=\"allow\" type=\"submit\" name=\"decision\" value=\"allow\">Allow requested access</button>\
         <button class=\"deny\" type=\"submit\" name=\"decision\" value=\"deny\">Deny</button>\
         </div></form>\
         <p>Only the OpenTraderWorld owner account can authorize this connector. \
         You can revoke it later from the OAuth grant API or the host CLI.</p>\
         </main></body></html>",
        client = esc(&client.client_name),
        user = esc(username),
        resource = esc(resource),
        scopes = scope_tags,
        permissions,
        pending = pending_id,
        csrf = esc(csrf),
        style = PAGE_STYLE,
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_s256(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn redirect_uri_policy_is_strict() {
        assert!(valid_redirect_uri("https://chatgpt.com/connector_platform_oauth_redirect"));
        assert!(valid_redirect_uri("http://127.0.0.1:1455/callback"));
        assert!(valid_redirect_uri("http://localhost:3000/cb"));
        assert!(!valid_redirect_uri("http://example.com/cb"));
        assert!(!valid_redirect_uri("https://example.com/cb#frag"));
        assert!(!valid_redirect_uri("https://user:pass@example.com/cb"));
        assert!(!valid_redirect_uri("javascript:alert(1)"));
    }

    #[test]
    fn public_base_requires_https_or_loopback() {
        assert_eq!(
            parse_public_base("https://nova.lumora-lab.top/"),
            Some("https://nova.lumora-lab.top".into())
        );
        assert_eq!(
            parse_public_base("http://127.0.0.1:5454"),
            Some("http://127.0.0.1:5454".into())
        );
        assert_eq!(parse_public_base("http://example.com"), None);
        assert_eq!(parse_public_base("https://example.com/path"), None);
        assert_eq!(parse_public_base("https://example.com?x=1"), None);
    }

    #[test]
    fn scopes_reject_unknown_values() {
        assert_eq!(
            parse_scope_list("mcp:read offline_access").unwrap(),
            vec!["mcp:read".to_string(), "offline_access".to_string()]
        );
        assert!(parse_scope_list("mcp:write").is_err());
        assert!(parse_scope_list("admin").is_err());
    }

    #[test]
    fn paper_trade_scope_is_supported_but_read_scope_remains_required() {
        let scopes = parse_scope_list("mcp:read mcp:paper-trade").unwrap();
        assert!(scopes.iter().any(|s| s == SCOPE_MCP_READ));
        assert!(scopes.iter().any(|s| s == SCOPE_MCP_PAPER_TRADE));

        let client = OAuthClient {
            id: uuid::Uuid::nil(),
            client_id: "test-client".into(),
            client_name: "Test".into(),
            redirect_uris: vec![],
            grant_types: vec![],
            response_types: vec![],
            token_endpoint_auth_method: "none".into(),
            scopes: vec![SCOPE_MCP_READ.into(), SCOPE_MCP_PAPER_TRADE.into()],
            source: "test".into(),
            created_at: OffsetDateTime::now_utc(),
            last_used_at: None,
            disabled_at: None,
        };
        assert!(parse_scopes(Some("mcp:paper-trade"), &client).is_err());
        assert!(parse_scopes(Some("mcp:read mcp:paper-trade"), &client).is_ok());
    }

    #[test]
    fn html_escaping_covers_dynamic_consent_values() {
        assert_eq!(esc("<script>\"x\"&'"), "&lt;script&gt;&quot;x&quot;&amp;&#39;");
    }
}
