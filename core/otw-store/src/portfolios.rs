//! Storage for the Portfolio Tracker module.
//!
//! Portfolios hold assets; each asset is an operations ledger (buy/sell) from which current
//! quantity, average cost and PnL derive. Prices are stored in USD (`last_price_usd`); values are
//! converted to the portfolio's display currency at read time via the journal's USD-based fx_rates
//! (carry-forward). Daily valuation snapshots build a value time series. Single-user: no owner
//! scoping. Distinct from `mportfolios` (the Dataroma cache).

use anyhow::Context;
use serde::Serialize;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::journal_fx;
use crate::market_quotes;

// ── Row types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Portfolio {
    pub id: Uuid,
    pub name: String,
    /// One line, shown on the card next to the figures.
    pub description: String,
    /// The long note: investment thesis, purpose, rules. Free text.
    pub notes: String,
    pub currency: String,
    pub auto_refresh: bool,
    pub position: f64,
    #[serde(with = "ts_opt")]
    pub refreshed_at: Option<OffsetDateTime>,
    #[serde(with = "ts")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ts")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Asset {
    pub id: Uuid,
    pub portfolio_id: Uuid,
    pub asset_class: String,
    pub provider: String,
    pub provider_id: String,
    pub symbol: String,
    pub name: String,
    /// Currency the asset's operations (price, fee) are entered in. Spot prices stay USD.
    pub currency: String,
    pub last_price_usd: Option<f64>,
    #[serde(with = "ts_opt")]
    pub last_price_at: Option<OffsetDateTime>,
    /// Overriding live-price source (NULL → price via `provider`/`provider_id`).
    pub spot_provider: Option<String>,
    /// Provider-specific ticker for `spot_provider` (e.g. Binance "BTCUSDT"), user-entered.
    pub spot_symbol: String,
    /// Last reconcile outcome: 'ok' | 'unresolved' | 'manual'. Only 'ok' is priced by refresh.
    pub recon_status: String,
    #[serde(with = "ts_opt")]
    pub recon_checked_at: Option<OffsetDateTime>,
    pub recon_note: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Operation {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub side: String,
    #[serde(with = "date_fmt")]
    pub op_date: Date,
    pub quantity: f64,
    pub price: f64,
    pub fee: f64,
    pub note: String,
    /// When the operation was actually entered — the tie-break that restores
    /// the real order of same-day operations in position accounting.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Snapshot {
    #[serde(with = "date_fmt")]
    pub snap_date: Date,
    pub currency: String,
    pub market_value: f64,
    pub cost_basis: f64,
}

// ── Derived (computed) views returned to the API ──────────────────────────────

/// An asset with its ledger-derived position and PnL, all amounts in the portfolio currency.
#[derive(Debug, Clone, Serialize)]
pub struct AssetView {
    #[serde(flatten)]
    pub asset: Asset,
    /// Current units held (Σ buy − Σ sell).
    pub quantity: f64,
    /// Average cost per unit of the open position, in portfolio currency (incl. buy fees).
    pub avg_cost: Option<f64>,
    /// Cost basis of the open position, in portfolio currency.
    pub cost_basis: f64,
    /// Latest price converted to portfolio currency, or None if no price yet.
    pub price: Option<f64>,
    /// Market value of the open position, in portfolio currency.
    pub market_value: Option<f64>,
    /// Unrealized PnL (market_value − cost_basis), portfolio currency.
    pub unrealized: Option<f64>,
    /// Realized PnL from closed quantity, portfolio currency.
    pub realized: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortfolioSummary {
    #[serde(flatten)]
    pub portfolio: Portfolio,
    pub asset_count: i64,
    pub market_value: f64,
    pub cost_basis: f64,
    pub unrealized: f64,
    pub realized: f64,
    /// Recent snapshot market values (oldest→newest) for the card sparkline.
    pub sparkline: Vec<f64>,
}

// ── Serde helpers ─────────────────────────────────────────────────────────────

mod ts {
    use serde::Serializer;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    pub fn serialize<S: Serializer>(t: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }
}
mod ts_opt {
    use serde::Serializer;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    pub fn serialize<S: Serializer>(t: &Option<OffsetDateTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?),
            None => s.serialize_none(),
        }
    }
}
mod date_fmt {
    use serde::Serializer;
    use time::{format_description::well_known::Iso8601, Date};
    pub fn serialize<S: Serializer>(d: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(
            &d.format(&Iso8601::DATE)
                .map_err(serde::ser::Error::custom)?,
        )
    }
}

