-- Independent API tokens for external AI agents (Phase 1: agent:ping).
--
-- These are deliberately separate from browser sessions (opaque cookie, table
-- `sessions`) and from MCP gateway tokens (`mcp_tokens`, module-level permissions):
--   * every token belongs to exactly one OTW user;
--   * every token carries an explicit scope list (`agent:ping` today);
--   * the bearer value is high-entropy and only its SHA-256 is stored;
--   * a token can expire and can be revoked without deleting its audit row.
--
-- A token is NEVER accepted as a session cookie and a session cookie is NEVER accepted
-- as an API token: the two credentials live in different tables and are read from
-- different transports (Authorization header vs. HttpOnly cookie).
CREATE TABLE IF NOT EXISTS agent_api_tokens (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Stable principal label returned by GET /api/agent/ping (e.g. "chatgpt").
    agent        TEXT NOT NULL,
    -- Operator-facing label for listing/revocation.
    name         TEXT NOT NULL,
    -- SHA-256 hex of the full bearer token. The plaintext exists only in the
    -- create command's stdout and is never persisted.
    token_hash   TEXT NOT NULL UNIQUE,
    -- Display-only leading characters; never enough to authenticate.
    prefix       TEXT NOT NULL,
    -- Explicit scopes, e.g. {agent:ping}. Unknown scopes are rejected at creation.
    scopes       TEXT[] NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    -- NULL = never expires.
    expires_at   TIMESTAMPTZ,
    -- NULL = active. Revocation is a timestamp so the row stays auditable.
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_agent_api_tokens_user
    ON agent_api_tokens (user_id);
