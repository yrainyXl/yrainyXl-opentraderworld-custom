//! Host-CLI commands: account recovery that runs from the machine's shell and nowhere else.
//!
//! A self-hosted single-user install has no second factor and no mailbox to send a reset link
//! to — there is no channel the server can trust more than the password it just lost. So the
//! recovery path is not a route at all: it is an argv branch that connects to the database,
//! does its work and exits before any listener is built. Nothing here is reachable over HTTP,
//! over MCP, or by an authenticated session, which is the entire security argument — the only
//! credential is being able to run a process next to the database, and anyone who can already
//! holds `DATABASE_URL` and `OTW_SECRET_KEY`. The command grants no privilege they lacked; it
//! just saves them from hand-writing an argon2 hash into `users`.
//!
//! Two rules shape the rest:
//!
//! * **The password never rides in argv.** `ps`, the shell history file and `docker inspect`
//!   all publish a process's arguments to every user on the host, and a container's command
//!   line survives in the daemon's state long after the process is gone. So the command takes
//!   a *username* and either mints a password itself (printed to that terminal only) or reads
//!   one from stdin.
//! * **What it sets is a temporary password, not the account's new credential.** It is marked
//!   `must_change_password`, so the value that crossed a terminal buys exactly one sign-in
//!   before the app demands a real one, and every existing session is revoked so a stolen
//!   cookie cannot outlive the reset.
//!
//! Encrypted data is unaffected: the vault and every stored provider secret are sealed with
//! `OTW_SECRET_KEY`, never with the login password, so a reset loses nothing.
//!
//! The same argv-only rule covers external-agent API tokens: `create-agent-token` prints a
//! freshly minted bearer value once to this terminal and stores only its SHA-256. Nothing
//! about a token is accepted from argv, so a token never lands in shell history or `ps`;
//! `list-agent-tokens` and `revoke-agent-token` operate on ids/prefixes only.

use anyhow::Context;
use sqlx::PgPool;
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
use uuid::Uuid;

use crate::auth;

/// Alphabet for a generated temporary password: Crockford base32 (no I, L, O, U), because
/// this string is read off a terminal and typed into a browser. Exactly 32 symbols, so a
/// 5-bit mask picks one uniformly — no modulo bias to reason about.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Bytes of entropy in a generated password: 20 symbols × 5 bits = 100 bits.
const PASSWORD_LEN: usize = 20;

/// Minimum length accepted for an operator-supplied password (matches the setup wizard).
const MIN_PASSWORD_LEN: usize = 8;

pub enum Command {
    ResetPassword { username: String, from_stdin: bool },
    ListUsers,
    CreateAgentToken {
        username: String,
        agent: String,
        name: String,
        scopes: Vec<String>,
        expires_in_days: Option<i64>,
    },
    ListAgentTokens { username: String },
    RevokeAgentToken { id: Uuid },
    CreateOauthClient {
        name: String,
        redirect_uris: Vec<String>,
        scopes: Vec<String>,
    },
    ListOauthClients,
    DeleteOauthClient { client_id: String },
    RevokeOauthGrants { username: String },
}

/// Read a command off argv, or `None` when this is an ordinary server start.
///
/// Unrecognised arguments deliberately fall through to `None` (the server boots as before):
/// `--seed-demo` is parsed elsewhere, and a typo must never leave a deployment silently
/// refusing to start.
pub fn from_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    match args.next()?.as_str() {
        "reset-password" => {
            let mut username: Option<String> = None;
            let mut from_stdin = false;
            for arg in args {
                match arg.as_str() {
                    "--stdin" => from_stdin = true,
                    other if other.starts_with('-') => usage(&format!("unknown option `{other}`")),
                    other if username.is_some() => usage(&format!("unexpected argument `{other}`")),
                    other => username = Some(other.to_string()),
                }
            }
            match username {
                Some(username) => Some(Command::ResetPassword { username, from_stdin }),
                None => usage("reset-password needs a username"),
            }
        }
        "list-users" => Some(Command::ListUsers),
        "create-agent-token" => parse_create_agent_token(args),
        "list-agent-tokens" => parse_list_agent_tokens(args),
        "revoke-agent-token" => parse_revoke_agent_token(args),
        "create-oauth-client" => parse_create_oauth_client(args),
        "list-oauth-clients" => parse_list_oauth_clients(args),
        "delete-oauth-client" => parse_delete_oauth_client(args),
        "revoke-oauth-grants" => parse_revoke_oauth_grants(args),
        _ => None,
    }
}

