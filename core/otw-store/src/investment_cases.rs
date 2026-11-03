//! Investment Case (Phase 1 MVP): per-user research entity for one instrument,
//! with an OPTIONAL association to one Editor document (its research body).
//!
//! The case is authoritative on its own: decisions / thesis / current state do
//! not depend on the page. `document_id` is a detachable reference — deleting
//! the page from the Editor unlinks it (document_id → NULL, FK ON DELETE
//! SET NULL) instead of cascading into the case or failing. When a research
//! page is needed again (re-entering research after a delete), a fresh page is
//! created under the shared root folder [`RESEARCH_FOLDER_TITLE`] and relinked.
//!
//! Positions/PnL are NOT stored here. Live data is aggregated from the Portfolio
//! Tracker — the single source of truth — using the exact same financial
//! semantics as the portfolio views (`portfolios::position_usd` USD ledger:
//! each operation converted at ITS OWN date's FX, then converted to the display
//! currency at today's FX). Investment Case never introduces a second
//! computation standard; the displayed "avg cost" is the USD-ledger figure, NOT
//! a historical native-currency average (this is the existing product semantics
//! of the Portfolio pages too).
//!
//! Ownership: `investment_cases.user_id` follows the `sessions` FK convention
//! (users.id ON DELETE CASCADE). OTW is currently single-user — portfolios /
//! documents / watchlists carry no user column. This user_id is the case's own
//! ownership anchor (all case/decision handlers must check it) and the seam for
//! a future multi-user migration; it does NOT by itself make the app multi-tenant.
//!
//! Decision snapshots are computed server-side at creation from the SAME
//! aggregation used by the live endpoint, then frozen: there is no UPDATE path
//! for snapshot columns in this module.

use anyhow::Context;
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::documents::{self, DocumentMeta};
use crate::journal_fx;
use crate::market_quotes;
use crate::portfolios::{self, Asset};

// ── Types ────────────────────────────────────────────────────────────────────

/// Root folder that research pages (one per case) live in. Case research pages
/// are auto-placed under this folder instead of the tree root. Kept in sync
/// with the folder the migration 9000000104 creates.
pub const RESEARCH_FOLDER_TITLE: &str = "股票研究";

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InvestmentCase {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub provider_id: String,
    pub symbol: String,
    pub name: String,
    pub exchange: String,
    pub currency: String,
    pub asset_class: String,
    /// The case's current research page in the Editor. Optional: deleting that
    /// page from the Editor unlinks it (NULL); a fresh page is created and
    /// relinked the next time research is entered for this case.
    pub document_id: Option<Uuid>,
    /// Projection of the LATEST decision/change record — the four fields below
    /// are derived data, only updated transactionally by [`record_change`],
    /// never edited standalone.
    pub current_action: Option<String>,
    pub current_confidence: Option<i16>,
    pub valuation_view: Option<String>,
    pub current_biggest_question: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DecisionRecord {
    pub id: Uuid,
    pub investment_case_id: Uuid,
    /// Action taken after this change (legacy name kept on the wire as well).
    pub decision_type: String,
    /// RE-DEFINED as confidence AFTER the change (DB column name kept — no rename).
    pub confidence: i16,
    /// Confidence before the change, copied from current state at record time.
    pub confidence_before: Option<i16>,
    /// The one required high-frequency field ("what changed since last time").
    pub what_happened: String,
    pub impact_direction: Option<String>,
    pub affected_assumption_id: Option<Uuid>,
    /// Text SNAPSHOT of the assumption at record time — later thesis edits must
    /// not rewrite history.
    pub affected_assumption_text: String,
    /// Valuation judgement AT THIS ROUND (mirrors the case-level vocabulary;
    /// carries the round's own reading, later rounds never overwrite it).
    pub valuation_view: Option<String>,
    /// The biggest open question AT THIS ROUND (frozen for history).
    pub biggest_question: String,
    /// Legacy decision fields (deprecated for new records, kept for old ones).
    pub reason: String,
    pub evidence: String,
    pub counter_evidence: String,
    pub invalidation: String,
    pub watch_items: String,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
    pub snapshot_provider: String,
    pub snapshot_provider_id: String,
    pub snapshot_symbol: String,
    pub snapshot_name: String,
    pub snapshot_exchange: String,
    pub snapshot_currency: String,
    pub snapshot_price: Option<f64>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub snapshot_price_at: Option<OffsetDateTime>,
    pub snapshot_quantity: f64,
    pub snapshot_avg_cost: Option<f64>,
    pub snapshot_cost_basis: Option<f64>,
    pub snapshot_market_value: Option<f64>,
    pub snapshot_unrealized_pnl: Option<f64>,
    pub snapshot_unrealized_pct: Option<f64>,
}

/// Per-portfolio breakdown of one instrument's position (instrument currency).
#[derive(Debug, Clone, Serialize)]
pub struct PortfolioSlice {
    pub portfolio_id: Uuid,
    pub portfolio_name: String,
    pub asset_id: Uuid,
    pub quantity: f64,
    pub avg_cost: Option<f64>,
    pub cost_basis: f64,
    pub market_value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
}

/// One operation with its portfolio context (merged across portfolios).
#[derive(Debug, Clone, Serialize)]
pub struct OperationView {
    pub asset_id: Uuid,
    pub portfolio_id: Uuid,
    pub portfolio_name: String,
    pub side: String,
    #[serde(with = "date_fmt")]
    pub op_date: Date,
    pub quantity: f64,
    pub price: f64,
    pub fee: f64,
    pub currency: String,
}

/// Aggregated live position for one instrument across ALL of the user's
/// portfolios (single-user model: every portfolio belongs to the user).
/// All amounts in the case's instrument currency.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentPosition {
    pub quantity: f64,
    pub cost_basis: f64,
    pub avg_cost: Option<f64>,
    pub market_value: Option<f64>,
    pub unrealized_pnl: Option<f64>,
    pub unrealized_pct: Option<f64>,
    pub price: Option<f64>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub price_at: Option<OffsetDateTime>,
    pub portfolios: Vec<PortfolioSlice>,
    pub operations: Vec<OperationView>,
}

