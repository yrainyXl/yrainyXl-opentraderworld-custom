-- China Market provider (cn-market service): A-share watchlist items and the
-- seeded "China Market" connector that quotes them.

-- 1. watchlist_items.provider: allow 'china' (bare 6-digit A-share code, e.g. 601118).
ALTER TABLE watchlist_items DROP CONSTRAINT IF EXISTS watchlist_items_provider_check;
ALTER TABLE watchlist_items ADD CONSTRAINT watchlist_items_provider_check
    CHECK (provider IN ('coingecko', 'yahoo', 'china'));

-- 2. Seed the keyless "China Market" connector (same pattern as 0047's seeded defaults),
-- granted to every data module so watchlists and the histdata downloads can use it.
INSERT INTO histdata_connectors (id, provider, name)
VALUES (gen_random_uuid(), 'china', 'China Market')
ON CONFLICT (name) DO NOTHING;

INSERT INTO connector_modules (connector_id, module)
SELECT id, '*' FROM histdata_connectors WHERE provider = 'china'
ON CONFLICT DO NOTHING;