/// Read the value following a flag. A missing value or another flag is a usage error.
fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    match args.next() {
        Some(v) if !v.starts_with('-') => v,
        Some(v) => usage(&format!("{flag} needs a value, got `{v}`")),
        None => usage(&format!("{flag} needs a value")),
    }
}

fn parse_create_agent_token(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut username: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut name: Option<String> = None;
    let mut scopes: Vec<String> = Vec::new();
    let mut expires_in_days: Option<i64> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--agent" => agent = Some(next_value(&mut args, "--agent")),
            "--name" => name = Some(next_value(&mut args, "--name")),
            "--scope" => scopes.push(next_value(&mut args, "--scope")),
            "--expires-in-days" => {
                let raw = next_value(&mut args, "--expires-in-days");
                let days = raw
                    .parse::<i64>()
                    .unwrap_or_else(|_| usage("--expires-in-days must be an integer"));
                expires_in_days = Some(days);
            }
            other if other.starts_with('-') => {
                usage(&format!("unknown option `{other}`"))
            }
            other if username.is_some() => {
                usage(&format!("unexpected argument `{other}`"))
            }
            other => username = Some(other.to_string()),
        }
    }

    let username = username.unwrap_or_else(|| usage("create-agent-token needs a username"));
    let agent = agent.unwrap_or_else(|| usage("create-agent-token needs --agent <name>"));

    if scopes.is_empty() {
        scopes.push(otw_store::agent_api_tokens::SCOPE_AGENT_PING.to_string());
    }
    for scope in &scopes {
        if !otw_store::agent_api_tokens::SUPPORTED_SCOPES.contains(&scope.as_str()) {
            usage(&format!(
                "unsupported scope `{scope}` (supported: {})",
                otw_store::agent_api_tokens::SUPPORTED_SCOPES.join(", ")
            ));
        }
    }
    scopes.sort();
    scopes.dedup();

    let name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{agent} agent token"));

    Some(Command::CreateAgentToken {
        username,
        agent,
        name,
        scopes,
        expires_in_days,
    })
}

fn parse_list_agent_tokens(mut args: impl Iterator<Item = String>) -> Option<Command> {
    match args.next() {
        Some(username) if !username.starts_with('-') => {
            if let Some(extra) = args.next() {
                usage(&format!("unexpected argument `{extra}`"));
            }
            Some(Command::ListAgentTokens { username })
        }
        Some(other) => usage(&format!("unexpected argument `{other}`")),
        None => usage("list-agent-tokens needs a username"),
    }
}

fn parse_revoke_agent_token(mut args: impl Iterator<Item = String>) -> Option<Command> {
    match args.next() {
        Some(raw) if !raw.starts_with('-') => {
            let id = Uuid::parse_str(&raw)
                .unwrap_or_else(|_| usage("revoke-agent-token needs a token id (UUID)"));
            if let Some(extra) = args.next() {
                usage(&format!("unexpected argument `{extra}`"));
            }
            Some(Command::RevokeAgentToken { id })
        }
        Some(other) => usage(&format!("unexpected argument `{other}`")),
        None => usage("revoke-agent-token needs a token id"),
    }
}