mod date_fmt {
    use serde::Serializer;
    use time::{format_description::well_known::Iso8601, Date};
    pub fn serialize<S: Serializer>(d: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.format(&Iso8601::DATE).map_err(serde::ser::Error::custom)?)
    }
}

// ── Case queries ─────────────────────────────────────────────────────────────

macro_rules! case_select {
    ($($rest:tt)*) => {
        sqlx::query_as::<_, InvestmentCase>(concat!(
            "SELECT id, user_id, provider, provider_id, symbol, name, exchange, currency, asset_class, document_id, current_action, current_confidence, valuation_view, current_biggest_question, created_at FROM investment_cases ",
            $($rest)*
        ))
    };
}

pub async fn get_case(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<InvestmentCase>> {
    Ok(case_select!("WHERE id = $1").bind(id).fetch_optional(pool).await?)
}

pub async fn get_case_by_instrument(
    pool: &PgPool,
    user_id: Uuid,
    provider: &str,
    provider_id: &str,
) -> anyhow::Result<Option<InvestmentCase>> {
    Ok(case_select!("WHERE user_id = $1 AND provider = $2 AND provider_id = $3")
        .bind(user_id)
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(pool)
        .await?)
}

/// Find the case that references this Editor document (used by the documents API
/// to guard deletes and enforce ownership on research documents).
pub async fn find_case_by_document_id(
    pool: &PgPool,
    document_id: Uuid,
) -> anyhow::Result<Option<InvestmentCase>> {
    Ok(case_select!("WHERE document_id = $1")
        .bind(document_id)
        .fetch_optional(pool)
        .await?)
}

/// One row of the Research home list: the case plus its latest change time
/// (None when no change/decision record exists yet). Live position is NOT part
/// of this row — the list page pulls it per case from the existing /live
/// aggregation, which stays the single place positions/P&L are computed.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InvestmentCaseRow {
    pub id: Uuid,
    pub provider: String,
    pub provider_id: String,
    pub symbol: String,
    pub name: String,
    pub exchange: String,
    pub currency: String,
    pub asset_class: String,
    pub document_id: Option<Uuid>,
    pub current_action: Option<String>,
    pub current_confidence: Option<i16>,
    pub valuation_view: Option<String>,
    pub current_biggest_question: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub latest_change_at: Option<OffsetDateTime>,
}

