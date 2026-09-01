-- Portfolio Tracker: China Market provider (cn-market service) for A-share assets.
--
-- Mirrors 0100 (watchlist_items) for portfolio_assets: A-shares are stored as
-- provider='china' with the bare 6-digit code as provider_id (e.g. 601118), quoted
-- in CNY and converted to USD via the journal FX rates on read/refresh.

-- 1. portfolio_assets.provider: allow 'china'.
ALTER TABLE portfolio_assets DROP CONSTRAINT IF EXISTS portfolio_assets_provider_check;
ALTER TABLE portfolio_assets ADD CONSTRAINT portfolio_assets_provider_check
    CHECK (provider IN ('coingecko', 'yahoo', 'china'));

-- 2. spot_provider override: allow pinning 'china' as the live price source.
ALTER TABLE portfolio_assets DROP CONSTRAINT IF EXISTS portfolio_assets_spot_provider_check;
ALTER TABLE portfolio_assets ADD CONSTRAINT portfolio_assets_spot_provider_check
    CHECK (spot_provider IS NULL OR
           spot_provider IN ('coingecko', 'yahoo', 'china', 'binance', 'kraken', 'coinbase'));
