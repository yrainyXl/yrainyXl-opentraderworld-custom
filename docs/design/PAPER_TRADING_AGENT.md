# Paper Trading Agent — Phase 1/2 Design

Status: implementation target for `feature/paper-trading-agent`.

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

Human/session setup endpoints:

- POST /api/paper-trading/strategies
- POST /api/paper-trading/strategies/{id}/assets

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

## Research isolation

Investment Case position aggregation must exclude any `portfolio_assets` whose `portfolio_id` is bound by `paper_trading_strategies`.

Paper positions stay visible in their Portfolio page and Paper Trading context, but do not affect personal Research live holdings/P&L.

## Phase boundary

Phase 1/2 backend covers strategy isolation, account context, run logging, safe paper execution, idempotency and MCP authorization.

Not included yet:

- real broker routing;
- order book / limit-order lifecycle;
- partial fills;
- multi-user Portfolio ownership migration;
- strategy comparison UI;
- automated performance reports;
- Journal duplication of Portfolio operations.