/// The user's investment cases for the Research home list, most recently
/// changed first (cases never changed come last, ordered by creation time).
pub async fn list_cases_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> anyhow::Result<Vec<InvestmentCaseRow>> {
    Ok(sqlx::query_as(
        "SELECT ic.id, ic.provider, ic.provider_id, ic.symbol, ic.name, ic.exchange,
                ic.currency, ic.asset_class, ic.document_id,
                ic.current_action, ic.current_confidence, ic.valuation_view,
                ic.current_biggest_question, ic.created_at,
                (SELECT dr.recorded_at FROM decision_records dr
                  WHERE dr.investment_case_id = ic.id
                  ORDER BY dr.recorded_at DESC, dr.id DESC LIMIT 1) AS latest_change_at
         FROM investment_cases ic
         WHERE ic.user_id = $1
         ORDER BY latest_change_at DESC NULLS LAST, ic.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

fn research_document_title(symbol: &str, name: &str) -> String {
    let symbol = symbol.trim();
    let name = name.trim();
    match (symbol.is_empty(), name.is_empty()) {
        (false, false) => format!("{symbol} {name} 研究"),
        (false, true) => format!("{symbol} 研究"),
        _ => "研究".to_string(),
    }
}

/// Ensure a case exists for (user_id, provider, provider_id), deriving all
/// identity fields server-side from the given portfolio asset. Idempotent and
/// concurrency-safe:
///
/// - fast path: existing case → return it (creating a fresh research page when
///   the previous one was deleted from the Editor);
/// - slow path: transaction that re-checks, then creates the research document
///   (under the “股票研究” folder) and the case. If the case INSERT loses a
///   unique-constraint race, the whole transaction (INCLUDING the already-
///   created document) is rolled back and the winner's case is returned —
///   never an orphan document, never two cases.
pub async fn ensure_case(
    pool: &PgPool,
    user_id: Uuid,
    asset: &Asset,
) -> anyhow::Result<InvestmentCase> {
    if let Some(c) = get_case_by_instrument(pool, user_id, &asset.provider, &asset.provider_id)
        .await?
    {
        return ensure_case_document(pool, &c).await;
    }

    let mut tx = pool.begin().await?;
    // Re-check inside the transaction (the fast path may have raced).
    let existing: Option<InvestmentCase> = case_select!(
        "WHERE user_id = $1 AND provider = $2 AND provider_id = $3 FOR UPDATE"
    )
    .bind(user_id)
    .bind(&asset.provider)
    .bind(&asset.provider_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(c) = existing {
        tx.commit().await?;
        return ensure_case_document(pool, &c).await;
    }

    let title = research_document_title(&asset.symbol, &asset.name);
    let folder_id = ensure_research_folder(&mut *tx).await?;
    let doc = documents::create_with(&mut *tx, Some(folder_id), "page", &title).await?;

    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO investment_cases
            (id, user_id, provider, provider_id, symbol, name, exchange, currency, asset_class, document_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (user_id, provider, provider_id) DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&asset.provider)
    .bind(&asset.provider_id)
    .bind(&asset.symbol)
    .bind(&asset.name)
    // portfolio_assets carries no exchange column; leave it empty for now
    // (a future unified Instrument Service would backfill it).
    .bind("")
    .bind(&asset.currency)
    .bind(&asset.asset_class)
    .bind(doc.id)
    .fetch_optional(&mut *tx)
    .await?;

    let case = match inserted {
        Some(_) => case_select!(
            "WHERE user_id = $1 AND provider = $2 AND provider_id = $3"
        )
        .bind(user_id)
        .bind(&asset.provider)
        .bind(&asset.provider_id)
        .fetch_optional(&mut *tx)
        .await?
        .context("case row missing right after insert")?,
        None => {
            // Unique-constraint race lost: roll back so the research document we
            // just created is NOT orphaned, then make sure the winner's case has
            // its research page and return it.
            tx.rollback().await?;
            let winner = get_case_by_instrument(
                pool,
                user_id,
                &asset.provider,
                &asset.provider_id,
            )
            .await?
            .context("concurrent ensure left no case behind")?;
            return ensure_case_document(pool, &winner).await;
        }
    };
    tx.commit().await?;
    Ok(case)
}

/// Make sure the case has a research page again: when `document_id` is NULL
/// (its page was deleted from the Editor), a fresh page is created under the
/// research folder and relinked. Otherwise the case is returned unchanged.
async fn ensure_case_document(
    pool: &PgPool,
    case: &InvestmentCase,
) -> anyhow::Result<InvestmentCase> {
    if case.document_id.is_some() {
        return Ok(case.clone());
    }
    let mut tx = pool.begin().await?;
    // Re-check under an exclusive lock (two callers may race to recreate).
    let current: Option<InvestmentCase> =
        case_select!("WHERE id = $1 FOR UPDATE").bind(case.id).fetch_optional(&mut *tx).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(case.clone());
    };
    if current.document_id.is_none() {
        attach_fresh_document(&mut *tx, &current).await?;
    }
    let updated = case_select!("WHERE id = $1")
        .bind(current.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(updated)
}

/// Find the case's research page, creating a fresh (empty) one under the
/// research folder when the current link is NULL or points at a missing
/// document. Returns `None` only when no such case exists. Used by the
/// case-scoped API (`POST /investment-cases/{id}/research-document`) so the
/// page is (re)created on demand, never as a GET side effect.
pub async fn ensure_research_document(
    pool: &PgPool,
    case_id: Uuid,
) -> anyhow::Result<Option<DocumentMeta>> {
    // Fast path without side effects: an existing, loadable page is returned.
    if let Some(case) = get_case(pool, case_id).await? {
        if let Some(doc_id) = case.document_id {
            let mut conn = pool.acquire().await?;
            if let Some(meta) = documents::get_meta(&mut conn, doc_id).await? {
                return Ok(Some(meta));
            }
        }
    }

    // Slow path: lock the case, (re)create and relink when needed.
    let mut tx = pool.begin().await?;
    let case: Option<InvestmentCase> =
        case_select!("WHERE id = $1 FOR UPDATE").bind(case_id).fetch_optional(&mut *tx).await?;
    let Some(case) = case else {
        tx.rollback().await?;
        return Ok(None);
    };
    let meta = match case.document_id {
        Some(doc_id) => match documents::get_meta(&mut *tx, doc_id).await? {
            // Stale pointer (page deleted out-of-band): recreate.
            Some(m) => m,
            None => attach_fresh_document(&mut *tx, &case).await?,
        },
        None => attach_fresh_document(&mut *tx, &case).await?,
    };
    tx.commit().await?;
    Ok(Some(meta))
}

/// The shared root folder for research pages, created once when missing and
/// reused afterwards. Keeps research articles out of the tree root.
async fn ensure_research_folder(conn: &mut PgConnection) -> anyhow::Result<Uuid> {
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM documents
         WHERE kind = 'folder' AND title = $1 AND parent_id IS NULL
         ORDER BY created_at LIMIT 1",
    )
    .bind(RESEARCH_FOLDER_TITLE)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }
    let folder = documents::create_with(conn, None, "folder", RESEARCH_FOLDER_TITLE).await?;
    Ok(folder.id)
}

/// Create a fresh (empty) research page for the case under the research folder
/// and link it. The caller must hold the case row (or be inside the
/// `ensure_case` transaction) so two recreates cannot double-create.
async fn attach_fresh_document(
    conn: &mut PgConnection,
    case: &InvestmentCase,
) -> anyhow::Result<DocumentMeta> {
    let folder_id = ensure_research_folder(conn).await?;
    let title = research_document_title(&case.symbol, &case.name);
    let doc = documents::create_with(conn, Some(folder_id), "page", &title).await?;
    sqlx::query("UPDATE investment_cases SET document_id = $2 WHERE id = $1")
        .bind(case.id)
        .bind(doc.id)
        .execute(&mut *conn)
        .await?;
    Ok(doc)
}

// ── Live aggregation ─────────────────────────────────────────────────────────

/// Aggregate the live position for the case's instrument across all portfolios
/// (single-user model), in the case's instrument currency, with the SAME
/// financial semantics as the Portfolio views:
///
/// - per asset: `portfolios::position_usd` (weighted-average USD ledger, each
///   operation at its own date's FX; buy fees into basis, sell fees out of
///   proceeds), then converted USD → case currency at today's FX;
/// - price: among assets that carry a price, the one with the LATEST
///   `last_price_at` (selection policy; no refresh trigger in Phase 1);
/// - aggregate: quantity = Σ, cost_basis = Σ (same currency), avg_cost =
///   basis/qty, market_value = price × qty, unrealized = MV − basis,
///   unrealized_pct = unrealized / basis.
pub async fn instrument_position(
    pool: &PgPool,
    case: &InvestmentCase,
) -> anyhow::Result<InstrumentPosition> {
    let today = OffsetDateTime::now_utc().date();

    let assets: Vec<Asset> = sqlx::query_as(
        "SELECT * FROM portfolio_assets WHERE provider = $1 AND provider_id = $2 \
         AND NOT EXISTS (SELECT 1 FROM paper_trading_strategies pts \
                         WHERE pts.portfolio_id = portfolio_assets.portfolio_id)",
    )
    .bind(&case.provider)
    .bind(&case.provider_id)
    .fetch_all(pool)
    .await?;

    let mut names: Vec<(Uuid, String)> = Vec::new();
    if !assets.is_empty() {
        let ids: Vec<Uuid> = assets.iter().map(|a| a.portfolio_id).collect();
        names = sqlx::query_as("SELECT id, name FROM portfolios WHERE id = ANY($1) ORDER BY position")
            .bind(&ids)
            .fetch_all(pool)
            .await?;
    }
    let name_of = |pid: Uuid| -> String {
        names
            .iter()
            .find(|(id, _)| *id == pid)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "—".to_string())
    };

    // Price selection: the shared Market Quote (freshly ensured by the /live endpoint)
    // wins when it is at least as new as the newest asset-level last price; otherwise the
    // latest persisted asset price is used. Either way the price is USD, then converted to
    // the case currency below — so every portfolio slice and the totals value the SAME
    // current market price, no matter which module refreshed it last.
    let local_best: Option<(f64, Option<OffsetDateTime>)> = assets
        .iter()
        .filter_map(|a| a.last_price_usd.map(|p| (p, a.last_price_at)))
        .max_by(|x, y| x.1.cmp(&y.1));
    let shared = match market_quotes::get(pool, &case.provider, &case.provider_id).await? {
        Some(q) if q.price_usd > 0.0 => Some((q.price_usd, q.quote_at)),
        _ => None,
    };
    let price_usd = match (local_best, shared) {
        (None, None) => None,
        (Some((p, at)), None) => Some((p, at.unwrap_or_else(OffsetDateTime::now_utc))),
        (None, Some(s)) => Some(s),
        (Some((lp, la)), Some((sp, sa))) => {
            let shared_newer = match la {
                Some(lt) => sa >= lt,
                None => true,
            };
            if shared_newer {
                Some((sp, sa))
            } else {
                Some((lp, la.unwrap_or(sa)))
            }
        }
    };
    let (price, price_at) = match price_usd {
        Some((p, at)) => (
            journal_fx::convert(pool, p, "USD", &case.currency, today)
                .await?
                .or(Some(p)),
            Some(at),
        ),
        None => (None, None),
    };

    let mut quantity = 0.0_f64;
    let mut basis = 0.0_f64;
    let mut slices = Vec::new();
    let mut operations = Vec::new();

    for asset in &assets {
        let ops = portfolios::list_operations(pool, asset.id).await?;
        let (q, basis_usd, _realized_usd) =
            portfolios::position_usd(pool, &ops, &asset.currency).await?;
        let basis_ccy = journal_fx::convert(pool, basis_usd, "USD", &case.currency, today)
            .await?
            .unwrap_or(basis_usd);
        quantity += q;
        basis += basis_ccy;

        let pname = name_of(asset.portfolio_id);
        let mv = price.map(|p| p * q);
        slices.push(PortfolioSlice {
            portfolio_id: asset.portfolio_id,
            portfolio_name: pname.clone(),
            asset_id: asset.id,
            quantity: q,
            avg_cost: if q > 0.0 { Some(basis_ccy / q) } else { None },
            cost_basis: basis_ccy,
            market_value: mv,
            unrealized_pnl: mv.map(|v| v - basis_ccy),
        });

        for op in ops {
            operations.push(OperationView {
                asset_id: op.asset_id,
                portfolio_id: asset.portfolio_id,
                portfolio_name: pname.clone(),
                side: op.side,
                op_date: op.op_date,
                quantity: op.quantity,
                price: op.price,
                fee: op.fee,
                currency: asset.currency.clone(),
            });
        }
    }

    let market_value = price.map(|p| p * quantity);
    let unrealized = market_value.map(|mv| mv - basis);

    Ok(InstrumentPosition {
        quantity,
        cost_basis: basis,
        avg_cost: if quantity > 0.0 { Some(basis / quantity) } else { None },
        market_value,
        unrealized_pnl: unrealized,
        unrealized_pct: match (unrealized, basis) {
            (Some(u), b) if b > 0.0 => Some(u / b * 100.0),
            _ => None,
        },
        price,
        price_at,
        portfolios: slices,
        operations,
    })
}

// ── Change Records (the high-frequency interaction) ─────────────────────────

/// User-supplied change-record fields. Snapshot numbers are NEVER accepted from
/// the client — they are computed server-side by [`instrument_position`].
/// `what_happened` is the only required high-frequency field; the legacy
/// decision text fields stay accepted (deprecated) for backward compatibility.
pub struct NewChange<'a> {
    /// Action taken after this change (observe/hold/buy/add/reduce/exit).
    pub decision_type: &'a str,
    /// Confidence AFTER the change (DB column `confidence`).
    pub confidence: i16,
    pub what_happened: &'a str,
    pub impact_direction: Option<&'a str>,
    pub affected_assumption_id: Option<Uuid>,
    /// Valuation judgement for this round (cheap/fair/expensive/unclear).
    pub valuation_view: Option<&'a str>,
    /// Biggest open question for this round.
    pub biggest_question: &'a str,
    pub reason: &'a str,
    pub evidence: &'a str,
    pub counter_evidence: &'a str,
    pub invalidation: &'a str,
    pub watch_items: &'a str,
}

