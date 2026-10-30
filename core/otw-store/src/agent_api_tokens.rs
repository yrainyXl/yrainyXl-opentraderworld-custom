//! Independent API tokens for external AI agents.
//!
//! A token is bound to one OTW user and carries an explicit scope list. Only the
//! SHA-256 of the bearer value is stored; the plaintext is returned exactly once by the
//! creation path (the host CLI) and never persisted or logged.
//!
//! This is intentionally a different credential from:
//! * browser sessions — opaque cookie tokens in `sessions`, read from a cookie only;
//! * MCP tokens — `mcp_tokens`, module-level permissions, read from `POST /api/mcp`.
//!
//! The distinct table and the distinct `otw_agent_` prefix keep the three from being
//! accepted interchangeably. Lookup is a direct indexed hash match: the token itself is
//! 256 bits of OS entropy, so no per-request KDF is needed (unlike a password).
//!
//! Expiry and revocation are checked by the caller AFTER the row is found, so an expired
//! or revoked token can be told apart from a wrong one. That distinction is visible only
//! to someone already holding the matching bearer value, so it leaks nothing useful.

use anyhow::Context;
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::mcp::hash_token;

/// Bearer value shape: `otw_agent_` + 64 hex characters (256 bits of entropy).
pub const TOKEN_PREFIX: &str = "otw_agent_";

/// The scope that allows `GET /api/agent/ping`.
pub const SCOPE_AGENT_PING: &str = "agent:ping";

/// Scopes this build understands. Rejecting anything else at creation means a typo can
/// never silently widen (or dead-letter) a token's access. Widening access requires both
/// a new entry here and an enforcement point in the handler.
pub const SUPPORTED_SCOPES: &[&str] = &[SCOPE_AGENT_PING];

/// A persisted token row. The hash is deliberately not part of this struct: callers of
/// the listing/ping paths have no reason to handle it, and `#[derive(Serialize)]` on it
/// would make an accidental DB-hash leak into an API response one field away.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent: String,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    /// When the token stops being accepted; `None` = never.
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// When the token was revoked; `None` = active.
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

impl AgentApiToken {
    /// Checked on every request, not at listing: a token that expires mid-session must
    /// stop working there and then.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|at| at <= OffsetDateTime::now_utc())
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Cheap shape check before touching the database. Also guarantees a session token
/// (64 raw hex) or an MCP token (`otw_mcp_…`) can never match this credential type.
pub fn looks_like_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix(TOKEN_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Mint a new token: returns the row and the plaintext (shown once, never stored).
pub async fn create_token(
    pool: &PgPool,
    user_id: Uuid,
    agent: &str,
    name: &str,
    scopes: &[String],
    expires_at: Option<OffsetDateTime>,
) -> anyhow::Result<(AgentApiToken, String)> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token = format!("{TOKEN_PREFIX}{hex}");
    let prefix: String = token.chars().take(16).collect();

    let row = sqlx::query_as::<_, AgentApiToken>(
        "INSERT INTO agent_api_tokens \
             (id, user_id, agent, name, token_hash, prefix, scopes, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, user_id, agent, name, prefix, scopes, created_at, last_used_at, \
                   expires_at, revoked_at",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(agent)
    .bind(name)
    .bind(hash_token(&token))
    .bind(&prefix)
    .bind(scopes.to_vec())
    .bind(expires_at)
    .fetch_one(pool)
    .await
    .context("creating agent api token")?;

    Ok((row, token))
}

/// Tokens owned by one user, newest first. Metadata only — never the hash or plaintext.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> anyhow::Result<Vec<AgentApiToken>> {
    sqlx::query_as(
        "SELECT id, user_id, agent, name, prefix, scopes, created_at, last_used_at, \
                expires_at, revoked_at \
         FROM agent_api_tokens WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing agent api tokens")
}

/// Revoke a token. Idempotent: revoking an already-revoked token keeps the first
/// timestamp. Returns `None` when the id does not exist.
pub async fn revoke(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<AgentApiToken>> {
    sqlx::query_as(
        "UPDATE agent_api_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE id = $1 \
         RETURNING id, user_id, agent, name, prefix, scopes, created_at, last_used_at, \
                   expires_at, revoked_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("revoking agent api token")
}

/// Resolve a presented bearer token to its row by hash. Read-only: expiry/revocation are
/// judged by the caller, and `last_used_at` is touched only after all checks pass, so a
/// rejected request leaves no trace beyond the lookup itself.
pub async fn find_by_token(
    pool: &PgPool,
    token: &str,
) -> anyhow::Result<Option<AgentApiToken>> {
    sqlx::query_as(
        "SELECT id, user_id, agent, name, prefix, scopes, created_at, last_used_at, \
                expires_at, revoked_at \
         FROM agent_api_tokens WHERE token_hash = $1",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await
    .context("resolving agent api token")
}

/// Record a successful use. Failure is not fatal for the caller (the token is still
/// valid); the handler logs a warning instead of failing the request.
pub async fn touch_last_used(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE agent_api_tokens SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("touching agent api token last_used_at")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> AgentApiToken {
        AgentApiToken {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            agent: "chatgpt".into(),
            name: "test".into(),
            prefix: "otw_agent_000000".into(),
            scopes: vec![SCOPE_AGENT_PING.to_string()],
            created_at: OffsetDateTime::now_utc(),
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn token_shape_accepts_only_agent_tokens() {
        let valid = format!("{TOKEN_PREFIX}{}", "a".repeat(64));
        assert!(looks_like_token(&valid));

        // Browser session tokens are 64 raw hex characters.
        assert!(!looks_like_token(&"a".repeat(64)));
        // MCP tokens have their own prefix and would never reach this table anyway.
        assert!(!looks_like_token(&format!("otw_mcp_{}", "a".repeat(64))));
        assert!(!looks_like_token(&format!("{TOKEN_PREFIX}{}", "a".repeat(63))));
        assert!(!looks_like_token(&format!("{TOKEN_PREFIX}{}", "g".repeat(64))));
        assert!(!looks_like_token(""));
    }

    #[test]
    fn scope_membership_is_exact() {
        let t = token();
        assert!(t.has_scope(SCOPE_AGENT_PING));
        assert!(!t.has_scope("agent:write"));
        assert!(!t.has_scope("agent:pin"));
    }

    #[test]
    fn expiry_and_revocation_are_orthogonal() {
        let mut t = token();
        assert!(!t.is_expired());
        assert!(!t.is_revoked());

        t.expires_at = Some(OffsetDateTime::now_utc() - time::Duration::seconds(1));
        assert!(t.is_expired());
        assert!(!t.is_revoked());

        t.revoked_at = Some(OffsetDateTime::now_utc());
        assert!(t.is_revoked());
    }
}
