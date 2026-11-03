//! OAuth 2.1 state for the MCP endpoint (ChatGPT + Codex).
//!
//! The authorization server is hosted by OTW itself. Clients are public: they use the
//! authorization-code flow with PKCE S256 and no client secret. Every value that can be
//! replayed as a credential — authorization codes, access tokens, refresh tokens, consent
//! CSRF tokens — is 256 bits of OS entropy and only its SHA-256 is stored.
//!
//! Refresh tokens rotate on every use. Each token belongs to a `family_id`; presenting a
//! token whose `used_at` is already set is treated as a replay and revokes the entire
//! family (both refresh and access tokens).
//!
//! All rows are bound to a real `users.id`. OTW is currently single-user, so the
//! authorization endpoint only admits admin accounts; this module does not attempt to
//! invent per-portfolio ownership that the schema does not have.

use anyhow::Context;
use serde::Serialize;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::mcp::hash_token;

pub const CLIENT_PREFIX: &str = "otw_oc_";
pub const CODE_PREFIX: &str = "otw_oac_";
pub const ACCESS_PREFIX: &str = "otw_oat_";
pub const REFRESH_PREFIX: &str = "otw_ort_";

pub const SCOPE_MCP_READ: &str = "mcp:read";
pub const SCOPE_MCP_PAPER_TRADE: &str = "mcp:paper-trade";
pub const SCOPE_OFFLINE_ACCESS: &str = "offline_access";

pub const ACCESS_TTL_SECS: i64 = 15 * 60;
pub const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
pub const CODE_TTL_SECS: i64 = 5 * 60;
pub const PENDING_TTL_SECS: i64 = 10 * 60;

/// Scopes this authorization server accepts. Paper Trading is a separate opt-in scope;
/// it never grants write access to the normal Portfolio module.
pub const SUPPORTED_SCOPES: &[&str] = &[SCOPE_MCP_READ, SCOPE_MCP_PAPER_TRADE, SCOPE_OFFLINE_ACCESS];

pub fn random_hex(bytes: usize) -> anyhow::Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn mint(prefix: &str, bytes: usize) -> anyhow::Result<String> {
    Ok(format!("{prefix}{}", random_hex(bytes)?))
}