/// Record a change ATOMICALLY:
///
/// 1. lock the case row and read the CURRENT confidence;
/// 2. create the immutable change record — `confidence_before` = current
///    confidence, `affected_assumption_text` = snapshot of the assumption's
///    text at this moment (later thesis edits never rewrite history);
/// 3. project the new state: `current_action` / `current_confidence` are
///    updated in the SAME transaction (Change Log = history; Current State =
///    projection of the latest record);
/// 4. commit.
///
/// There is no UPDATE path for change records: snapshots never change.
pub async fn record_change(
    pool: &PgPool,
    case: &InvestmentCase,
    input: &NewChange<'_>,
    pos: &InstrumentPosition,
) -> anyhow::Result<DecisionRecord> {
    let mut tx = pool.begin().await?;

    // Lock the case row; read the current confidence projection.
    let (current_confidence,): (Option<i16>,) = sqlx::query_as(
        "SELECT current_confidence FROM investment_cases WHERE id = $1 FOR UPDATE",
    )
    .bind(case.id)
    .fetch_one(&mut *tx)
    .await?;

    // Snapshot the assumption text if one is referenced (missing row → no link).
    let mut affected_text = String::new();
    if let Some(aid) = input.affected_assumption_id {
        let found: Option<(String,)> =
            sqlx::query_as("SELECT text FROM thesis_assumptions WHERE id = $1")
                .bind(aid)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((text,)) = found {
            affected_text = text;
        }
    }
    let affected_id = if affected_text.is_empty() {
        None
    } else {
        input.affected_assumption_id
    };

    let record = sqlx::query_as(
        "INSERT INTO decision_records (
            id, investment_case_id, decision_type, confidence, confidence_before,
            what_happened, impact_direction,
            affected_assumption_id, affected_assumption_text,
            reason, evidence, counter_evidence, invalidation, watch_items,
            recorded_at,
            snapshot_provider, snapshot_provider_id, snapshot_symbol, snapshot_name,
            snapshot_exchange, snapshot_currency,
            snapshot_price, snapshot_price_at,
            snapshot_quantity, snapshot_avg_cost, snapshot_cost_basis,
            snapshot_market_value, snapshot_unrealized_pnl, snapshot_unrealized_pct,
            valuation_view, biggest_question
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9,
            $10, $11, $12, $13, $14, $15,
            $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29,
            $30, $31
         )
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(case.id)
    .bind(input.decision_type)
    .bind(input.confidence)
    .bind(current_confidence)
    .bind(input.what_happened)
    .bind(input.impact_direction)
    .bind(affected_id)
    .bind(affected_text)
    .bind(input.reason)
    .bind(input.evidence)
    .bind(input.counter_evidence)
    .bind(input.invalidation)
    .bind(input.watch_items)
    .bind(OffsetDateTime::now_utc())
    .bind(&case.provider)
    .bind(&case.provider_id)
    .bind(&case.symbol)
    .bind(&case.name)
    .bind(&case.exchange)
    .bind(&case.currency)
    .bind(pos.price)
    .bind(pos.price_at)
    .bind(pos.quantity)
    .bind(pos.avg_cost)
    .bind(Some(pos.cost_basis))
    .bind(pos.market_value)
    .bind(pos.unrealized_pnl)
    .bind(pos.unrealized_pct)
    .bind(input.valuation_view)
    .bind(input.biggest_question)
    .fetch_one(&mut *tx)
    .await?;

    // Project the new current judgement in the same transaction. current_action /
    // current_confidence / valuation_view / current_biggest_question on the case
    // are derived data — mirrors of the LATEST record, never edited standalone.
    sqlx::query(
        "UPDATE investment_cases
            SET current_action = $2,
                current_confidence = $3,
                valuation_view = $4,
                current_biggest_question = $5
          WHERE id = $1",
    )
    .bind(case.id)
    .bind(input.decision_type)
    .bind(input.confidence)
    .bind(input.valuation_view)
    .bind(input.biggest_question)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(record)
}

