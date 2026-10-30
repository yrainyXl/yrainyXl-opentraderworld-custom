//! OAuth state-machine tests (authorization code binding, single use, refresh rotation,
//! replay detection, revocation and user/client scoping).
//!
//! These hit a real Postgres when `DATABASE_URL` is set, following the convention of the
//! sibling integration tests. Without it they pass vacuously.

use uuid::Uuid;

use otw_store::connect_and_migrate;
use otw_store::mcp::hash_token;
use otw_store::oauth::{self, CodeError, RefreshError};
use sqlx::PgPool;

const REDIRECT: &str = "https://client.example/callback";
const RESOURCE: &str = "https://otw.example/api/mcp";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => {
            eprintln!("DATABASE_URL not set — skipping OAuth DB tests");
            return None;
        }
    };
    Some(connect_and_migrate(&url).await.expect("test database"))
}

async fn user(pool: &PgPool) -> otw_store::User {
    let id = Uuid::new_v4();
    let name = format!("oauth-test-{id}");
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin)          VALUES ($1, $2, 'placeholder', FALSE)",
    )
    .bind(id)
    .bind(&name)
    .execute(pool)
    .await
    .expect("test user");
    otw_store::User {
        id,
        username: name,
        password_hash: "placeholder".into(),
        is_admin: false,
        must_change_password: false,
    }
}

async fn client(pool: &PgPool) -> otw_store::oauth::OAuthClient {
    let (row, _) = oauth::create_client(
        pool,
        "test client",
        &[REDIRECT.to_string()],
        &[
            oauth::SCOPE_MCP_READ.to_string(),
            oauth::SCOPE_OFFLINE_ACCESS.to_string(),
        ],
        "pre_registered",
    )
    .await
    .expect("client");
    row
}

async fn cleanup(pool: &PgPool, user_id: Uuid) {
    let _ = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await;
}

async fn pending(pool: &PgPool, user_id: Uuid, client_id: Uuid) -> oauth::OAuthPending {
    let id = oauth::create_pending(
        pool,
        user_id,
        client_id,
        REDIRECT,
        RESOURCE,
        &[
            oauth::SCOPE_MCP_READ.to_string(),
            oauth::SCOPE_OFFLINE_ACCESS.to_string(),
        ],
        Some("state-123"),
        CHALLENGE,
        "S256",
        &hash_token("csrf-test"),
    )
    .await
    .expect("pending");
    // Return the row the decision path would consume; create_pending returns only the id.
    oauth::consume_pending(pool, id, user_id, &hash_token("csrf-test"))
        .await
        .expect("pending lookup")
        .expect("pending row")
}

#[tokio::test]
async fn code_is_bound_to_client_redirect_resource_and_single_use() {
    let Some(pool) = pool().await else { return };
    let u = user(&pool).await;
    let c = client(&pool).await;
    let p = pending(&pool, u.id, c.id).await;
    let code = oauth::create_code(&pool, &p).await.expect("code");

    // Wrong client / redirect / resource must all be rejected before the code is consumed.
    let other_client = Uuid::new_v4();
    assert_eq!(
        oauth::consume_code(&pool, &code, other_client, REDIRECT, RESOURCE)
            .await
            .unwrap(),
        Err(CodeError::ClientMismatch)
    );
    assert_eq!(
        oauth::consume_code(&pool, &code, c.id, "https://evil.example/cb", RESOURCE)
            .await
            .unwrap(),
        Err(CodeError::RedirectMismatch)
    );
    assert_eq!(
        oauth::consume_code(&pool, &code, c.id, REDIRECT, "https://other.example/api/mcp")
            .await
            .unwrap(),
        Err(CodeError::ResourceMismatch)
    );

    // The correct exchange succeeds once…
    let consumed = oauth::consume_code(&pool, &code, c.id, REDIRECT, RESOURCE)
        .await
        .unwrap()
        .expect("correct bindings");
    assert_eq!(consumed.client_id, c.id);
    assert_eq!(consumed.user_id, u.id);
    assert_eq!(consumed.redirect_uri, REDIRECT);
    assert_eq!(consumed.resource, RESOURCE);

    // …and the same code is now burned.
    assert_eq!(
        oauth::consume_code(&pool, &code, c.id, REDIRECT, RESOURCE)
            .await
            .unwrap(),
        Err(CodeError::Reused)
    );

    oauth::delete_client(&pool, &c.client_id).await.ok();
    cleanup(&pool, u.id).await;
}

