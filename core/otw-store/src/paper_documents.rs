//! Current paper strategy notes, stored as real Editor pages (not duplicated trade facts).
//! One document per (paper strategy, asset). The agent must reread and supply
//! expected_updated_at before changing it; identical content is a no-op.
use anyhow::{anyhow, Context};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;
use crate::documents;

const ROOT: &str = "AI 模拟交易策略";

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DocumentLink {
    pub asset_id: Uuid,
    pub symbol: String,
    pub name: String,
    pub provider: String,
    pub provider_id: String,
    pub document_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

pub async fn list(pool: &PgPool, strategy_id: Uuid) -> anyhow::Result<Vec<DocumentLink>> {
    sqlx::query_as::<_, DocumentLink>(
        "SELECT a.id AS asset_id, a.symbol, a.name, a.provider, a.provider_id,                 d.id AS document_id, d.updated_at          FROM paper_trading_strategies s          JOIN portfolio_assets a ON a.portfolio_id = s.portfolio_id          LEFT JOIN paper_strategy_documents p ON p.strategy_id = s.id AND p.asset_id = a.id          LEFT JOIN documents d ON d.id = p.document_id          WHERE s.id = $1 ORDER BY a.symbol, a.id")
        .bind(strategy_id).fetch_all(pool).await.context("listing paper strategy documents")
}

pub async fn get(pool: &PgPool, strategy_id: Uuid, asset_id: Uuid)
    -> anyhow::Result<Option<documents::Document>> {
    let id: Option<(Uuid,)> = sqlx::query_as(
        "SELECT d.id FROM paper_strategy_documents p          JOIN paper_trading_strategies s ON s.id = p.strategy_id          JOIN portfolio_assets a ON a.id = p.asset_id AND a.portfolio_id = s.portfolio_id          JOIN documents d ON d.id = p.document_id          WHERE p.strategy_id = $1 AND p.asset_id = $2")
        .bind(strategy_id).bind(asset_id).fetch_optional(pool).await?;
    match id {
        Some((id,)) => documents::get(pool, id).await,
        None => Ok(None),
    }
}

async fn folder(conn: &mut PgConnection, parent: Option<Uuid>, title: &str)
    -> anyhow::Result<Uuid> {
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM documents WHERE kind = 'folder'          AND parent_id IS NOT DISTINCT FROM $1 AND title = $2          ORDER BY created_at LIMIT 1")
        .bind(parent).bind(title).fetch_optional(&mut *conn).await?;
    if let Some((id,)) = existing { return Ok(id); }
    Ok(documents::create_with(conn, parent, "folder", title).await?.id)
}

fn starter_content() -> Value {
    let h = |title: &str| json!({"type":"heading","attrs":{"level":2},
        "content":[{"type":"text","text":title}]});
    json!({"type":"doc","content":[
        h("当前判断与结论"),
        h("核心证据（事实 / 市场预期 / 推断）"),
        h("买入、加仓、减仓与退出条件"),
        h("证伪条件与待验证问题"),
        h("重要判断变化与交易复盘")
    ]})
}

/// Explicit creation endpoint. Serializes concurrent calls and avoids orphan pages.
/// Existing users' assets are provisioned lazily when they open the link.
pub async fn ensure(pool: &PgPool, strategy_id: Uuid, asset_id: Uuid)
    -> anyhow::Result<Option<documents::Document>> {
    let mut tx = pool.begin().await?;
    let strategy: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM paper_trading_strategies WHERE id = $1 FOR UPDATE")
        .bind(strategy_id).fetch_optional(&mut *tx).await?;
    let Some((strategy_name,)) = strategy else { return Ok(None); };
    let asset: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT a.symbol, a.name, a.provider, a.provider_id          FROM portfolio_assets a JOIN paper_trading_strategies s ON s.portfolio_id = a.portfolio_id          WHERE s.id = $1 AND a.id = $2")
        .bind(strategy_id).bind(asset_id).fetch_optional(&mut *tx).await?;
    let Some((symbol, name, _provider, _provider_id)) = asset else { return Ok(None); };
    let linked: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT document_id FROM paper_strategy_documents WHERE strategy_id = $1 AND asset_id = $2")
        .bind(strategy_id).bind(asset_id).fetch_optional(&mut *tx).await?;
    if let Some((Some(id),)) = linked {
        if documents::get_meta(&mut tx, id).await?.is_some() {
            tx.commit().await?;
            return documents::get(pool, id).await;
        }
    }

    // A single transaction-scoped advisory lock protects the common root from
    // duplicate folder creation when different strategies initialize together.
    sqlx::query("SELECT pg_advisory_xact_lock(701118)")
        .execute(&mut *tx).await?;
    let root = folder(&mut tx, None, ROOT).await?;
    let group_name = format!("{} · {}", strategy_name, &strategy_id.to_string()[..8]);
    let group = folder(&mut tx, Some(root), &group_name).await?;
    // Use a stable asset identifier to avoid symbol/name collisions.
    let label = format!("{} {} · {}", symbol, name_safe(&name), &asset_id.to_string()[..8]);
    let asset_folder = folder(&mut tx, Some(group), &label).await?;
    let page = documents::create_with(&mut tx, Some(asset_folder), "page", "全局交易策略").await?;
    sqlx::query("UPDATE documents SET content = $2 WHERE id = $1")
        .bind(page.id).bind(starter_content()).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO paper_strategy_documents (strategy_id, asset_id, document_id)          VALUES ($1, $2, $3) ON CONFLICT (strategy_id, asset_id)          DO UPDATE SET document_id = EXCLUDED.document_id")
        .bind(strategy_id).bind(asset_id).bind(page.id).execute(&mut *tx).await?;
    tx.commit().await?;
    documents::get(pool, page.id).await
}

fn name_safe(name: &str) -> String {
    name.chars().take(70).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateOutcome { Updated, Unchanged, Stale, Missing }

pub async fn update(pool: &PgPool, strategy_id: Uuid, asset_id: Uuid,
    expected_updated_at: OffsetDateTime, content: &Value) -> anyhow::Result<UpdateOutcome> {
    if !content.is_object() || content.get("type").and_then(Value::as_str) != Some("doc")
        || !content.get("content").is_some_and(Value::is_array) {
        return Err(anyhow!("content must be a TipTap document JSON object"));
    }
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid, Option<Value>, OffsetDateTime)> = sqlx::query_as(
        "SELECT d.id, d.content, d.updated_at          FROM paper_strategy_documents p          JOIN paper_trading_strategies s ON s.id = p.strategy_id          JOIN portfolio_assets a ON a.id = p.asset_id AND a.portfolio_id = s.portfolio_id          JOIN documents d ON d.id = p.document_id          WHERE p.strategy_id = $1 AND p.asset_id = $2 FOR UPDATE OF d")
        .bind(strategy_id).bind(asset_id).fetch_optional(&mut *tx).await?;
    let Some((id, old, updated_at)) = row else { return Ok(UpdateOutcome::Missing); };
    if old.as_ref() == Some(content) { return Ok(UpdateOutcome::Unchanged); }
    if updated_at != expected_updated_at { return Ok(UpdateOutcome::Stale); }
    sqlx::query("UPDATE documents SET content = $2, updated_at = now() WHERE id = $1")
        .bind(id).bind(content).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(UpdateOutcome::Updated)
}
