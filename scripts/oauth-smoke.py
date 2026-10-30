#!/usr/bin/env python3
"""End-to-end smoke tests for the OTW MCP OAuth server.

Covers discovery, DCR, PKCE authorization/consent, token exchange, refresh rotation and
replay detection, read-only MCP tool access, fixed-token compatibility, and the negative
cases (wrong PKCE, code reuse, redirect/client/resource mismatch, revoked token).

Bearer values are never printed. Requires a disposable **admin** OTW account (the
authorization endpoint only admits the owner) and a running core:

    OTW_BASE_URL=https://nova.lumora-lab.top \
    OTW_TEST_USERNAME=oauth-smoke-... \
    OTW_TEST_PASSWORD='...' \
    python3 scripts/oauth-smoke.py

The test account must already exist; delete it afterwards (it cascades to its tokens).
"""

import base64
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from http.cookiejar import CookieJar

BASE = os.environ.get("OTW_BASE_URL", "http://127.0.0.1:5454").rstrip("/")
USERNAME = os.environ.get("OTW_TEST_USERNAME", "")
PASSWORD = os.environ.get("OTW_TEST_PASSWORD", "")
REDIRECT = os.environ.get("OTW_OAUTH_REDIRECT", "http://127.0.0.1:1455/callback")
RESOURCE = f"{BASE}/api/mcp"

failures = 0
passes = 0


def check(name, ok, detail=""):
    global failures, passes
    if ok:
        print(f"PASS  {name}")
        passes += 1
    else:
        print(f"FAIL  {name}" + (f" — {detail}" if detail else ""))
        failures += 1


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


jar = CookieJar()
opener = urllib.request.build_opener(
    urllib.request.HTTPCookieProcessor(jar), NoRedirect()
)


def http(method, path, *, data=None, headers=None, form=False):
    """Return (status, headers, body). Redirects are not followed."""
    url = path if path.startswith("http") else BASE + path
    h = dict(headers or {})
    body = None
    if data is not None:
        if form:
            body = urllib.parse.urlencode(data).encode()
            h.setdefault("Content-Type", "application/x-www-form-urlencoded")
        else:
            body = json.dumps(data).encode()
            h.setdefault("Content-Type", "application/json")
    req = urllib.request.Request(url, data=body, method=method, headers=h)
    try:
        with opener.open(req, timeout=20) as resp:
            return resp.status, dict(resp.headers), resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read().decode("utf-8", "replace")


def header(headers, name):
    for k, v in headers.items():
        if k.lower() == name.lower():
            return v
    return ""


def json_body(body):
    try:
        return json.loads(body)
    except Exception:
        return {}


def login():
    status, _, body = http(
        "POST", "/api/login", data={"username": USERNAME, "password": PASSWORD}
    )
    check("login succeeds", status == 200, f"status={status}")
    return status == 200


def register_client(name):
    status, _, body = http(
        "POST",
        "/api/oauth/register",
        data={
            "client_name": name,
            "redirect_uris": [REDIRECT],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "mcp:read offline_access",
        },
    )
    data = json_body(body)
    ok = status == 201 and data.get("client_id", "").startswith("otw_oc_")
    check(f"DCR registers {name}", ok, f"status={status} body={body[:200]}")
    return data.get("client_id")


def pkce():
    verifier = uuid.uuid4().hex + uuid.uuid4().hex
    challenge = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
        .rstrip(b"=")
        .decode()
    )
    return verifier, challenge


def authorize_code(client_id, verifier, challenge, scope="mcp:read offline_access"):
    state = uuid.uuid4().hex
    q = urllib.parse.urlencode(
        {
            "response_type": "code",
            "client_id": client_id,
            "redirect_uri": REDIRECT,
            "state": state,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "resource": RESOURCE,
            "scope": scope,
        }
    )
    status, _, page = http("GET", f"/api/oauth/authorize?{q}")
    if status != 200:
        check("authorize renders consent", False, f"status={status} body={page[:200]}")
        return None
    check("authorize renders consent", True)

    req_id = re.search(r'name="request_id" value="([^"]+)"', page)
    csrf = re.search(r'name="csrf" value="([^"]+)"', page)
    if not req_id or not csrf:
        check("consent page carries request_id + csrf", False)
        return None
    check("consent page carries request_id + csrf", True)

    status, headers, body = http(
        "POST",
        "/api/oauth/authorize/decision",
        data={
            "request_id": req_id.group(1),
            "csrf": csrf.group(1),
            "decision": "allow",
        },
        form=True,
    )
    if status not in (302, 303, 307, 308):
        check("consent approval redirects", False, f"status={status} body={body[:200]}")
        return None
    loc = header(headers, "Location")
    parsed = urllib.parse.urlsplit(loc)
    params = urllib.parse.parse_qs(parsed.query)
    ok = (
        parsed.scheme == urllib.parse.urlsplit(REDIRECT).scheme
        and parsed.netloc == urllib.parse.urlsplit(REDIRECT).netloc
        and parsed.path == urllib.parse.urlsplit(REDIRECT).path
        and params.get("code", [""])[0].startswith("otw_oac_")
        and params.get("state", [""])[0] == state
        and params.get("iss", [""])[0] == BASE
    )
    check("consent approval redirects with code/state/iss", ok, loc[:200])
    return params.get("code", [None])[0] if ok else None