fn parse_create_oauth_client(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut name: Option<String> = None;
    let mut redirect_uris: Vec<String> = Vec::new();
    let mut scopes: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--redirect-uri" => redirect_uris.push(next_value(&mut args, "--redirect-uri")),
            "--scope" => scopes.push(next_value(&mut args, "--scope")),
            other if other.starts_with('-') => usage(&format!("unknown option `{other}`")),
            other if name.is_some() => usage(&format!("unexpected argument `{other}`")),
            other => name = Some(other.to_string()),
        }
    }
    let name = name.unwrap_or_else(|| usage("create-oauth-client needs a client name"));
    if redirect_uris.is_empty() {
        usage("create-oauth-client needs at least one --redirect-uri");
    }
    if scopes.is_empty() {
        scopes = vec![
            otw_store::oauth::SCOPE_MCP_READ.to_string(),
            otw_store::oauth::SCOPE_OFFLINE_ACCESS.to_string(),
        ];
    }
    for scope in &scopes {
        if !otw_store::oauth::SUPPORTED_SCOPES.contains(&scope.as_str()) {
            usage(&format!(
                "unsupported OAuth scope `{scope}` (supported: {})",
                otw_store::oauth::SUPPORTED_SCOPES.join(", ")
            ));
        }
    }
    for uri in &redirect_uris {
        if !crate::oauth::valid_redirect_uri(uri) {
            usage(&format!(
                "invalid redirect URI `{uri}` (HTTPS, or HTTP on localhost/127.0.0.1; no fragments/userinfo)"
            ));
        }
    }
    scopes.sort();
    scopes.dedup();
    Some(Command::CreateOauthClient { name, redirect_uris, scopes })
}

fn parse_list_oauth_clients(mut args: impl Iterator<Item = String>) -> Option<Command> {
    if let Some(extra) = args.next() {
        usage(&format!("unexpected argument `{extra}`"));
    }
    Some(Command::ListOauthClients)
}

fn parse_delete_oauth_client(mut args: impl Iterator<Item = String>) -> Option<Command> {
    match args.next() {
        Some(client_id) if !client_id.starts_with('-') => {
            if let Some(extra) = args.next() {
                usage(&format!("unexpected argument `{extra}`"));
            }
            Some(Command::DeleteOauthClient { client_id })
        }
        Some(other) => usage(&format!("unexpected argument `{other}`")),
        None => usage("delete-oauth-client needs a client_id"),
    }
}

fn parse_revoke_oauth_grants(mut args: impl Iterator<Item = String>) -> Option<Command> {
    match args.next() {
        Some(username) if !username.starts_with('-') => {
            if let Some(extra) = args.next() {
                usage(&format!("unexpected argument `{extra}`"));
            }
            Some(Command::RevokeOauthGrants { username })
        }
        Some(other) => usage(&format!("unexpected argument `{other}`")),
        None => usage("revoke-oauth-grants needs a username"),
    }
}

/// Print usage and exit. Never returns.
fn usage(problem: &str) -> ! {
    eprintln!("otw-core: {problem}\n");
    eprintln!("Account recovery (run on the host, e.g. `docker exec -it opentraderworld-core-1 /app/otw-core …`):");
    eprintln!("  reset-password <username>           set a printed one-time password");
    eprintln!("  reset-password <username> --stdin   read the new password from stdin instead");
    eprintln!("  list-users                          print the account names");
    eprintln!();
    eprintln!("External AI agent API tokens (run on the host; the bearer value is printed ONCE and never stored):");
    eprintln!("  create-agent-token <username> --agent <name> [--name <label>]");
    eprintln!("      [--scope <scope>] [--expires-in-days <n>]");
    eprintln!("      supported scopes: {}", otw_store::agent_api_tokens::SUPPORTED_SCOPES.join(", "));
    eprintln!("      default scope: agent:ping; --expires-in-days 0 = never expires");
    eprintln!("  list-agent-tokens <username>        list token metadata (never the token)");
    eprintln!("  revoke-agent-token <id>             revoke one token by id");
    eprintln!();
    eprintln!("MCP OAuth clients (pre-registration; ChatGPT can also use dynamic registration):");
    eprintln!("  create-oauth-client <name> --redirect-uri <uri> [--redirect-uri <uri>] [--scope <scope>]");
    eprintln!("      supported scopes: {}", otw_store::oauth::SUPPORTED_SCOPES.join(", "));
    eprintln!("  list-oauth-clients                  list registered OAuth clients");
    eprintln!("  delete-oauth-client <client_id>     delete a client and revoke its grants");
    eprintln!("  revoke-oauth-grants <username>      revoke every OAuth token for an account");
    std::process::exit(2);
}

