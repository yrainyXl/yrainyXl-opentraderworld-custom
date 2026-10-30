#!/usr/bin/env bash
#
# Smoke tests for the external-agent API: GET /api/agent/ping.
#
# The bearer values are supplied through the environment and are NEVER printed, echoed
# or written to disk. This script is safe to run in a CI log.
#
# Required:
#   OTW_BASE_URL         e.g. http://127.0.0.1:5454 (Caddy) or http://core:8080 in-network
#   OTW_AGENT_TOKEN      a live token with the agent:ping scope
#
# Optional (the matching check is skipped when the variable is unset):
#   OTW_AGENT_NOSCOPE_TOKEN   a live token whose scopes array does NOT contain agent:ping
#                             (create it as usual, then: UPDATE agent_api_tokens
#                              SET scopes = '{}' WHERE id = '<id>';)
#   OTW_AGENT_EXPIRED_TOKEN   a token whose expires_at is in the past
#   OTW_AGENT_REVOKED_TOKEN   a token revoked with `revoke-agent-token`
#   OTW_MCP_TOKEN             an MCP bearer token (must NOT be accepted here)
#   OTW_SESSION_COOKIE       a browser session cookie value (must NOT be accepted here)
#
# Example:
#   OTW_BASE_URL=http://127.0.0.1:5454 \
#   OTW_AGENT_TOKEN="$TOKEN" \
#   ./scripts/agent-api-smoke.sh
#
set -uo pipefail

BASE="${OTW_BASE_URL:-http://127.0.0.1:5454}"
BASE="${BASE%/}"
PING="$BASE/api/agent/ping"

if [ -z "${OTW_AGENT_TOKEN:-}" ]; then
    echo "error: set OTW_AGENT_TOKEN to a valid agent API token" >&2
    exit 2
fi

failures=0
passes=0

# request <expected_status> <label> <url> [bearer_token] [extra curl args...]
request() {
    local expected="$1" label="$2" url="$3" token="${4:-}"
    # Drop the named parameters so "$@" below is only the optional extra curl args.
    # (A bare `shift 4` fails when the caller omitted the optional token.)
    if [ "$#" -ge 4 ]; then
        shift 4
    else
        shift 3
    fi
    local body status
    body="$(mktemp)" || { echo "error: mktemp failed" >&2; exit 2; }

    if [ -n "$token" ]; then
        status="$(curl -sS --max-time 15 -o "$body" -w '%{http_code}' \
            -H "Authorization: Bearer $token" "$@" "$url" 2>/dev/null)"
    else
        status="$(curl -sS --max-time 15 -o "$body" -w '%{http_code}' \
            "$@" "$url" 2>/dev/null)"
    fi
    # curl itself failed (no HTTP response): normalize to 000.
    [ -n "$status" ] || status="000"

    if [ "$status" = "$expected" ]; then
        echo "PASS  $label -> $status"
        passes=$((passes + 1))
    else
        echo "FAIL  $label -> $status (expected $expected)"
        echo "      body: $(tr '\n' ' ' < "$body" | cut -c1-300)"
        failures=$((failures + 1))
    fi
    rm -f "$body"
    return 0
}

echo "Agent API smoke tests against $BASE"
echo

# 1. Valid token succeeds and returns the expected business fields.
valid_body="$(mktemp)"
valid_status="$(curl -sS --max-time 15 -o "$valid_body" -w '%{http_code}' \
    -H "Authorization: Bearer $OTW_AGENT_TOKEN" "$PING" 2>/dev/null)"
[ -n "$valid_status" ] || valid_status="000"
if [ "$valid_status" = "200" ] \
    && grep -Eq '"status"[[:space:]]*:[[:space:]]*"ok"' "$valid_body" \
    && grep -Eq '"agent"[[:space:]]*:[[:space:]]*"[^"]+"' "$valid_body" \
    && grep -Eq '"serverTime"[[:space:]]*:[[:space:]]*"[^"]+"' "$valid_body" \
    && grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*true' "$valid_body" \
    && ! grep -q 'otw_agent_' "$valid_body"; then
    echo "PASS  valid token returns authenticated payload -> 200"
    passes=$((passes + 1))
else
    echo "FAIL  valid token payload -> $valid_status"
    echo "      body: $(tr '\n' ' ' < "$valid_body" | cut -c1-300)"
    failures=$((failures + 1))
fi
rm -f "$valid_body"

# 2. Missing token.
request 401 "missing token" "$PING"

# 3. Well-formed but unknown token (the same shape as a real token).
request 401 "unknown token" "$PING" "otw_agent_$(printf '0%.0s' {1..64})"

# 4. Malformed / wrong credential shape (session and MCP tokens must not work here).
request 401 "malformed token" "$PING" "not-a-token"
request 401 "session-shaped token" "$PING" "$(printf 'a%.0s' {1..64})"
if [ -n "${OTW_MCP_TOKEN:-}" ]; then
    request 401 "MCP token is not an agent token" "$PING" "$OTW_MCP_TOKEN"
fi
if [ -n "${OTW_SESSION_COOKIE:-}" ]; then
    request 401 "browser session cookie is not a bearer token" "$PING" "" \
        -H "Cookie: otw_session=$OTW_SESSION_COOKIE"
fi

# 5. Expired token.
if [ -n "${OTW_AGENT_EXPIRED_TOKEN:-}" ]; then
    request 401 "expired token" "$PING" "$OTW_AGENT_EXPIRED_TOKEN"
else
    echo "SKIP  expired token (set OTW_AGENT_EXPIRED_TOKEN)"
fi

# 6. Revoked token.
if [ -n "${OTW_AGENT_REVOKED_TOKEN:-}" ]; then
    request 401 "revoked token" "$PING" "$OTW_AGENT_REVOKED_TOKEN"
else
    echo "SKIP  revoked token (set OTW_AGENT_REVOKED_TOKEN)"
fi

# 7. Valid token, insufficient scope.
if [ -n "${OTW_AGENT_NOSCOPE_TOKEN:-}" ]; then
    request 403 "token without agent:ping scope" "$PING" "$OTW_AGENT_NOSCOPE_TOKEN"
else
    echo "SKIP  insufficient scope (set OTW_AGENT_NOSCOPE_TOKEN)"
fi

# 8. Credential separation: the agent token must not open a session-protected route.
request 401 "agent token is not a session" "$BASE/api/portfolios" "$OTW_AGENT_TOKEN"

# 9. Repetition has no business side effect: the same GET twice is still a successful
#    read with the same shape (the endpoint touches no business table).
first="$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $OTW_AGENT_TOKEN" "$PING" 2>/dev/null)"
second="$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $OTW_AGENT_TOKEN" "$PING" 2>/dev/null)"
if [ "$first" = "200" ] && [ "$second" = "200" ]; then
    echo "PASS  repeated valid requests stay successful -> 200/200"
    passes=$((passes + 1))
else
    echo "FAIL  repeated valid requests -> $first/$second (expected 200/200)"
    failures=$((failures + 1))
fi

echo
echo "$passes passed, $failures failed"
[ "$failures" -eq 0 ]