def exchange_code(client_id, code, verifier, redirect=REDIRECT, resource=RESOURCE):
    status, _, body = http(
        "POST",
        "/api/oauth/token",
        data={
            "grant_type": "authorization_code",
            "client_id": client_id,
            "code": code,
            "redirect_uri": redirect,
            "code_verifier": verifier,
            "resource": resource,
        },
        form=True,
    )
    return status, json_body(body)


def refresh(client_id, token, resource=RESOURCE):
    status, _, body = http(
        "POST",
        "/api/oauth/token",
        data={
            "grant_type": "refresh_token",
            "client_id": client_id,
            "refresh_token": token,
            "resource": resource,
        },
        form=True,
    )
    return status, json_body(body), body


def mcp(access_token, method, params=None):
    payload = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        payload["params"] = params
    status, headers, body = http(
        "POST",
        "/api/mcp",
        data=payload,
        headers={"Authorization": f"Bearer {access_token}"},
    )
    return status, headers, json_body(body), body


def tool_names(result):
    tools = result.get("result", {}).get("tools", [])
    return [t.get("name") for t in tools]


def main():
    if not USERNAME or not PASSWORD:
        print("error: set OTW_TEST_USERNAME and OTW_TEST_PASSWORD", file=sys.stderr)
        return 2
    if not login():
        return 1

    # Discovery must be JSON, not the SPA index.html.
    status, _, body = http("GET", "/.well-known/oauth-protected-resource")
    prm = json_body(body)
    check(
        "protected resource metadata",
        status == 200
        and prm.get("resource") == RESOURCE
        and prm.get("authorization_servers") == [BASE],
        f"status={status} body={body[:200]}",
    )
    status, _, body = http("GET", "/.well-known/oauth-authorization-server")
    asm = json_body(body)
    check(
        "authorization server metadata",
        status == 200
        and asm.get("issuer") == BASE
        and "S256" in asm.get("code_challenge_methods_supported", [])
        and asm.get("registration_endpoint"),
        f"status={status} body={body[:200]}",
    )

    client_id = register_client("oauth-smoke")
    if not client_id:
        return 1
    second_client_id = register_client("oauth-smoke-second")
    if not second_client_id:
        return 1

    verifier, challenge = pkce()
    code = authorize_code(client_id, verifier, challenge)
    if not code:
        return 1
    status, tokens = exchange_code(client_id, code, verifier)
    ok = (
        status == 200
        and tokens.get("access_token", "").startswith("otw_oat_")
        and tokens.get("refresh_token", "").startswith("otw_ort_")
        and tokens.get("token_type") == "Bearer"
        and tokens.get("resource") == RESOURCE
        and "mcp:read" in tokens.get("scope", "")
    )
    check("authorization code exchange", ok, f"status={status}")
    if not ok:
        return 1
    access = tokens["access_token"]
    refresh_token = tokens["refresh_token"]

    # Code reuse.
    status, err = exchange_code(client_id, code, verifier)
    check(
        "authorization code cannot be reused",
        status == 400 and err.get("error") == "invalid_grant",
        f"status={status} body={err}",
    )

    # Wrong PKCE: fresh code, wrong verifier.
    v2, c2 = pkce()
    code2 = authorize_code(client_id, v2, c2)
    status, err = exchange_code(client_id, code2, "x" * 50)
    check(
        "wrong PKCE verifier is rejected",
        status == 400 and err.get("error") == "invalid_grant",
        f"status={status} body={err}",
    )

    # Redirect mismatch: fresh code, wrong redirect_uri.
    v3, c3 = pkce()
    code3 = authorize_code(client_id, v3, c3)
    status, err = exchange_code(client_id, code3, v3, redirect="http://127.0.0.1:1455/other")
    check(
        "redirect_uri mismatch is rejected",
        status == 400 and err.get("error") == "invalid_grant",
        f"status={status} body={err}",
    )

    # Resource mismatch: fresh code, wrong resource.
    v4, c4 = pkce()
    code4 = authorize_code(client_id, v4, c4)
    status, err = exchange_code(
        client_id, code4, v4, resource="https://other.example/api/mcp"
    )
    check(
        "resource mismatch is rejected",
        status == 400 and err.get("error") == "invalid_target",
        f"status={status} body={err}",
    )

    # Client mismatch: fresh code for client A, exchange as client B.
    v5, c5 = pkce()
    code5 = authorize_code(client_id, v5, c5)
    status, err = exchange_code(second_client_id, code5, v5)
    check(
        "authorization code is bound to its client",
        status == 400 and err.get("error") == "invalid_grant",
        f"status={status} body={err}",
    )

    # MCP: initialize, tools/list, a read, and a denied write.
    status, headers, result, raw = mcp(access, "initialize", {})
    check(
        "OAuth token can initialize MCP",
        status == 200 and result.get("result", {}).get("serverInfo", {}).get("name"),
        f"status={status} body={raw[:200]}",
    )
    status, _, result, raw = mcp(access, "tools/list", {})
    names = tool_names(result)
    check(
        "OAuth tools/list is read-only",
        status == 200
        and "otw_read" in names
        and "otw_catalog" in names
        and "otw_write" not in names
        and "otw_compute" not in names,
        f"status={status} tools={names}",
    )
    status, _, result, raw = mcp(
        access, "tools/call", {"name": "otw_read", "arguments": {"path": "/api/portfolios"}}
    )
    check(
        "OAuth token can read a catalog endpoint",
        status == 200 and not result.get("result", {}).get("isError", False),
        f"status={status} body={raw[:200]}",
    )
    status, _, result, raw = mcp(
        access,
        "tools/call",
        {
            "name": "otw_write",
            "arguments": {"method": "POST", "path": "/api/portfolios"},
        },
    )
    text = ""
    if result.get("result", {}).get("content"):
        text = result["result"]["content"][0].get("text", "")
    check(
        "OAuth token cannot write",
        result.get("result", {}).get("isError") is True and "read-only" in text,
        f"status={status} body={raw[:200]}",
    )

    # Refresh rotation and replay.
    status, rotated, raw = refresh(client_id, refresh_token)
    ok = (
        status == 200
        and rotated.get("access_token", "").startswith("otw_oat_")
        and rotated.get("refresh_token", "").startswith("otw_ort_")
    )
    check("refresh token rotates", ok, f"status={status} body={raw[:200]}")
    if ok:
        status, replay, raw = refresh(client_id, refresh_token)
        check(
            "refresh token replay is rejected",
            status == 400 and replay.get("error") == "invalid_grant",
            f"status={status} body={raw[:200]}",
        )
        # Replay revokes the family, including the rotated access token.
        status, headers, _, raw = mcp(rotated["access_token"], "initialize", {})
        check(
            "replay revokes the token family",
            status == 401,
            f"status={status} body={raw[:200]}",
        )

    # Invalid token + challenge header.
    status, headers, _, _ = mcp("otw_oat_" + "0" * 64, "initialize", {})
    check(
        "invalid access token returns 401 + WWW-Authenticate",
        status == 401
        and "resource_metadata=" in header(headers, "WWW-Authenticate"),
        f"status={status} www={header(headers, 'WWW-Authenticate')[:200]}",
    )

    # Fixed MCP token compatibility (created through the existing session API).
    status, _, body = http(
        "POST",
        "/api/mcp/tokens",
        data={
            "name": f"oauth-smoke-fixed-{uuid.uuid4().hex[:8]}",
            "permissions": {"portfolios": "r"},
        },
    )
    fixed = json_body(body)
    fixed_token = fixed.get("token")
    fixed_id = (fixed.get("record") or {}).get("id")
    check(
        "fixed MCP token can still be created",
        status == 200 and fixed_token and fixed_token.startswith("otw_mcp_"),
        f"status={status}",
    )
    if fixed_token:
        status, _, result, raw = mcp(fixed_token, "initialize", {})
        check(
            "fixed MCP token still initializes",
            status == 200 and result.get("result", {}).get("serverInfo"),
            f"status={status} body={raw[:200]}",
        )
    if fixed_id:
        http("DELETE", f"/api/mcp/tokens/{fixed_id}")

    # Revoke an access token through RFC 7009.
    v6, c6 = pkce()
    code6 = authorize_code(client_id, v6, c6)
    status, revoke_tokens = exchange_code(client_id, code6, v6)
    if status == 200:
        http(
            "POST",
            "/api/oauth/revoke",
            data={"token": revoke_tokens["access_token"]},
            form=True,
        )
        status, _, _, raw = mcp(revoke_tokens["access_token"], "initialize", {})
        check(
            "revoked access token is rejected",
            status == 401,
            f"status={status} body={raw[:200]}",
        )

    print()
    print(f"{passes} passed, {failures} failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
