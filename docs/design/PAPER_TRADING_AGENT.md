# Paper Trading Agent — Phase 1/2 Design

Status: backend implemented and tested on `feature/paper-trading-agent`; draft PR, no deployment.

## Goal

Let ChatGPT and other external AI agents run isolated paper-trading strategies inside OTW while reusing Portfolio Tracker as the single source of truth for fills, holdings, cost basis and P/L.

This feature is paper-trading only. It must never write to a user's normal/real portfolio through the AI authorization path.

## Invariants

1. Portfolio Tracker remains the only trade/position fact store.
2. Paper Trading owns strategy identity, starting capital, AI run/audit records and the safe execution gateway.
3. A paper strategy owns exactly one Portfolio Tracker portfolio.
4. OAuth AI writes use a dedicated `mcp:paper-trade` scope. They do not receive generic `portfolios:rw`.
5. AI orders never accept a client-supplied fill price. The server uses the shared Market Quote and records the resulting fill into `portfolio_operations`.
6. Buy orders cannot exceed available cash. Sell orders cannot exceed the current long position.
7. Every run and order carries an idempotency key; retries return the existing record instead of duplicating work.
8. Investment Case / Research excludes paper-strategy portfolios by default, so simulated positions cannot pollute the user's personal research holdings.
9. No real broker/exchange order routing is added.

## Data model

### paper_trading_strategies

One strategy = one isolated paper portfolio.

- id
- user_id
- portfolio_id (unique)
- name
- ai_provider
- model
- prompt_version
- currency
- start_date (allows an experiment to be initialized with its actual historical start date)
- initial_cash
- status: active | paused | ended
- max_position_pct
- max_order_value
- slippage_bps
- fee_rate_bps
- min_fee
- created_at / updated_at

The Portfolio row stores the asset ledger. This table stores only paper-account configuration and policy.

### paper_trading_runs

One scheduled AI reasoning cycle, including cycles that place no trade.

- id
- strategy_id
- idempotency_key (unique within strategy)
- scheduled_at
- model
- market_facts
- market_expectations
- inference
- decision: observe | hold | buy | add | reduce | exit | no_trade
- confidence 1..5
- evidence JSON
- risk_assessment
- account_snapshot JSON (server-generated)
- created_at

The snapshot is frozen historical context. It is not a live account balance.

### paper_trading_orders

Audit envelope around the Portfolio operation produced by the paper execution engine.

- id
- strategy_id
- run_id
- idempotency_key (unique within strategy)
- asset_id
- side: buy | sell
- requested_quantity
- quote_price_usd
- quote_at
- operation_id (unique FK to portfolio_operations)
- created_at

The fill price/quantity/fee remain authoritative in `portfolio_operations`; the order row records intent, quote provenance and linkage.

## Account calculation

Available cash is derived, not stored as a second mutable balance:

`initial_cash - buys - buy fees + sells - sell fees`

Every operation is converted from its asset currency to the strategy currency using the operation date's existing Journal FX semantics.

Total equity = available cash + current Portfolio market value.

This keeps cash deterministic from the same Portfolio ledger instead of introducing a second cash balance that can drift.

## REST API

Human/session setup and control endpoints (not in MCP catalog):

- POST /api/paper-trading/strategies
- PATCH /api/paper-trading/strategies/{id} — active | paused | ended; ended is terminal

Agent-readable endpoints:

- GET /api/paper-trading/strategies
- GET /api/paper-trading/strategies/{id}
- GET /api/paper-trading/strategies/{id}/context
- GET /api/paper-trading/strategies/{id}/runs
- GET /api/paper-trading/strategies/{id}/orders

Agent-write endpoints:

- POST /api/paper-trading/strategies/{id}/runs
- POST /api/paper-trading/strategies/{id}/assets
- POST /api/paper-trading/strategies/{id}/orders

The strategy-creation endpoint is deliberately not in the MCP catalog: strategy/account setup remains a human/session action.

## Day-level activity and current thesis (incremental UI/API)

- `GET /api/paper-trading/strategies/{id}/days?limit=10&offset=0`: newest-first **calendar-day** pagination (Asia/Shanghai); run count, fill count and estimated equity change. A run snapshot is captured **before** the order belonging to that run, so `estimated_pnl` must not be called official closing P/L or accurate realized P/L. Null equity must display as unknown.
- `GET /api/paper-trading/strategies/{id}/days/{YYYY-MM-DD}?limit=100&offset=0`: lazy-load the full AI cycle inputs and real paper fills for an expanded day. The default page shows only daily counts/estimated P/L, not repeated verbose reasoning.
- `GET /api/paper-trading/strategies/{id}/documents`: list all strategy assets with linked Editor pages (or `document_id:null`).
- `POST /api/paper-trading/strategies/{id}/documents/{asset_id}`: idempotently ensure an Editor page exists, grouped under **AI 模拟交易策略 / strategy / asset**; useful for pre-existing paper assets and before the first AI run.
- `GET /api/paper-trading/strategies/{id}/documents/{asset_id}`: read the current thesis as rich document JSON and `updated_at`.
- `PUT /api/paper-trading/strategies/{id}/documents/{asset_id}`: send `expected_updated_at` from GET plus exactly one of `markdown` (simple headings/bullets/paragraphs) or `content` (TipTap JSON). An exact repeat is a no-op, while a stale update returns 409. A write is justified only by a meaningful new fact, changed trigger/invalidating condition, actual buy/sell rationale or new outcome/postmortem; not by another routine HOLD_CASH run.

