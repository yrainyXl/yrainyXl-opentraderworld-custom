-- Shared Market Quote cache ("one current market price for an instrument").
--
-- OTW historically kept two disconnected quote caches — `watchlist_items.quote`
-- (JSONB, provider-native price + spark/history, refreshed by the Watchlist loop) and
-- `portfolio_assets.last_price_usd` (USD, refreshed by the Portfolio daily/manual
-- refresh). Portfolio P&L and the Investment Case `/live` aggregation read only the
-- latter, so after a Watchlist refresh moved the price, Portfolio/Research kept showing
-- the old value until the Portfolio side refreshed on its own.
--
-- This table is that missing shared boundary: one row per stable instrument identity
-- `(provider, provider_id)` (e.g. china:601118) holding the *current market price in
-- USD* (converted from the provider-native quote with the journal FX rates when the
-- instrument quotes in CNY/etc.), plus when it was observed (`quote_at`).
--
--   Market Quote (this table, USD)
--        │
--    ┌───┼────┐
--    ↓   ↓    ↓
-- Watchlist  Portfolio  Research(/live)
--
-- Writers (both go through their existing refresh, so no extra provider calls):
--   - the Watchlist refresh writes every successfully quoted item here;
--   - Portfolio refresh / add-asset keep writing `last_price_usd` as today; readers
--     prefer whichever of {this row, last_price_usd} carries the newer `quote_at`.
--
-- Readers (Portfolio asset views, Investment Case /live) prefer this row when it is
-- fresher than the asset's own last_price; the old column stays as fallback/last-known.
-- Entry endpoints run a TTL-bounded "ensure fresh" over the involved identities, so a
-- page opening after an idle gap refreshes the Market Quote itself instead of showing
-- the last persisted price forever.
--
-- No destructive migration: `last_price_usd`/`last_price_at` and the Watchlist quote
-- JSONB are untouched; this table is additive and needs no data backfill.

CREATE TABLE IF NOT EXISTS market_quotes (
    provider    TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    -- Current price in USD (converted from the native quote currency at write time).
    price_usd   DOUBLE PRECISION NOT NULL CHECK (price_usd > 0),
    -- The native quote currency the USD price was converted from (CNY/USD/…).
    currency    TEXT NOT NULL DEFAULT 'USD',
    -- When this price was observed (server clock at the provider fetch).
    quote_at    TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, provider_id)
);
CREATE INDEX IF NOT EXISTS idx_market_quotes_at ON market_quotes(quote_at);
