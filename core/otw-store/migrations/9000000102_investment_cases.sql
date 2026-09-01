-- Investment Case MVP (Phase 1): per-user research entity for one instrument,
-- linked to exactly one Editor document. Positions/PnL are NOT stored here —
-- live data is aggregated from the Portfolio Tracker (single source of truth);
-- decision_records freeze a server-computed snapshot at creation time.
--
-- Ownership: investment_cases.user_id follows the sessions FK convention
-- (users.id ON DELETE CASCADE). OTW is currently single-user; this user_id is
-- the case's own ownership anchor and the seam for a future multi-user
-- migration (it does not make the whole app multi-tenant on its own).

CREATE TABLE investment_cases (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider     text NOT NULL,
    provider_id  text NOT NULL,
    symbol       text NOT NULL DEFAULT '',
    name         text NOT NULL DEFAULT '',
    exchange     text NOT NULL DEFAULT '',
    currency     text NOT NULL DEFAULT 'USD',
    asset_class  text NOT NULL DEFAULT 'equity',
    document_id  uuid NOT NULL REFERENCES documents(id) ON DELETE RESTRICT,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT investment_cases_user_provider_provider_id_key
        UNIQUE (user_id, provider, provider_id)
);

CREATE TABLE decision_records (
    id                      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id      uuid NOT NULL REFERENCES investment_cases(id) ON DELETE CASCADE,
    decision_type           text NOT NULL,
    confidence              smallint NOT NULL DEFAULT 3,
    reason                  text NOT NULL DEFAULT '',
    evidence                text NOT NULL DEFAULT '',
    counter_evidence        text NOT NULL DEFAULT '',
    invalidation            text NOT NULL DEFAULT '',
    watch_items             text NOT NULL DEFAULT '',
    recorded_at             timestamptz NOT NULL DEFAULT now(),
    snapshot_provider       text NOT NULL,
    snapshot_provider_id    text NOT NULL,
    snapshot_symbol         text NOT NULL,
    snapshot_name           text NOT NULL,
    snapshot_exchange       text NOT NULL DEFAULT '',
    snapshot_currency       text NOT NULL,
    snapshot_price          double precision,
    snapshot_price_at       timestamptz,
    snapshot_quantity       double precision NOT NULL DEFAULT 0,
    snapshot_avg_cost       double precision,
    snapshot_cost_basis     double precision,
    snapshot_market_value   double precision,
    snapshot_unrealized_pnl double precision,
    snapshot_unrealized_pct double precision,
    CONSTRAINT decision_records_decision_type_check
        CHECK (decision_type IN ('observe', 'hold', 'buy', 'add', 'reduce', 'exit')),
    CONSTRAINT decision_records_confidence_check
        CHECK (confidence BETWEEN 1 AND 5)
);

CREATE INDEX idx_decision_records_case
    ON decision_records (investment_case_id, recorded_at);