/// Run a command against the database, then return so `main` can exit.
pub async fn run(cmd: Command, database_url: &str) -> anyhow::Result<()> {
    let pool = otw_store::connect_and_migrate(database_url).await?;
    match cmd {
        Command::ResetPassword { username, from_stdin } => {
            reset_password(&pool, &username, from_stdin).await
        }
        Command::ListUsers => {
            for name in otw_store::list_usernames(&pool).await? {
                println!("{name}");
            }
            Ok(())
        }
        Command::CreateAgentToken {
            username,
            agent,
            name,
            scopes,
            expires_in_days,
        } => {
            create_agent_token(&pool, &username, &agent, &name, &scopes, expires_in_days).await
        }
        Command::ListAgentTokens { username } => list_agent_tokens(&pool, &username).await,
        Command::RevokeAgentToken { id } => revoke_agent_token(&pool, id).await,
        Command::CreateOauthClient {
            name,
            redirect_uris,
            scopes,
        } => create_oauth_client(&pool, &name, &redirect_uris, &scopes).await,
        Command::ListOauthClients => list_oauth_clients(&pool).await,
        Command::DeleteOauthClient { client_id } => {
            delete_oauth_client(&pool, &client_id).await
        }
        Command::RevokeOauthGrants { username } => {
            revoke_oauth_grants(&pool, &username).await
        }
    }
}

async fn reset_password(pool: &PgPool, username: &str, from_stdin: bool) -> anyhow::Result<()> {
    let username = username.trim();
    let Some(user) = otw_store::find_user_by_username(pool, username).await? else {
        // Listing the accounts on a miss is not a disclosure here: the caller already reads
        // the whole database. It saves the round trip when the forgotten thing is the
        // username rather than the password.
        let known = otw_store::list_usernames(pool).await?;
        let known = if known.is_empty() { "none".to_string() } else { known.join(", ") };
        anyhow::bail!("no account named `{username}` (accounts: {known})");
    };

    let (password, generated) = if from_stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .context("reading the new password from stdin")?;
        let password = buf.trim_end_matches(['\r', '\n']).to_string();
        if password.chars().count() < MIN_PASSWORD_LEN {
            anyhow::bail!("password must be at least {MIN_PASSWORD_LEN} characters");
        }
        (password, false)
    } else {
        (generate_password()?, true)
    };

    let hash = auth::hash_password(&password)?;
    otw_store::reset_password(pool, user.id, &hash).await?;

    // Audit trail, written straight to `app_logs` rather than through `tracing`: the layer
    // that persists tracing events is asynchronous and this process exits immediately after.
    // The row records that a reset happened, never what it set.
    if let Err(e) = otw_store::logs::insert(
        pool,
        "warn",
        "otw_core::cli",
        &format!(
            "password reset from the host CLI for account '{}'; all sessions revoked",
            user.username
        ),
    )
    .await
    {
        eprintln!("warning: the reset succeeded but could not be written to the app log: {e:#}");
    }

    println!("Password reset for '{}'.", user.username);
    if generated {
        println!("\n  Temporary password:  {password}\n");
    }
    println!("Sign in with it once; the app then asks for a password of your own.");
    println!("Every existing session was revoked.");
    Ok(())
}

/// Turn "valid for N days" into an absolute instant. `None`/`Some(0)` = no expiry.
/// The ten-year ceiling is a guard against a fat-fingered figure, not a security boundary.
fn expiry_from_days(days: Option<i64>) -> anyhow::Result<Option<OffsetDateTime>> {
    match days {
        None | Some(0) => Ok(None),
        Some(n) if (1..=3650).contains(&n) => {
            Ok(Some(OffsetDateTime::now_utc() + Duration::days(n)))
        }
        Some(_) => anyhow::bail!("--expires-in-days must be between 0 and 3650"),
    }
}

