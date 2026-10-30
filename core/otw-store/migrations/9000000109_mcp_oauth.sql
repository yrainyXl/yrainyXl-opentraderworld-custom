-- MCP OAuth 2.1 authorization server state (ChatGPT + Codex).
--
-- Design notes:
--   * Clients are public (PKCE, token_endpoint_auth_method = "none"). No client secrets
--     are stored; a client_id is an identifier, not a credential.
--   * Authorization codes, access tokens, refresh tokens and pending-request CSRF tokens
--     are 256-bit opaque values. Only their SHA-256 is stored; the plaintext exists only
--     in the HTTP response that mints it.
--   * Access tokens are short-lived (minutes). Refresh tokens rotate on every use and are
--     tracked by family so a replay can revoke the whole family.
--   * All rows are bound to a real OTW user. The current OTW data model is single-user,
--     so the authorization endpoint only admits admin accounts (see CUSTOM_CHANGES C-017).
--
-- This is additive: existing `mcp_tokens` (fixed bearer tokens for local Codex) and all
-- browser-session/agent-API credentials are untouched.

CREATE TABLE IF NOT EXISTS oauth_clients (
    id            UUID PRIMARY KEY,
    -- Public identifier presented by the client. Not a secret.
    client_id     TEXT NOT NULL UNIQUE,
    client_name   TEXT NOT NULL DEFAULT '',
    -- Exact-match allowlist. No wildcards, no fragments.
    redirect_uris TEXT[] NOT NULL,
    grant_types   TEXT[] NOT NULL DEFAULT '{authorization_code,refresh_token}',
    response_types TEXT[] NOT NULL DEFAULT '{code}',
    token_endpoint_auth_method TEXT NOT NULL DEFAULT 'none',
    scopes        TEXT[] NOT NULL DEFAULT '{mcp:read}',
    -- 'dcr' = dynamically registered, 'pre_registered' = created by the host CLI.
    source        TEXT NOT NULL DEFAULT 'dcr',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    disabled_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_oauth_clients_created
    ON oauth_clients (created_at);

-- A consent decision in progress. One row per authorize request; deleted when decided
-- (or expired). The CSRF token is stored hashed, like every other bearer-ish value.
CREATE TABLE IF NOT EXISTS oauth_pending_requests (
    id                    UUID PRIMARY KEY,
    user_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id             UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    redirect_uri          TEXT NOT NULL,
    resource              TEXT NOT NULL,
    scopes                TEXT[] NOT NULL,
    state                 TEXT,
    code_challenge        TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL,
    csrf_hash             TEXT NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at            TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_oauth_pending_user
    ON oauth_pending_requests (user_id, expires_at);

CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
    id                    UUID PRIMARY KEY,
    code_hash             TEXT NOT NULL UNIQUE,
    client_id             UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri          TEXT NOT NULL,
    resource              TEXT NOT NULL,
    scopes                TEXT[] NOT NULL,
    code_challenge        TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at            TIMESTAMPTZ NOT NULL,
    used_at               TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_oauth_codes_expiry
    ON oauth_authorization_codes (expires_at);

CREATE TABLE IF NOT EXISTS oauth_access_tokens (
    id           UUID PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE,
    client_id    UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Shared by a refresh-token family; replay revokes this whole family.
    family_id    UUID NOT NULL,
    resource     TEXT NOT NULL,
    scopes       TEXT[] NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_oauth_access_family
    ON oauth_access_tokens (family_id);
CREATE INDEX IF NOT EXISTS idx_oauth_access_user_client
    ON oauth_access_tokens (user_id, client_id);

CREATE TABLE IF NOT EXISTS oauth_refresh_tokens (
    id          UUID PRIMARY KEY,
    token_hash  TEXT NOT NULL UNIQUE,
    client_id   UUID NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    family_id   UUID NOT NULL,
    resource    TEXT NOT NULL,
    scopes      TEXT[] NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    -- Set the moment a rotation consumes this token. A second use is a replay.
    used_at     TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ,
    replaced_by UUID
);

CREATE INDEX IF NOT EXISTS idx_oauth_refresh_family
    ON oauth_refresh_tokens (family_id);
CREATE INDEX IF NOT EXISTS idx_oauth_refresh_user_client
    ON oauth_refresh_tokens (user_id, client_id);