These endpoints are scoped to the existing `mcp:paper-trade` permission and user-owned strategy; they do not grant generic Editor writes. The Editor remains the sole source of note content, Portfolio remains the source of fills, and the immutable runs remain the complete audit. The scheduled ChatGPT prompts are deliberately **not** updated in this code phase.

## MCP / OAuth

OAuth continues to require `mcp:read`.

New optional scope:

`mcp:paper-trade`

When present, the MCP principal receives:

- read access to normal catalog modules;
- read+write only to the new `paper-trading` module.

It still receives no generic Portfolio write permission and no delete permission.

`otw_write` is exposed to OAuth only when the effective permission map contains a writable module. A hand-crafted call is still checked by the static catalog and module permissions.

## Execution flow

1. Agent reads strategy context.
2. Agent gathers external evidence / OTW market data.
3. Agent POSTs one run record, with a deterministic idempotency key.
4. If no trade: stop. The run remains the audit record.
5. If trade: POST an order referencing that run and an asset within the strategy's paper portfolio.
6. Server refreshes shared Market Quote if stale.
7. Server computes a fill price from quote + configured slippage.
8. Server enforces cash, long-only sell quantity, max-order-value and max-position-percent.
9. Server inserts the Portfolio operation and paper order atomically.
10. Retry with the same idempotency key returns the already-created order.

All ledger, position and FX reads during execution use the locked transaction's connection. Portfolio's existing position calculator and Journal FX conversion are reused through connection-taking helpers. This works even with a one-connection pool; waiting orders cannot consume all connections needed by the lock holder.

Existing paper assets cannot change currency or asset class. Order bodies reject unknown fields, including client prices, fees and portfolio/user identifiers. Run bodies reject client account snapshots. An idempotency key reused with a different order quantity, side, run, asset or note, or a different run payload/explicit scheduled time, returns a conflict. Exact retries preserve original fills and snapshots after a strategy ends and do not need a live quote.

China orders use Asia/Shanghai business dates. Until the Market layer supplies trusted settlement metadata, every China instrument conservatively uses T+1; selecting `etf` in the request cannot bypass this. Other providers use UTC business dates. Quotes must be no older than 30 seconds and no more than 5 seconds in the future. Any held instrument without a fresh shared quote blocks new buys; ordinary Portfolio fallback pricing is never an execution quote.

Example session-only strategy setup:

```json
{"name":"AI experiment","currency":"USD","initial_cash":10000,"model":"configured-model","prompt_version":"v1","max_position_pct":0.5,"max_order_value":3000}
```

After setup, the agent reads the strategy context, resolves an instrument using `GET /api/paper-trading/search?q=...&kind=stock`, and adds it using the returned provider identity. A no-trade cycle can be recorded with:

```json
{"idempotency_key":"strategy-cycle-2026-11-02","decision":"no_trade","confidence":3,"market_facts":{},"market_expectations":{},"inference":"Reason for observing","evidence":[],"risk_assessment":{}}
```

For a `buy`/`add` or `reduce`/`exit` run, `POST .../orders` accepts only `idempotency_key`, `run_id`, `asset_id`, `side`, `quantity` and optional `note`. Quantity is asset units, max-order-value is in strategy currency, max-position-percent is a fraction, and fee/slippage rates are basis points. `min_fee` is in the asset currency. Fills are immediate simulated market fills, without exchange session, holiday, lot-size or order-book modeling. This is an experiment ledger, not exchange execution emulation.

Run the repository check first, then targeted tests against a **disposable** PostgreSQL database:

```sh
scripts/cargo-check.sh
# Within the pinned Rust environment, with DATABASE_URL set to the disposable DB:
cargo test -p otw-store --lib --test paper_trading --test market_quotes --test oauth --test price_observations
cargo test -p otw-core --bin otw-core
```

`.github/workflows/paper-trading.yml` provides that environment without using production credentials. The new paper database suite fails when DATABASE_URL is missing; its tests never silently skip. The workflow does not deploy, route broker orders, or write production portfolios.

## Research isolation

Investment Case position aggregation must exclude any `portfolio_assets` whose `portfolio_id` is bound by `paper_trading_strategies`.

Paper positions stay visible in their Portfolio page and Paper Trading context, but do not affect personal Research live holdings/P&L.

## Phase boundary

Phase 1/2 backend covers strategy isolation, account context, run logging, safe paper execution, idempotency and MCP authorization.

Implementation status: the migration, store, REST API, MCP catalog/scope and Research/Portfolio guards are implemented on `feature/paper-trading-agent`. Compilation and backend tests passed in the pinned Rust environment against disposable PostgreSQL. See [the validation report](PAPER_TRADING_VALIDATION.md) for exact files, commits, counts and the final formatting/test workflow. No production migration, deployment or merge was performed. Live provider/ChatGPT scheduled-task smoke testing remains outside this backend PR.

Not included yet:

- real broker routing;
- order book / limit-order lifecycle;
- partial fills;
- multi-user Portfolio ownership migration;
- strategy comparison UI;
- automated performance reports;
- Journal duplication of Portfolio operations.