#[tokio::test]
async fn expired_code_is_rejected() {
    let Some(pool) = pool().await else { return };
    let u = user(&pool).await;
    let c = client(&pool).await;
    let p = pending(&pool, u.id, c.id).await;
    let code = oauth::create_code(&pool, &p).await.expect("code");
    sqlx::query(
        "UPDATE oauth_authorization_codes SET expires_at = now() - interval '1 minute' \
         WHERE code_hash = $1",
    )
    .bind(hash_token(&code))
    .execute(&pool)
    .await
    .expect("expire code");

    assert_eq!(
        oauth::consume_code(&pool, &code, c.id, REDIRECT, RESOURCE)
            .await
            .unwrap(),
        Err(CodeError::Expired)
    );
    oauth::delete_client(&pool, &c.client_id).await.ok();
    cleanup(&pool, u.id).await;
}

#[tokio::test]
async fn refresh_rotation_and_replay_revoke_the_family() {
    let Some(pool) = pool().await else { return };
    let u = user(&pool).await;
    let c = client(&pool).await;

    let first = oauth::issue_tokens(
        &pool,
        c.id,
        u.id,
        RESOURCE,
        &[oauth::SCOPE_MCP_READ.to_string()],
    )
    .await
    .expect("first tokens");

    let rotated = oauth::rotate_refresh(&pool, &first.refresh_token, c.id, RESOURCE)
        .await
        .unwrap()
        .expect("rotation succeeds");
    assert_ne!(first.refresh_token, rotated.refresh_token);
    assert_ne!(first.access_token, rotated.access_token);

    // The first (now consumed) refresh token is a replay. It must fail and burn the family.
    assert_eq!(
        oauth::rotate_refresh(&pool, &first.refresh_token, c.id, RESOURCE)
            .await
            .unwrap(),
        Err(RefreshError::Replay)
    );

    for token in [&first.access_token, &rotated.access_token] {
        let row = oauth::find_access_token(&pool, token)
            .await
            .unwrap()
            .expect("access row exists");
        assert!(row.is_revoked(), "replay must revoke every access token in the family");
    }

    // The rotated refresh token is also dead after family revocation.
    assert_eq!(
        oauth::rotate_refresh(&pool, &rotated.refresh_token, c.id, RESOURCE)
            .await
            .unwrap(),
        Err(RefreshError::Invalid)
    );

    oauth::delete_client(&pool, &c.client_id).await.ok();
    cleanup(&pool, u.id).await;
}

#[tokio::test]
async fn refresh_token_is_bound_and_revocable() {
    let Some(pool) = pool().await else { return };
    let u = user(&pool).await;
    let c = client(&pool).await;
    let issued = oauth::issue_tokens(
        &pool,
        c.id,
        u.id,
        RESOURCE,
        &[oauth::SCOPE_MCP_READ.to_string()],
    )
    .await
    .expect("tokens");

    assert_eq!(
        oauth::rotate_refresh(&pool, &issued.refresh_token, Uuid::new_v4(), RESOURCE)
            .await
            .unwrap(),
        Err(RefreshError::ClientMismatch)
    );
    assert_eq!(
        oauth::rotate_refresh(&pool, &issued.refresh_token, c.id, "https://other/api/mcp")
            .await
            .unwrap(),
        Err(RefreshError::ResourceMismatch)
    );

    assert!(oauth::revoke_refresh_token(&pool, &issued.refresh_token)
        .await
        .unwrap());
    let access = oauth::find_access_token(&pool, &issued.access_token)
        .await
        .unwrap()
        .expect("access row");
    assert!(access.is_revoked());

    oauth::delete_client(&pool, &c.client_id).await.ok();
    cleanup(&pool, u.id).await;
}

#[tokio::test]
async fn grants_list_and_user_scoped_revocation() {
    let Some(pool) = pool().await else { return };
    let u = user(&pool).await;
    let other = user(&pool).await;
    let c = client(&pool).await;
    oauth::issue_tokens(
        &pool,
        c.id,
        u.id,
        RESOURCE,
        &[oauth::SCOPE_MCP_READ.to_string()],
    )
    .await
    .expect("tokens");

    let grants = oauth::list_grants(&pool, u.id).await.expect("grants");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].client_id, c.client_id);
    assert_eq!(grants[0].access_tokens, 1);
    assert_eq!(grants[0].refresh_tokens, 1);

    // Revoking for the wrong user affects nothing.
    assert_eq!(
        oauth::revoke_user_client(&pool, other.id, &c.client_id)
            .await
            .unwrap(),
        0
    );
    assert_eq!(oauth::list_grants(&pool, u.id).await.unwrap().len(), 1);

    // Revoking for the owner removes the grant from the active list.
    assert!(oauth::revoke_user_client(&pool, u.id, &c.client_id)
        .await
        .unwrap()
        > 0);
    assert!(oauth::list_grants(&pool, u.id).await.unwrap().is_empty());

    oauth::delete_client(&pool, &c.client_id).await.ok();
    cleanup(&pool, u.id).await;
    cleanup(&pool, other.id).await;
}