fn fmt_time(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.unix_timestamp().to_string())
}

fn fmt_opt_time(t: Option<OffsetDateTime>) -> String {
    t.map(fmt_time).unwrap_or_else(|| "never".to_string())
}

async fn create_agent_token(
    pool: &PgPool,
    username: &str,
    agent: &str,
    name: &str,
    scopes: &[String],
    expires_in_days: Option<i64>,
) -> anyhow::Result<()> {
    let agent = agent.trim();
    if agent.is_empty()
        || agent.len() > 64
        || !agent
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        anyhow::bail!(
            "--agent must be 1-64 characters of A-Z, a-z, 0-9, '.', '_' or '-'"
        );
    }
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        anyhow::bail!("--name must be 1-200 characters");
    }
    if scopes.is_empty() {
        anyhow::bail!("at least one --scope is required");
    }

    let user = otw_store::find_user_by_username(pool, username.trim())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no account named `{username}`"))?;

    let expires_at = expiry_from_days(expires_in_days)?;
    let (row, token) = otw_store::agent_api_tokens::create_token(
        pool,
        user.id,
        agent,
        name,
        scopes,
        expires_at,
    )
    .await?;

    // Audit trail: records the existence and metadata of the credential, never the
    // bearer value. Written directly because this process exits right after.
    if let Err(e) = otw_store::logs::insert(
        pool,
        "info",
        "otw_core::cli",
        &format!(
            "agent API token created for account '{}' (id {}, agent '{}', scopes [{}], expires {})",
            user.username,
            row.id,
            row.agent,
            row.scopes.join(", "),
            fmt_opt_time(row.expires_at)
        ),
    )
    .await
    {
        eprintln!("warning: the token was created but the audit log entry failed: {e:#}");
    }

    println!("Agent API token created for account '{}'.", user.username);
    println!("  id:      {}", row.id);
    println!("  agent:   {}", row.agent);
    println!("  prefix:  {}", row.prefix);
    println!("  scopes:  {}", row.scopes.join(", "));
    println!("  expires: {}", fmt_opt_time(row.expires_at));
    println!();
    println!("  token:   {token}");
    println!();
    println!("Store it now: it is shown only once, and only its SHA-256 is stored.");
    println!("Send it as `Authorization: Bearer <token>` to /api/agent/ping.");
    Ok(())
}

async fn list_agent_tokens(pool: &PgPool, username: &str) -> anyhow::Result<()> {
    let user = otw_store::find_user_by_username(pool, username.trim())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no account named `{username}`"))?;
    let tokens = otw_store::agent_api_tokens::list_for_user(pool, user.id).await?;

    if tokens.is_empty() {
        println!("No agent API tokens for '{}'.", user.username);
        return Ok(());
    }
    println!("Agent API tokens for '{}':", user.username);
    for t in tokens {
        let status = if t.is_revoked() {
            "revoked"
        } else if t.is_expired() {
            "expired"
        } else {
            "active"
        };
        println!(
            "  {}  prefix={}  agent={}  name={}  scopes=[{}]  created={}  expires={}  status={}",
            t.id,
            t.prefix,
            t.agent,
            t.name,
            t.scopes.join(", "),
            fmt_time(t.created_at),
            fmt_opt_time(t.expires_at),
            status
        );
    }
    Ok(())
}

async fn create_oauth_client(
    pool: &PgPool,
    name: &str,
    redirect_uris: &[String],
    scopes: &[String],
) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        anyhow::bail!("OAuth client name must be 1-100 characters");
    }
    let (row, client_id) =
        otw_store::oauth::create_client(pool, name, redirect_uris, scopes, "pre_registered")
            .await?;
    if let Err(e) = otw_store::logs::insert(
        pool,
        "info",
        "otw_core::cli",
        &format!(
            "OAuth client registered from the host CLI (client_id {}, name '{}')",
            row.client_id, row.client_name
        ),
    )
    .await
    {
        eprintln!("warning: the client was created but the audit log entry failed: {e:#}");
    }
    println!("OAuth client registered: {}", row.client_name);
    println!("  client_id:     {client_id}");
    println!("  redirect_uris: {}", row.redirect_uris.join(", "));
    println!("  scopes:        {}", row.scopes.join(", "));
    println!("  auth method:   {} (public client; PKCE required)", row.token_endpoint_auth_method);
    Ok(())
}