pub fn looks_like_access_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix(ACCESS_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn looks_like_refresh_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix(REFRESH_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

// ── Clients ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OAuthClient {
    pub id: Uuid,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub scopes: Vec<String>,
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
}

impl OAuthClient {
    pub fn is_disabled(&self) -> bool {
        self.disabled_at.is_some()
    }

    pub fn allows_redirect(&self, uri: &str) -> bool {
        self.redirect_uris.iter().any(|u| u == uri)
    }

    pub fn allows_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

pub async fn create_client(
    pool: &PgPool,
    name: &str,
    redirect_uris: &[String],
    scopes: &[String],
    source: &str,
) -> anyhow::Result<(OAuthClient, String)> {
    let client_id = mint(CLIENT_PREFIX, 16)?;
    let row = sqlx::query_as::<_, OAuthClient>(
        "INSERT INTO oauth_clients \
             (id, client_id, client_name, redirect_uris, scopes, source) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, client_id, client_name, redirect_uris, grant_types, response_types, \
                   token_endpoint_auth_method, scopes, source, created_at, last_used_at, disabled_at",
    )
    .bind(Uuid::new_v4())
    .bind(&client_id)
    .bind(name)
    .bind(redirect_uris.to_vec())
    .bind(scopes.to_vec())
    .bind(source)
    .fetch_one(pool)
    .await
    .context("creating oauth client")?;
    Ok((row, client_id))
}

pub async fn find_client(pool: &PgPool, client_id: &str) -> anyhow::Result<Option<OAuthClient>> {
    sqlx::query_as(
        "SELECT id, client_id, client_name, redirect_uris, grant_types, response_types, \
                token_endpoint_auth_method, scopes, source, created_at, last_used_at, disabled_at \
         FROM oauth_clients WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await
    .context("finding oauth client")
}

pub async fn find_client_by_uuid(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<Option<OAuthClient>> {
    sqlx::query_as(
        "SELECT id, client_id, client_name, redirect_uris, grant_types, response_types, \
                token_endpoint_auth_method, scopes, source, created_at, last_used_at, disabled_at \
         FROM oauth_clients WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("finding oauth client by id")
}

pub async fn list_clients(pool: &PgPool) -> anyhow::Result<Vec<OAuthClient>> {
    sqlx::query_as(
        "SELECT id, client_id, client_name, redirect_uris, grant_types, response_types, \
                token_endpoint_auth_method, scopes, source, created_at, last_used_at, disabled_at \
         FROM oauth_clients ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
    .context("listing oauth clients")
}

pub async fn count_clients(pool: &PgPool) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM oauth_clients")
        .fetch_one(pool)
        .await
        .context("counting oauth clients")?;
    Ok(n)
}

pub async fn delete_client(pool: &PgPool, client_id: &str) -> anyhow::Result<bool> {
    let res = sqlx::query("DELETE FROM oauth_clients WHERE client_id = $1")
        .bind(client_id)
        .execute(pool)
        .await
        .context("deleting oauth client")?;
    Ok(res.rows_affected() > 0)
}

pub async fn touch_client(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE oauth_clients SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("touching oauth client")?;
    Ok(())
}

// ── Pending authorization requests (consent page state) ──────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthPending {
    pub id: Uuid,
    pub user_id: Uuid,
    pub client_id: Uuid,
    pub redirect_uri: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub csrf_hash: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

pub async fn create_pending(
    pool: &PgPool,
    user_id: Uuid,
    client_id: Uuid,
    redirect_uri: &str,
    resource: &str,
    scopes: &[String],
    state: Option<&str>,
    code_challenge: &str,
    code_challenge_method: &str,
    csrf_hash: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(PENDING_TTL_SECS);
    sqlx::query(
        "INSERT INTO oauth_pending_requests \
             (id, user_id, client_id, redirect_uri, resource, scopes, state, code_challenge, \
              code_challenge_method, csrf_hash, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(user_id)
    .bind(client_id)
    .bind(redirect_uri)
    .bind(resource)
    .bind(scopes.to_vec())
    .bind(state)
    .bind(code_challenge)
    .bind(code_challenge_method)
    .bind(csrf_hash)
    .bind(expires_at)
    .execute(pool)
    .await
    .context("creating oauth pending request")?;
    Ok(id)
}

/// Atomically consume a pending request: delete it only if it belongs to `user_id`, the
/// CSRF hash matches and it has not expired. Returns the row for the redirect decision.
pub async fn consume_pending(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
    csrf_hash: &str,
) -> anyhow::Result<Option<OAuthPending>> {
    sqlx::query_as(
        "DELETE FROM oauth_pending_requests \
         WHERE id = $1 AND user_id = $2 AND csrf_hash = $3 AND expires_at > now() \
         RETURNING id, user_id, client_id, redirect_uri, resource, scopes, state, \
                   code_challenge, code_challenge_method, csrf_hash, created_at, expires_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(csrf_hash)
    .fetch_optional(pool)
    .await
    .context("consuming oauth pending request")
}

// ── Authorization codes ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCode {
    pub id: Uuid,
    pub client_id: Uuid,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    Invalid,
    Expired,
    Reused,
    ClientMismatch,
    RedirectMismatch,
    ResourceMismatch,
}

pub async fn create_code(
    pool: &PgPool,
    pending: &OAuthPending,
) -> anyhow::Result<String> {
    let code = mint(CODE_PREFIX, 32)?;
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(CODE_TTL_SECS);
    sqlx::query(
        "INSERT INTO oauth_authorization_codes \
             (id, code_hash, client_id, user_id, redirect_uri, resource, scopes, \
              code_challenge, code_challenge_method, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(Uuid::new_v4())
    .bind(hash_token(&code))
    .bind(pending.client_id)
    .bind(pending.user_id)
    .bind(&pending.redirect_uri)
    .bind(&pending.resource)
    .bind(pending.scopes.clone())
    .bind(&pending.code_challenge)
    .bind(&pending.code_challenge_method)
    .bind(expires_at)
    .execute(pool)
    .await
    .context("creating oauth authorization code")?;
    Ok(code)
}

/// Consume an authorization code. The row is marked used inside the same transaction that
/// validates it, so two concurrent exchanges cannot both succeed.
pub async fn consume_code(
    pool: &PgPool,
    code: &str,
    client_id: Uuid,
    redirect_uri: &str,
    resource: &str,
) -> anyhow::Result<Result<OAuthCode, CodeError>> {
    let mut tx = pool.begin().await.context("begin code exchange")?;
    let row: Option<(
        Uuid,
        Uuid,
        Uuid,
        String,
        String,
        Vec<String>,
        String,
        String,
        Option<OffsetDateTime>,
        OffsetDateTime,
    )> = sqlx::query_as(
        "SELECT id, client_id, user_id, redirect_uri, resource, scopes, code_challenge, \
                code_challenge_method, used_at, expires_at \
         FROM oauth_authorization_codes WHERE code_hash = $1 FOR UPDATE",
    )
    .bind(hash_token(code))
    .fetch_optional(&mut *tx)
    .await
    .context("loading oauth authorization code")?;

    let Some((id, code_client, user_id, code_redirect, code_resource, scopes, challenge, method, used_at, expires_at)) = row
    else {
        tx.commit().await.ok();
        return Ok(Err(CodeError::Invalid));
    };

    let now = OffsetDateTime::now_utc();
    let outcome = if used_at.is_some() {
        Err(CodeError::Reused)
    } else if expires_at <= now {
        Err(CodeError::Expired)
    } else if code_client != client_id {
        Err(CodeError::ClientMismatch)
    } else if code_redirect != redirect_uri {
        Err(CodeError::RedirectMismatch)
    } else if code_resource != resource {
        Err(CodeError::ResourceMismatch)
    } else {
        Ok(OAuthCode {
            id,
            client_id: code_client,
            user_id,
            redirect_uri: code_redirect,
            resource: code_resource,
            scopes,
            code_challenge: challenge,
            code_challenge_method: method,
        })
    };

    if outcome.is_ok() {
        sqlx::query("UPDATE oauth_authorization_codes SET used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("marking oauth code used")?;
    }
    tx.commit().await.context("commit code exchange")?;
    Ok(outcome)
}

// ── Tokens ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OAuthAccessToken {
    pub id: Uuid,
    pub client_id: Uuid,
    pub client_public_id: String,
    pub client_name: String,
    pub client_disabled_at: Option<OffsetDateTime>,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub resource: String,
    pub scopes: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

impl OAuthAccessToken {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= OffsetDateTime::now_utc()
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: OffsetDateTime,
    pub refresh_expires_at: OffsetDateTime,
    pub scopes: Vec<String>,
}

async fn insert_access_and_refresh(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    resource: &str,
    scopes: &[String],
) -> anyhow::Result<IssuedTokens> {
    let access_token = mint(ACCESS_PREFIX, 32)?;
    let refresh_token = mint(REFRESH_PREFIX, 32)?;
    let access_expires_at = OffsetDateTime::now_utc() + Duration::seconds(ACCESS_TTL_SECS);
    let refresh_expires_at = OffsetDateTime::now_utc() + Duration::seconds(REFRESH_TTL_SECS);
    sqlx::query(
        "INSERT INTO oauth_access_tokens \
             (id, token_hash, client_id, user_id, family_id, resource, scopes, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::new_v4())
    .bind(hash_token(&access_token))
    .bind(client_id)
    .bind(user_id)
    .bind(family_id)
    .bind(resource)
    .bind(scopes.to_vec())
    .bind(access_expires_at)
    .execute(&mut **tx)
    .await
    .context("inserting oauth access token")?;
    sqlx::query(
        "INSERT INTO oauth_refresh_tokens \
             (id, token_hash, client_id, user_id, family_id, resource, scopes, expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::new_v4())
    .bind(hash_token(&refresh_token))
    .bind(client_id)
    .bind(user_id)
    .bind(family_id)
    .bind(resource)
    .bind(scopes.to_vec())
    .bind(refresh_expires_at)
    .execute(&mut **tx)
    .await
    .context("inserting oauth refresh token")?;
    Ok(IssuedTokens {
        access_token,
        refresh_token,
        access_expires_at,
        refresh_expires_at,
        scopes: scopes.to_vec(),
    })
}

pub async fn issue_tokens(
    pool: &PgPool,
    client_id: Uuid,
    user_id: Uuid,
    resource: &str,
    scopes: &[String],
) -> anyhow::Result<IssuedTokens> {
    let mut tx = pool.begin().await.context("begin token issue")?;
    let family_id = Uuid::new_v4();
    let issued = insert_access_and_refresh(
        &mut tx,
        client_id,
        user_id,
        family_id,
        resource,
        scopes,
    )
    .await?;
    tx.commit().await.context("commit token issue")?;
    Ok(issued)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshError {
    Invalid,
    Expired,
    Replay,
    ClientMismatch,
    ResourceMismatch,
}

/// Rotate a refresh token. On replay (the token was already used) the whole family is
/// revoked before the error is returned.
pub async fn rotate_refresh(
    pool: &PgPool,
    refresh_token: &str,
    client_id: Uuid,
    resource: &str,
) -> anyhow::Result<Result<IssuedTokens, RefreshError>> {
    let mut tx = pool.begin().await.context("begin refresh rotation")?;
    let row: Option<(
        Uuid,
        Uuid,
        Uuid,
        Uuid,
        String,
        Vec<String>,
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
        OffsetDateTime,
    )> = sqlx::query_as(
        "SELECT id, client_id, user_id, family_id, resource, scopes, used_at, revoked_at, expires_at \
         FROM oauth_refresh_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(hash_token(refresh_token))
    .fetch_optional(&mut *tx)
    .await
    .context("loading oauth refresh token")?;

    let Some((id, token_client, user_id, family_id, token_resource, scopes, used_at, revoked_at, expires_at)) = row
    else {
        tx.commit().await.ok();
        return Ok(Err(RefreshError::Invalid));
    };

    let now = OffsetDateTime::now_utc();
    if used_at.is_some() {
        // Replay: burn the family, including every access token derived from it.
        sqlx::query(
            "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) \
             WHERE family_id = $1",
        )
        .bind(family_id)
        .execute(&mut *tx)
        .await
        .context("revoking refresh family on replay")?;
        sqlx::query(
            "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) \
             WHERE family_id = $1",
        )
        .bind(family_id)
        .execute(&mut *tx)
        .await
        .context("revoking access family on replay")?;
        tx.commit().await.context("commit replay revocation")?;
        return Ok(Err(RefreshError::Replay));
    }
    if revoked_at.is_some() {
        tx.commit().await.ok();
        return Ok(Err(RefreshError::Invalid));
    }
    if expires_at <= now {
        tx.commit().await.ok();
        return Ok(Err(RefreshError::Expired));
    }
    if token_client != client_id {
        tx.commit().await.ok();
        return Ok(Err(RefreshError::ClientMismatch));
    }
    if token_resource != resource {
        tx.commit().await.ok();
        return Ok(Err(RefreshError::ResourceMismatch));
    }

    sqlx::query("UPDATE oauth_refresh_tokens SET used_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("marking refresh token used")?;
    let issued = insert_access_and_refresh(
        &mut tx,
        token_client,
        user_id,
        family_id,
        &token_resource,
        &scopes,
    )
    .await?;
    tx.commit().await.context("commit refresh rotation")?;
    Ok(Ok(issued))
}

pub async fn find_access_token(
    pool: &PgPool,
    token: &str,
) -> anyhow::Result<Option<OAuthAccessToken>> {
    sqlx::query_as(
        "SELECT t.id, t.client_id, c.client_id AS client_public_id, c.client_name, \
                c.disabled_at AS client_disabled_at, t.user_id, \
                t.family_id, t.resource, t.scopes, t.created_at, t.expires_at, t.revoked_at, \
                t.last_used_at \
         FROM oauth_access_tokens t JOIN oauth_clients c ON c.id = t.client_id \
         WHERE t.token_hash = $1",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await
    .context("finding oauth access token")
}

pub async fn touch_access_token(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE oauth_access_tokens SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("touching oauth access token")?;
    Ok(())
}

/// Revoke an access token (RFC 7009). Only that token; the refresh token stays valid.
pub async fn revoke_access_token(pool: &PgPool, token: &str) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE token_hash = $1",
    )
    .bind(hash_token(token))
    .execute(pool)
    .await
    .context("revoking oauth access token")?;
    Ok(res.rows_affected() > 0)
}

/// Revoke a refresh token and its whole family (the standard "log out this client" action).
pub async fn revoke_refresh_token(pool: &PgPool, token: &str) -> anyhow::Result<bool> {
    let family: Option<(Uuid,)> = sqlx::query_as(
        "SELECT family_id FROM oauth_refresh_tokens WHERE token_hash = $1",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await
    .context("finding oauth refresh token for revocation")?;
    let Some((family_id,)) = family else {
        return Ok(false);
    };
    revoke_family(pool, family_id).await?;
    Ok(true)
}

pub async fn revoke_family(pool: &PgPool, family_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE family_id = $1",
    )
    .bind(family_id)
    .execute(pool)
    .await
    .context("revoking refresh family")?;
    sqlx::query(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE family_id = $1",
    )
    .bind(family_id)
    .execute(pool)
    .await
    .context("revoking access family")?;
    Ok(())
}

/// Revoke every grant a user has for one client. Used by the user-facing revoke API.
pub async fn revoke_user_client(
    pool: &PgPool,
    user_id: Uuid,
    client_public_id: &str,
) -> anyhow::Result<u64> {
    let Some(client) = find_client(pool, client_public_id).await? else {
        return Ok(0);
    };
    let mut tx = pool.begin().await.context("begin user-client revocation")?;
    let a = sqlx::query(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE user_id = $1 AND client_id = $2",
    )
    .bind(user_id)
    .bind(client.id)
    .execute(&mut *tx)
    .await
    .context("revoking access tokens for user/client")?;
    let r = sqlx::query(
        "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE user_id = $1 AND client_id = $2",
    )
    .bind(user_id)
    .bind(client.id)
    .execute(&mut *tx)
    .await
    .context("revoking refresh tokens for user/client")?;
    sqlx::query("DELETE FROM oauth_authorization_codes WHERE user_id = $1 AND client_id = $2")
        .bind(user_id)
        .bind(client.id)
        .execute(&mut *tx)
        .await
        .context("deleting codes for user/client")?;
    sqlx::query("DELETE FROM oauth_pending_requests WHERE user_id = $1 AND client_id = $2")
        .bind(user_id)
        .bind(client.id)
        .execute(&mut *tx)
        .await
        .context("deleting pending requests for user/client")?;
    tx.commit().await.context("commit user-client revocation")?;
    Ok(a.rows_affected() + r.rows_affected())
}

/// Revoke all OAuth grants for a user. Called when the account password changes or is reset,
/// so a stolen refresh token cannot outlive the credential it belonged to.
pub async fn revoke_all_for_user(pool: &PgPool, user_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .context("revoking all oauth access tokens")?;
    sqlx::query(
        "UPDATE oauth_refresh_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .context("revoking all oauth refresh tokens")?;
    sqlx::query("DELETE FROM oauth_authorization_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .context("deleting all oauth codes")?;
    sqlx::query("DELETE FROM oauth_pending_requests WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .context("deleting all oauth pending requests")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OAuthGrant {
    pub client_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub access_tokens: i64,
    pub refresh_tokens: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

pub async fn list_grants(pool: &PgPool, user_id: Uuid) -> anyhow::Result<Vec<OAuthGrant>> {
    sqlx::query_as(
        "SELECT c.client_id, c.client_name, c.scopes, \
                COALESCE(at.n, 0)::bigint AS access_tokens, \
                COALESCE(rt.n, 0)::bigint AS refresh_tokens, \
                at.last_used AS last_used_at \
         FROM oauth_clients c \
         LEFT JOIN ( \
             SELECT client_id, count(*) AS n, max(last_used_at) AS last_used \
             FROM oauth_access_tokens \
             WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now() \
             GROUP BY client_id \
         ) at ON at.client_id = c.id \
         LEFT JOIN ( \
             SELECT client_id, count(*) AS n \
             FROM oauth_refresh_tokens \
             WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now() \
             GROUP BY client_id \
         ) rt ON rt.client_id = c.id \
         WHERE at.client_id IS NOT NULL OR rt.client_id IS NOT NULL \
         ORDER BY c.client_name, c.client_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing oauth grants")
}

/// Best-effort housekeeping: expired consent requests and codes carry no audit value.
pub async fn cleanup(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM oauth_pending_requests WHERE expires_at < now() - interval '1 hour'")
        .execute(pool)
        .await
        .context("cleaning oauth pending requests")?;
    sqlx::query(
        "DELETE FROM oauth_authorization_codes WHERE expires_at < now() - interval '1 day'",
    )
    .execute(pool)
    .await
    .context("cleaning oauth codes")?;
    Ok(())
}
