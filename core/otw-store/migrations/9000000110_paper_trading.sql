-- Isolated AI paper-trading accounts.
--
-- Portfolio Tracker remains the single source of truth for all fills and positions.
-- These tables add strategy identity, AI decision audit records and a controlled
-- execution envelope around portfolio_operations. No broker/exchange routing exists.

CREATE TABLE IF NOT EXISTS paper_trading_strategies (
    id                         UUID PRIMARY KEY,
    user_id                    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    portfolio_id               UUID NOT NULL UNIQUE REFERENCES portfolios(id) ON DELETE RESTRICT,
    name                       TEXT NOT NULL,
    ai_provider                TEXT NOT NULL DEFAULT 'chatgpt',
    model                      TEXT NOT NULL DEFAULT '',
    prompt_version             TEXT NOT NULL DEFAULT '',
    currency                   TEXT NOT NULL DEFAULT 'CNY',
    initial_cash               DOUBLE PRECISION NOT NULL CHECK (initial_cash > 0),
    status                     TEXT NOT NULL DEFAULT 'active'
                               CHECK (status IN ('active', 'paused', 'ended')),
    -- Fraction in (0, 1], e.g. 0.30 = maximum 30% of account equity in one asset.
    max_position_pct           DOUBLE PRECISION NOT NULL DEFAULT 0.50
                               CHECK (max_position_pct > 0 AND max_position_pct <= 1),
    max_order_value            DOUBLE PRECISION NOT NULL DEFAULT 0
                               CHECK (max_order_value >= 0),
    slippage_bps               DOUBLE PRECISION NOT NULL DEFAULT 5
                               CHECK (slippage_bps >= 0 AND slippage_bps <= 1000),
    fee_rate_bps               DOUBLE PRECISION NOT NULL DEFAULT 3
                               CHECK (fee_rate_bps >= 0 AND fee_rate_bps <= 1000),
    sell_extra_fee_rate_bps    DOUBLE PRECISION NOT NULL DEFAULT 5
                               CHECK (sell_extra_fee_rate_bps >= 0 AND sell_extra_fee_rate_bps <= 1000),
    min_fee                    DOUBLE PRECISION NOT NULL DEFAULT 5
                               CHECK (min_fee >= 0),
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_paper_strategies_user
    ON paper_trading_strategies(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_paper_strategies_portfolio
    ON paper_trading_strategies(portfolio_id);

CREATE TABLE IF NOT EXISTS paper_trading_runs (
    id                  UUID PRIMARY KEY,
    strategy_id         UUID NOT NULL REFERENCES paper_trading_strategies(id) ON DELETE CASCADE,
    idempotency_key     TEXT NOT NULL,
    scheduled_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    model               TEXT NOT NULL DEFAULT '',
    market_facts        JSONB NOT NULL DEFAULT '{}'::jsonb,
    market_expectations JSONB NOT NULL DEFAULT '{}'::jsonb,
    inference           TEXT NOT NULL DEFAULT '',
    decision            TEXT NOT NULL
                        CHECK (decision IN ('observe', 'hold', 'buy', 'add', 'reduce', 'exit', 'no_trade')),
    confidence          SMALLINT NOT NULL CHECK (confidence BETWEEN 1 AND 5),
    evidence            JSONB NOT NULL DEFAULT '[]'::jsonb,
    risk_assessment     JSONB NOT NULL DEFAULT '{}'::jsonb,
    account_snapshot    JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (strategy_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_paper_runs_strategy_at
    ON paper_trading_runs(strategy_id, scheduled_at DESC);

CREATE TABLE IF NOT EXISTS paper_trading_orders (
    id                 UUID PRIMARY KEY,
    strategy_id        UUID NOT NULL REFERENCES paper_trading_strategies(id) ON DELETE RESTRICT,
    run_id             UUID NOT NULL REFERENCES paper_trading_runs(id) ON DELETE RESTRICT,
    asset_id           UUID NOT NULL REFERENCES portfolio_assets(id) ON DELETE RESTRICT,
    operation_id       UUID NOT NULL UNIQUE REFERENCES portfolio_operations(id) ON DELETE RESTRICT,
    idempotency_key    TEXT NOT NULL,
    side               TEXT NOT NULL CHECK (side IN ('buy', 'sell')),
    requested_quantity DOUBLE PRECISION NOT NULL CHECK (requested_quantity > 0),
    quote_price_usd    DOUBLE PRECISION NOT NULL CHECK (quote_price_usd > 0),
    quote_at           TIMESTAMPTZ NOT NULL,
    slippage_bps       DOUBLE PRECISION NOT NULL CHECK (slippage_bps >= 0),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (strategy_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_paper_orders_strategy_at
    ON paper_trading_orders(strategy_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_paper_orders_run
    ON paper_trading_orders(run_id);

-- Existing OAuth clients may request the new scope at their next authorize flow.
-- This expands the client's allowed-scope ceiling only; it does NOT grant the scope.
-- The resource owner must still request it and approve it on the consent page.
UPDATE oauth_clients
SET scopes = array_append(scopes, 'mcp:paper-trade')
WHERE 'mcp:read' = ANY(scopes)
  AND NOT ('mcp:paper-trade' = ANY(scopes));