async fn list_oauth_clients(pool: &PgPool) -> anyhow::Result<()> {
    let clients = otw_store::oauth::list_clients(pool).await?;
    if clients.is_empty() {
        println!("No OAuth clients registered.");
        return Ok(());
    }
    for c in clients {
        let status = if c.is_disabled() { "disabled" } else { "active" };
        println!(
            "  {}  name={}  redirect_uris=[{}]  scopes=[{}]  source={}  created={}  status={}",
            c.client_id,
            c.client_name,
            c.redirect_uris.join(", "),
            c.scopes.join(", "),
            c.source,
            fmt_time(c.created_at),
            status
        );
    }
    Ok(())
}

async fn delete_oauth_client(pool: &PgPool, client_id: &str) -> anyhow::Result<()> {
    if !otw_store::oauth::delete_client(pool, client_id).await? {
        anyhow::bail!("no OAuth client with client_id `{client_id}`");
    }
    if let Err(e) = otw_store::logs::insert(
        pool,
        "warn",
        "otw_core::cli",
        &format!("OAuth client deleted from the host CLI (client_id {client_id})"),
    )
    .await
    {
        eprintln!("warning: the client was deleted but the audit log entry failed: {e:#}");
    }
    println!("Deleted OAuth client {client_id}; its tokens, codes and pending requests are gone.");
    Ok(())
}

async fn revoke_oauth_grants(pool: &PgPool, username: &str) -> anyhow::Result<()> {
    let user = otw_store::find_user_by_username(pool, username.trim())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no account named `{username}`"))?;
    otw_store::oauth::revoke_all_for_user(pool, user.id).await?;
    if let Err(e) = otw_store::logs::insert(
        pool,
        "warn",
        "otw_core::cli",
        &format!(
            "all OAuth grants revoked from the host CLI for account '{}'",
            user.username
        ),
    )
    .await
    {
        eprintln!("warning: the grants were revoked but the audit log entry failed: {e:#}");
    }
    println!("Revoked every OAuth access/refresh token for '{}'.", user.username);
    Ok(())
}

async fn revoke_agent_token(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    let row = otw_store::agent_api_tokens::revoke(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no agent API token with id `{id}`"))?;

    if let Err(e) = otw_store::logs::insert(
        pool,
        "warn",
        "otw_core::cli",
        &format!(
            "agent API token revoked from the host CLI (id {}, agent '{}', prefix {})",
            row.id, row.agent, row.prefix
        ),
    )
    .await
    {
        eprintln!("warning: the token was revoked but the audit log entry failed: {e:#}");
    }

    println!(
        "Revoked agent API token {} (agent '{}', prefix {}).",
        row.id, row.agent, row.prefix
    );
    println!("Any further request with it will be rejected with 401.");
    Ok(())
}

/// A fresh temporary password, grouped in fives so it can be read off a terminal without
/// losing your place.
fn generate_password() -> anyhow::Result<String> {
    let mut bytes = [0u8; PASSWORD_LEN];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    let mut out = String::with_capacity(PASSWORD_LEN + PASSWORD_LEN / 5);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[(b & 31) as usize] as char);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_are_grouped_and_unambiguous() {
        let pw = generate_password().unwrap();
        assert_eq!(pw.len(), PASSWORD_LEN + 3);
        assert_eq!(pw.matches('-').count(), 3);
        assert!(pw
            .chars()
            .filter(|c| *c != '-')
            .all(|c| ALPHABET.contains(&(c as u8))));
    }

    #[test]
    fn generated_passwords_differ() {
        assert_ne!(generate_password().unwrap(), generate_password().unwrap());
    }
}