const PF_COLS: &str = "id, name, description, notes, currency, auto_refresh, position, refreshed_at, created_at, updated_at";
const ASSET_COLS: &str = "id, portfolio_id, asset_class, provider, provider_id, symbol, name, currency, last_price_usd, last_price_at, spot_provider, spot_symbol, recon_status, recon_checked_at, recon_note";

// ── Portfolios CRUD ───────────────────────────────────────────────────────────

pub async fn list_portfolios(pool: &PgPool) -> anyhow::Result<Vec<Portfolio>> {
    let sql = format!("SELECT {PF_COLS} FROM portfolios ORDER BY position, created_at");
    Ok(sqlx::query_as::<_, Portfolio>(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
        .context("listing portfolios")?)
}

pub async fn get_portfolio(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Portfolio>> {
    let sql = format!("SELECT {PF_COLS} FROM portfolios WHERE id = $1");
    Ok(sqlx::query_as::<_, Portfolio>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("fetching portfolio")?)
}

pub async fn create_portfolio(
    pool: &PgPool,
    name: &str,
    description: &str,
    currency: &str,
) -> anyhow::Result<Portfolio> {
    let sql = format!(
        "INSERT INTO portfolios (id, name, description, currency, position) \
         VALUES ($1,$2,$3,$4, (SELECT COALESCE(MAX(position),0)+1 FROM portfolios)) \
         RETURNING {PF_COLS}"
    );
    Ok(sqlx::query_as::<_, Portfolio>(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::new_v4())
        .bind(name)
        .bind(description)
        .bind(currency)
        .fetch_one(pool)
        .await
        .context("creating portfolio")?)
}

/// Patch mutable portfolio fields. Any `None` is left unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn update_portfolio(
    pool: &PgPool,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    notes: Option<&str>,
    currency: Option<&str>,
    auto_refresh: Option<bool>,
) -> anyhow::Result<Option<Portfolio>> {
    let sql = format!(
        "UPDATE portfolios SET \
         name = COALESCE($2, name), \
         description = COALESCE($3, description), \
         notes = COALESCE($4, notes), \
         currency = COALESCE($5, currency), \
         auto_refresh = COALESCE($6, auto_refresh), \
         updated_at = now() \
         WHERE id = $1 RETURNING {PF_COLS}"
    );
    Ok(sqlx::query_as::<_, Portfolio>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(notes)
        .bind(currency)
        .bind(auto_refresh)
        .fetch_optional(pool)
        .await
        .context("updating portfolio")?)
}

pub async fn delete_portfolio(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM portfolios WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting portfolio")?;
    Ok(())
}

/// Portfolios with auto_refresh on (used by the daily job).
pub async fn list_auto_refresh(pool: &PgPool) -> anyhow::Result<Vec<Portfolio>> {
    let sql = format!("SELECT {PF_COLS} FROM portfolios WHERE auto_refresh ORDER BY position");
    Ok(sqlx::query_as::<_, Portfolio>(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
        .context("listing auto-refresh portfolios")?)
}

// ── Assets ────────────────────────────────────────────────────────────────────

pub async fn list_assets(pool: &PgPool, portfolio_id: Uuid) -> anyhow::Result<Vec<Asset>> {
    list_assets_on(&mut *pool.acquire().await?, portfolio_id).await
}

pub(crate) async fn list_assets_on(
    conn: &mut sqlx::PgConnection,
    portfolio_id: Uuid,
) -> anyhow::Result<Vec<Asset>> {
    let sql = format!(
        "SELECT {ASSET_COLS} FROM portfolio_assets WHERE portfolio_id = $1 ORDER BY symbol"
    );
    Ok(sqlx::query_as::<_, Asset>(sqlx::AssertSqlSafe(sql))
        .bind(portfolio_id)
        .fetch_all(conn)
        .await
        .context("listing assets")?)
}

pub async fn get_asset(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Asset>> {
    let sql = format!("SELECT {ASSET_COLS} FROM portfolio_assets WHERE id = $1");
    Ok(sqlx::query_as::<_, Asset>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("fetching asset")?)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_asset(
    pool: &PgPool,
    portfolio_id: Uuid,
    asset_class: &str,
    provider: &str,
    provider_id: &str,
    symbol: &str,
    name: &str,
    currency: &str,
) -> anyhow::Result<Asset> {
    let sql = format!(
        "INSERT INTO portfolio_assets (id, portfolio_id, asset_class, provider, provider_id, symbol, name, currency) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (portfolio_id, provider, provider_id) DO UPDATE SET symbol = EXCLUDED.symbol, name = EXCLUDED.name, currency = EXCLUDED.currency \
         RETURNING {ASSET_COLS}"
    );
    Ok(sqlx::query_as::<_, Asset>(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::new_v4())
        .bind(portfolio_id)
        .bind(asset_class)
        .bind(provider)
        .bind(provider_id)
        .bind(symbol)
        .bind(name)
        .bind(currency)
        .fetch_one(pool)
        .await
        .context("adding asset")?)
}

pub async fn delete_asset(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM portfolio_assets WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting asset")?;
    Ok(())
}

/// Store a freshly fetched USD spot for an asset.
pub async fn set_asset_price(pool: &PgPool, id: Uuid, price_usd: f64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE portfolio_assets SET last_price_usd = $2, last_price_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(price_usd)
    .execute(pool)
    .await
    .context("setting asset price")?;
    Ok(())
}

/// Patch an asset's price-source override and/or reconcile status. Any `None` is left unchanged.
/// Passing `Some(None)` for `spot_provider` clears the override (back to the default provider).
pub async fn update_asset(
    pool: &PgPool,
    id: Uuid,
    spot_provider: Option<Option<&str>>,
    spot_symbol: Option<&str>,
    recon_status: Option<&str>,
) -> anyhow::Result<Option<Asset>> {
    // `spot_provider` is tri-state: None = leave, Some(None) = clear, Some(Some(p)) = set. We flag
    // "touch this column" separately so a clear ($2=NULL) is distinguishable from "leave alone".
    let touch_provider = spot_provider.is_some();
    let provider_val = spot_provider.flatten();
    let sql = format!(
        "UPDATE portfolio_assets SET \
         spot_provider = CASE WHEN $2 THEN $3 ELSE spot_provider END, \
         spot_symbol   = COALESCE($4, spot_symbol), \
         recon_status  = COALESCE($5, recon_status) \
         WHERE id = $1 RETURNING {ASSET_COLS}"
    );
    Ok(sqlx::query_as::<_, Asset>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(touch_provider)
        .bind(provider_val)
        .bind(spot_symbol)
        .bind(recon_status)
        .fetch_optional(pool)
        .await
        .context("updating asset")?)
}

/// Record the outcome of a reconcile check for an asset.
pub async fn set_recon(pool: &PgPool, id: Uuid, status: &str, note: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE portfolio_assets SET recon_status = $2, recon_note = $3, recon_checked_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(note)
    .execute(pool)
    .await
    .context("setting recon status")?;
    Ok(())
}

// ── Operations ────────────────────────────────────────────────────────────────

pub async fn list_operations(pool: &PgPool, asset_id: Uuid) -> anyhow::Result<Vec<Operation>> {
    list_operations_on(&mut *pool.acquire().await?, asset_id).await
}

pub(crate) async fn list_operations_on(
    conn: &mut sqlx::PgConnection,
    asset_id: Uuid,
) -> anyhow::Result<Vec<Operation>> {
    Ok(sqlx::query_as::<_, Operation>(
        "SELECT id, asset_id, side, op_date, quantity, price, fee, note, created_at \
         FROM portfolio_operations WHERE asset_id = $1 ORDER BY op_date DESC, created_at DESC",
    )
    .bind(asset_id)
    .fetch_all(conn)
    .await
    .context("listing operations")?)
}

/// Every operation in a portfolio (joined across its assets), for the filterable operations list.
/// Every operation in a portfolio, with its asset's symbol and the currency the amounts are
/// entered in (op price/fee are stored raw in the asset's currency, not converted).
pub async fn list_portfolio_operations(
    pool: &PgPool,
    portfolio_id: Uuid,
) -> anyhow::Result<Vec<(Operation, String, String)>> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, Date, f64, f64, f64, String, OffsetDateTime, String, String)>(
        "SELECT o.id, o.asset_id, o.side, o.op_date, o.quantity, o.price, o.fee, o.note, o.created_at, a.symbol, a.currency \
         FROM portfolio_operations o JOIN portfolio_assets a ON a.id = o.asset_id \
         WHERE a.portfolio_id = $1 ORDER BY o.op_date DESC, o.created_at DESC",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await
    .context("listing portfolio operations")?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                asset_id,
                side,
                op_date,
                quantity,
                price,
                fee,
                note,
                created_at,
                symbol,
                currency,
            )| {
                (
                    Operation {
                        id,
                        asset_id,
                        side,
                        op_date,
                        quantity,
                        price,
                        fee,
                        note,
                        created_at,
                    },
                    symbol,
                    currency,
                )
            },
        )
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub async fn add_operation(
    pool: &PgPool,
    asset_id: Uuid,
    side: &str,
    op_date: Date,
    quantity: f64,
    price: f64,
    fee: f64,
    note: &str,
) -> anyhow::Result<Operation> {
    // `portfolio_id` is denormalized from the asset (an asset never changes portfolio):
    // import de-duplication is per portfolio, and its unique index cannot reach through
    // a join to find one.
    Ok(sqlx::query_as::<_, Operation>(
        "INSERT INTO portfolio_operations (id, asset_id, portfolio_id, side, op_date, quantity, price, fee, note) \
         VALUES ($1,$2,(SELECT portfolio_id FROM portfolio_assets WHERE id = $2),$3,$4,$5,$6,$7,$8) \
         RETURNING id, asset_id, side, op_date, quantity, price, fee, note, created_at",
    )
    .bind(Uuid::new_v4())
    .bind(asset_id)
    .bind(side)
    .bind(op_date)
    .bind(quantity)
    .bind(price)
    .bind(fee)
    .bind(note)
    .fetch_one(pool)
    .await
    .context("adding operation")?)
}

pub async fn delete_operation(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM portfolio_operations WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting operation")?;
    Ok(())
}

// ── Derived position & PnL (weighted-average cost, USD ledger) ─────────────────

/// Chronological operation order for position accounting: business date first,
/// then the real entry order (`created_at`) as the same-day tie-break, then `id`
/// only as a deterministic final tie-break (ids carry no business meaning).
fn sorted_operations(ops: &[Operation]) -> Vec<&Operation> {
    let mut chrono: Vec<&Operation> = ops.iter().collect();
    chrono.sort_by(|a, b| {
        a.op_date
            .cmp(&b.op_date)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    chrono
}

/// Pure long-only accumulation core (unit-testable without a database).
/// `resolve` maps one operation to its (price, fee) pair — USD passes them
/// through; other currencies resolve them at the operation's own date's FX.
///
/// Sell semantics are consistent long-only accounting: a sell realizes against
/// the currently held quantity only (`sold = min(quantity, held)`); the fee
/// scales with the covered portion; a sell with nothing held has NO effect
/// (no short selling, no phantom proceeds/realized).
fn accumulate_position<'a>(
    chrono: impl Iterator<Item = &'a Operation>,
    mut resolve: impl FnMut(&Operation) -> (f64, f64),
) -> (f64, f64, f64) {
    let (mut qty, mut basis, mut realized) = (0.0_f64, 0.0_f64, 0.0_f64);
    for op in chrono {
        let (price, fee) = resolve(op);
        if op.side == "buy" {
            qty += op.quantity;
            basis += op.quantity * price + fee;
        } else {
            // Sell (long-only): realize only against the currently held quantity.
            let avg = if qty > 0.0 { basis / qty } else { 0.0 };
            let sold = op.quantity.min(qty.max(0.0));
            if sold <= 0.0 {
                // Nothing held: a sell has no P&L effect at all (no short selling).
                continue;
            }
            // Proceeds and fee both scale with the COVERED portion, so quantity
            // semantics stay consistent when a sell exceeds the position.
            let proceeds = sold * price - fee * (sold / op.quantity);
            realized += proceeds - sold * avg;
            qty -= sold;
            basis -= sold * avg;
            if qty <= 0.0 {
                qty = 0.0;
                basis = 0.0;
            }
        }
    }
    (qty, basis.max(0.0), realized)
}

/// Walk an asset's operations (chronologically — business date, then entry
/// order) accumulating an average-cost position, in USD.
/// Returns (open_qty, open_cost_basis_usd, realized_pnl_usd). Buy fee is added to basis;
/// sell fee reduces proceeds.
///
/// Operations are entered in the asset's `currency`; each one converts to USD at **its own**
/// date, so a historical buy keeps the rate that applied then instead of being re-valued at
/// today's. A missing rate falls back to the raw amount (same as elsewhere in the module).
pub(crate) async fn position_usd(
    pool: &PgPool,
    ops: &[Operation],
    currency: &str,
) -> anyhow::Result<(f64, f64, f64)> {
    position_usd_on(&mut *pool.acquire().await?, ops, currency).await
}

async fn position_usd_on(
    conn: &mut sqlx::PgConnection,
    ops: &[Operation],
    currency: &str,
) -> anyhow::Result<(f64, f64, f64)> {
    let chrono = sorted_operations(ops);
    let mut fx = journal_fx::FxCache::new();
    // Resolve every operation's (price, fee) first (async, per-date FX), then run
    // the pure accumulator over the resolved pairs.
    let mut resolved: Vec<(f64, f64)> = Vec::with_capacity(chrono.len());
    for op in &chrono {
        let (price, fee) = if currency == "USD" {
            (op.price, op.fee)
        } else {
            (
                fx.convert_on(conn, op.price, currency, "USD", op.op_date)
                    .await?
                    .unwrap_or(op.price),
                fx.convert_on(conn, op.fee, currency, "USD", op.op_date)
                    .await?
                    .unwrap_or(op.fee),
            )
        };
        resolved.push((price, fee));
    }
    let mut i = 0;
    Ok(accumulate_position(chrono.into_iter(), |op| {
        let r = resolved[i];
        i += 1;
        let _ = op;
        r
    }))
}

/// Default pricing identity of an asset: `(provider, provider_id)` — the key the shared
/// `market_quotes` cache is written under. An asset with a `spot_provider` override prices
/// through that source instead, which the shared cache does not cover, so it keeps its own
/// `last_price_usd` (the Portfolio refresh persists it).
fn asset_default_key(asset: &Asset) -> Option<(String, String)> {
    if asset.spot_provider.is_some() {
        None
    } else {
        Some((asset.provider.clone(), asset.provider_id.clone()))
    }
}

/// Pick the current USD price to value an asset with: the shared Market Quote when it is
/// at least as fresh as the asset's own `last_price_usd`/`last_price_at`, otherwise the
/// locally persisted last price. This is what lets one "current market price" flow into
/// Portfolio (and Research) regardless of which module refreshed it last.
pub fn pick_price_usd(
    asset: &Asset,
    market: Option<&market_quotes::MarketQuote>,
) -> Option<(f64, OffsetDateTime)> {
    let local = asset.last_price_usd.map(|p| (p, asset.last_price_at));
    let shared = market
        .filter(|m| m.price_usd > 0.0)
        .map(|m| (m.price_usd, m.quote_at));
    match (local, shared) {
        (None, None) => None,
        (Some((p, at)), None) => Some((p, at.unwrap_or_else(OffsetDateTime::now_utc))),
        (None, Some(s)) => Some(s),
        (Some((lp, la)), Some((sp, sa))) => {
            // Prefer whichever side carries the newer observation time.
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
    }
}

/// Build the computed view for one asset, converting USD figures to `currency` on `date`.
/// `quote_override` is the resolved current price (USD, observation time) for the asset's
/// default identity — `None` keeps the asset's own persisted `last_price_usd`. Passing an
/// owned tuple keeps every future in this module free of borrowed-across-await state.
pub(crate) async fn asset_view(
    pool: &PgPool,
    asset: Asset,
    ops: &[Operation],
    currency: &str,
    date: Date,
    quote_override: Option<(f64, OffsetDateTime)>,
) -> anyhow::Result<AssetView> {
    asset_view_on(
        &mut *pool.acquire().await?,
        asset,
        ops,
        currency,
        date,
        quote_override,
    )
    .await
}

/// Reuses the Portfolio accounting engine inside a paper execution transaction.
pub(crate) async fn asset_view_on(
    conn: &mut sqlx::PgConnection,
    asset: Asset,
    ops: &[Operation],
    currency: &str,
    date: Date,
    quote_override: Option<(f64, OffsetDateTime)>,
) -> anyhow::Result<AssetView> {
    // Surface whichever price source won as the row's own (computed) price fields, so the
    // serialized view carries the fresh USD price and its observation time unchanged.
    let asset = match quote_override {
        Some((p, at)) => {
            let mut a = asset;
            a.last_price_usd = Some(p);
            a.last_price_at = Some(at);
            a
        }
        None => asset,
    };

    let (qty, basis_usd, realized_usd) = position_usd_on(conn, ops, &asset.currency).await?;
    let mut fx = journal_fx::FxCache::new();

    let cost_basis = fx
        .convert_on(conn, basis_usd, "USD", currency, date)
        .await?
        .unwrap_or(basis_usd);
    let realized = fx
        .convert_on(conn, realized_usd, "USD", currency, date)
        .await?
        .unwrap_or(realized_usd);
    let avg_cost = if qty > 0.0 {
        Some(cost_basis / qty)
    } else {
        None
    };

    let (price, market_value, unrealized) = match asset.last_price_usd {
        Some(p) => {
            let price = fx
                .convert_on(conn, p, "USD", currency, date)
                .await?
                .unwrap_or(p);
            let mv = price * qty;
            (Some(price), Some(mv), Some(mv - cost_basis))
        }
        None => (None, None, None),
    };

    Ok(AssetView {
        asset,
        quantity: qty,
        avg_cost,
        cost_basis,
        price,
        market_value,
        unrealized,
        realized,
    })
}

/// Computed views for every asset in a portfolio (in the portfolio's currency, today's FX).
/// Each asset values at the fresher of its shared Market Quote and its own last price.
pub async fn asset_views(pool: &PgPool, pf: &Portfolio) -> anyhow::Result<Vec<AssetView>> {
    let today = OffsetDateTime::now_utc().date();
    let assets = list_assets(pool, pf.id).await?;
    // Load the shared quotes for every default-priced identity in one round-trip, then
    // resolve the winning price per asset up front (pure computation, no awaits) so the
    // per-asset futures below hold owned data only.
    let mut seen = std::collections::HashSet::new();
    let keys: Vec<(String, String)> = assets
        .iter()
        .filter_map(asset_default_key)
        .filter(|k| seen.insert(k.clone()))
        .collect();
    let quotes = market_quotes::get_many(pool, &keys).await?;
    let by_key: std::collections::HashMap<(String, String), market_quotes::MarketQuote> = quotes
        .into_iter()
        .map(|q| ((q.provider.clone(), q.provider_id.clone()), q))
        .collect();
    let overrides: std::collections::HashMap<uuid::Uuid, (f64, OffsetDateTime)> = assets
        .iter()
        .filter_map(|a| {
            let market = asset_default_key(a).and_then(|k| by_key.get(&k));
            pick_price_usd(a, market).map(|(p, at)| (a.id, (p, at)))
        })
        .collect();

    let mut out = Vec::with_capacity(assets.len());
    for a in assets {
        let ops = list_operations(pool, a.id).await?;
        let override_q = overrides.get(&a.id).copied();
        out.push(asset_view(pool, a, &ops, &pf.currency, today, override_q).await?);
    }
    Ok(out)
}

/// One-row totals for the listing page.
pub async fn summary(pool: &PgPool, pf: &Portfolio) -> anyhow::Result<PortfolioSummary> {
    let views = asset_views(pool, pf).await?;
    let mut market_value = 0.0;
    let mut cost_basis = 0.0;
    let mut unrealized = 0.0;
    let mut realized = 0.0;
    for v in &views {
        market_value += v.market_value.unwrap_or(0.0);
        cost_basis += v.cost_basis;
        unrealized += v.unrealized.unwrap_or(0.0);
        realized += v.realized;
    }
    let sparkline = sparkline_values(pool, pf.id).await?;
    Ok(PortfolioSummary {
        portfolio: pf.clone(),
        asset_count: views.len() as i64,
        market_value,
        cost_basis,
        unrealized,
        realized,
        sparkline,
    })
}

/// Recent snapshot market values (oldest→newest), capped for the card sparkline.
async fn sparkline_values(pool: &PgPool, portfolio_id: Uuid) -> anyhow::Result<Vec<f64>> {
    let mut rows: Vec<f64> = sqlx::query_scalar::<_, f64>(
        "SELECT market_value FROM portfolio_snapshots \
         WHERE portfolio_id = $1 ORDER BY snap_date DESC LIMIT 60",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await
    .context("loading sparkline")?;
    rows.reverse();
    Ok(rows)
}

// ── Snapshots ─────────────────────────────────────────────────────────────────

pub async fn list_snapshots(pool: &PgPool, portfolio_id: Uuid) -> anyhow::Result<Vec<Snapshot>> {
    Ok(sqlx::query_as::<_, Snapshot>(
        "SELECT snap_date, currency, market_value, cost_basis FROM portfolio_snapshots \
         WHERE portfolio_id = $1 ORDER BY snap_date",
    )
    .bind(portfolio_id)
    .fetch_all(pool)
    .await
    .context("listing snapshots")?)
}

/// Upsert today's snapshot for a portfolio from its current computed totals.
pub async fn snapshot_today(pool: &PgPool, pf: &Portfolio) -> anyhow::Result<()> {
    let s = summary(pool, pf).await?;
    sqlx::query(
        "INSERT INTO portfolio_snapshots (portfolio_id, snap_date, currency, market_value, cost_basis) \
         VALUES ($1, CURRENT_DATE, $2, $3, $4) \
         ON CONFLICT (portfolio_id, snap_date) DO UPDATE SET \
         currency = EXCLUDED.currency, market_value = EXCLUDED.market_value, cost_basis = EXCLUDED.cost_basis",
    )
    .bind(pf.id)
    .bind(&pf.currency)
    .bind(s.market_value)
    .bind(s.cost_basis)
    .execute(pool)
    .await
    .context("writing snapshot")?;
    Ok(())
}

pub async fn mark_refreshed(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE portfolios SET refreshed_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── CoinGecko coin list cache ──────────────────────────────────────────────────

/// Cached coin list (raw [{id,symbol,name}]) and when it was fetched, or None if never.
pub async fn coingecko_cache(
    pool: &PgPool,
) -> anyhow::Result<Option<(serde_json::Value, OffsetDateTime)>> {
    let row: Option<(serde_json::Value, OffsetDateTime)> =
        sqlx::query_as("SELECT coins, fetched_at FROM portfolio_coingecko_cache WHERE id")
            .fetch_optional(pool)
            .await
            .context("reading coingecko cache")?;
    Ok(row)
}

pub async fn set_coingecko_cache(pool: &PgPool, coins: &serde_json::Value) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO portfolio_coingecko_cache (id, coins, fetched_at) VALUES (TRUE, $1, now()) \
         ON CONFLICT (id) DO UPDATE SET coins = EXCLUDED.coins, fetched_at = now()",
    )
    .bind(coins)
    .execute(pool)
    .await
    .context("writing coingecko cache")?;
    Ok(())
}

#[cfg(test)]
mod position_tests {
    use super::*;
    use time::macros::{date, datetime};

    fn op(
        side: &str,
        d: Date,
        t: OffsetDateTime,
        quantity: f64,
        price: f64,
        fee: f64,
    ) -> Operation {
        Operation {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            side: side.to_string(),
            op_date: d,
            quantity,
            price,
            fee,
            note: String::new(),
            created_at: t,
        }
    }

    /// Identity resolution: amounts are already "USD" (no FX conversion), so the
    /// accumulator runs exactly like the USD branch of position_usd.
    fn accumulate(ops: Vec<Operation>) -> (f64, f64, f64) {
        let chrono = sorted_operations(&ops);
        accumulate_position(chrono.into_iter(), |op| (op.price, op.fee))
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    #[test]
    fn same_day_buy_then_sell_uses_entry_order() {
        // A: BUY 100 @ 56.50 then SELL 50 @ 56.00, same day, BUY entered first.
        let ops = vec![
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 15:33:38 UTC),
                100.0,
                56.50,
                0.0,
            ),
            op(
                "sell",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 15:34:29 UTC),
                50.0,
                56.00,
                0.0,
            ),
        ];
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 50.0);
        near(basis, 2825.0);
        near(basis / qty, 56.50);
        near(realized, -25.0);
    }

    #[test]
    fn cross_day_buy_then_sell() {
        // B: BUY on one day, SELL on a later day — same expected accounting.
        let ops = vec![
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 09:00:00 UTC),
                100.0,
                56.50,
                0.0,
            ),
            op(
                "sell",
                date!(2026 - 09 - 02),
                datetime!(2026-09-02 09:00:00 UTC),
                50.0,
                56.00,
                0.0,
            ),
        ];
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 50.0);
        near(basis, 2825.0);
        near(realized, -25.0);
    }

    #[test]
    fn same_day_full_exit() {
        // C: BUY 100 then SELL 100 same day → flat, zero basis.
        let ops = vec![
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 10:00:00 UTC),
                100.0,
                56.50,
                0.0,
            ),
            op(
                "sell",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 11:00:00 UTC),
                100.0,
                56.00,
                0.0,
            ),
        ];
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 0.0);
        near(basis, 0.0);
        near(realized, -50.0); // round trip at a loss: 100*(56.00-56.50)
    }

    #[test]
    fn same_day_mixed_sequence_follows_entry_order() {
        // D: BUY 100 / SELL 50 / BUY 20, same day, strictly by created_at.
        let ops = vec![
            op(
                "sell",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 15:01:00 UTC),
                50.0,
                12.0,
                0.0,
            ),
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 15:00:00 UTC),
                100.0,
                10.0,
                0.0,
            ),
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 15:02:00 UTC),
                20.0,
                10.0,
                0.0,
            ),
        ];
        // The Vec is deliberately NOT in chronological order — the sort must fix it.
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 70.0);
        near(basis, 700.0);
        near(basis / qty, 10.0);
        near(realized, 100.0); // SELL 50 @12 vs avg 10
    }

    #[test]
    fn oversell_is_consistent_long_only() {
        // E: hold 20, sell 50 → realized only on the COVERED 20 shares; the
        // uncovered remainder is dropped (no short selling, no phantom profit).
        let ops = vec![
            op(
                "buy",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 10:00:00 UTC),
                20.0,
                10.0,
                0.0,
            ),
            op(
                "sell",
                date!(2026 - 09 - 01),
                datetime!(2026-09-01 11:00:00 UTC),
                50.0,
                12.0,
                0.0,
            ),
        ];
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 0.0);
        near(basis, 0.0);
        // 20 covered shares: 20*(12-10)=40. NOT 50*12=600-based fake profit.
        near(realized, 40.0);
    }

    #[test]
    fn sell_with_nothing_held_has_no_effect() {
        // Defensive: SELL into a zero position must not fabricate proceeds.
        let ops = vec![op(
            "sell",
            date!(2026 - 09 - 01),
            datetime!(2026-09-01 11:00:00 UTC),
            50.0,
            56.00,
            0.0,
        )];
        let (qty, basis, realized) = accumulate(ops);
        near(qty, 0.0);
        near(basis, 0.0);
        near(realized, 0.0);
    }
}