// ── Thesis (low frequency) ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Thesis {
    pub id: Uuid,
    pub investment_case_id: Uuid,
    pub core_thesis: String,
    pub invalidation: String,
    pub major_risks: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ThesisAssumption {
    pub id: Uuid,
    pub investment_case_id: Uuid,
    pub position: i16,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Read the thesis + its assumptions. LAZY: returns None when the user has
/// never saved a thesis — reads never create rows.
pub async fn get_thesis(
    pool: &PgPool,
    case_id: Uuid,
) -> anyhow::Result<Option<(Thesis, Vec<ThesisAssumption>)>> {
    let thesis: Option<Thesis> =
        sqlx::query_as("SELECT * FROM investment_theses WHERE investment_case_id = $1")
            .bind(case_id)
            .fetch_optional(pool)
            .await?;
    let Some(thesis) = thesis else { return Ok(None) };
    let assumptions: Vec<ThesisAssumption> = sqlx::query_as(
        "SELECT * FROM thesis_assumptions WHERE investment_case_id = $1 ORDER BY position",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await?;
    Ok(Some((thesis, assumptions)))
}

/// Save the thesis (UPSERT — first save creates the row, later saves update it)
/// and sync its assumptions in one transaction. Assumptions are matched by
/// (case, position): text edits keep the row id (Change Record links stay
/// valid); positions absent from the payload are deleted (their links fall to
/// ON DELETE SET NULL — history text snapshots survive).
pub async fn upsert_thesis(
    pool: &PgPool,
    case_id: Uuid,
    core_thesis: &str,
    invalidation: &str,
    major_risks: &str,
    assumptions: &[(i16, String)],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO investment_theses (id, investment_case_id, core_thesis, invalidation, major_risks)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (investment_case_id) DO UPDATE
         SET core_thesis = EXCLUDED.core_thesis,
             invalidation = EXCLUDED.invalidation,
             major_risks = EXCLUDED.major_risks,
             updated_at = now()",
    )
    .bind(Uuid::new_v4())
    .bind(case_id)
    .bind(core_thesis)
    .bind(invalidation)
    .bind(major_risks)
    .execute(&mut *tx)
    .await?;

    // Positions present in the payload: upsert (keep existing id via the
    // (case_id, position) unique key so change-record links survive edits).
    for (position, text) in assumptions {
        sqlx::query(
            "INSERT INTO thesis_assumptions (id, investment_case_id, position, text)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (investment_case_id, position) DO UPDATE
             SET text = EXCLUDED.text, updated_at = now()",
        )
        .bind(Uuid::new_v4())
        .bind(case_id)
        .bind(position)
        .bind(text)
        .execute(&mut *tx)
        .await?;
    }
    // Positions no longer present: delete (FK ON DELETE SET NULL on records).
    sqlx::query(
        "DELETE FROM thesis_assumptions WHERE investment_case_id = $1 AND NOT (position = ANY($2))",
    )
    .bind(case_id)
    .bind(&assumptions.iter().map(|(p, _)| *p).collect::<Vec<i16>>())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}



pub async fn list_decisions(pool: &PgPool, case_id: Uuid) -> anyhow::Result<Vec<DecisionRecord>> {
    Ok(sqlx::query_as(
        "SELECT * FROM decision_records WHERE investment_case_id = $1 ORDER BY recorded_at DESC, id",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await?)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_document_title_formats() {
        assert_eq!(research_document_title("601118", "海南橡胶"), "601118 海南橡胶 研究");
        assert_eq!(research_document_title("601118", ""), "601118 研究");
        assert_eq!(research_document_title("", ""), "研究");
    }
}