#[cfg(test)]
mod quote_pick_tests {
    use super::*;
    use time::macros::datetime;

    fn asset(last: Option<(f64, OffsetDateTime)>, override_p: Option<&str>) -> Asset {
        let (last_price_usd, last_price_at) = match last {
            Some((p, at)) => (Some(p), Some(at)),
            None => (None, None),
        };
        Asset {
            id: Uuid::new_v4(),
            portfolio_id: Uuid::new_v4(),
            asset_class: "stock".into(),
            provider: "yahoo".into(),
            provider_id: "AAPL".into(),
            symbol: "AAPL".into(),
            name: "Apple".into(),
            currency: "USD".into(),
            last_price_usd,
            last_price_at,
            spot_provider: override_p.map(str::to_string),
            spot_symbol: String::new(),
            recon_status: "ok".into(),
            recon_checked_at: None,
            recon_note: String::new(),
        }
    }

    fn quote(price: f64, at: OffsetDateTime) -> market_quotes::MarketQuote {
        market_quotes::MarketQuote {
            provider: "yahoo".into(),
            provider_id: "AAPL".into(),
            price_usd: price,
            currency: "USD".into(),
            quote_at: at,
        }
    }

    #[test]
    fn shared_quote_wins_when_newer() {
        let a = asset(Some((10.0, datetime!(2026-09-01 0:00 UTC))), None);
        let (p, _) =
            pick_price_usd(&a, Some(&quote(12.0, datetime!(2026-09-02 0:00 UTC)))).unwrap();
        assert_eq!(p, 12.0);
    }

    #[test]
    fn stale_shared_quote_keeps_last_price() {
        let a = asset(Some((10.0, datetime!(2026-09-02 0:00 UTC))), None);
        let (p, _) =
            pick_price_usd(&a, Some(&quote(12.0, datetime!(2026-09-01 0:00 UTC)))).unwrap();
        assert_eq!(p, 10.0);
    }

    #[test]
    fn missing_shared_quote_falls_back_to_last_price() {
        let a = asset(Some((10.0, datetime!(2026-09-01 0:00 UTC))), None);
        let (p, _) = pick_price_usd(&a, None).unwrap();
        assert_eq!(p, 10.0);
        assert!(pick_price_usd(&asset(None, None), None).is_none());
    }

    #[test]
    fn spot_override_assets_have_no_default_identity() {
        // An asset priced through an exchange override has no default identity in the
        // shared cache — asset_views must never hand it a default-provider market row
        // (this guard is what keeps the override priced by its own source).
        let a = asset(
            Some((10.0, datetime!(2026-09-02 0:00 UTC))),
            Some("binance"),
        );
        assert_eq!(asset_default_key(&a), None);
        let (p, _) = pick_price_usd(&a, None).unwrap();
        assert_eq!(p, 10.0);
    }
}
