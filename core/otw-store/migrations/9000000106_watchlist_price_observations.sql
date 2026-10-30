-- Price observations on watchlist items ("我在某个时间点、这个价格附近主动留下的观察").
--
-- A price observation is a *user* record — a frozen snapshot of "what I saw and noted
-- here" — and stays fully distinct from the system's daily market bars:
--
--   Market bars (quote JSONB `history` / provider fetches)  = market fact, never copied.
--   watchlist_price_observations                            = user observation snapshots.
--
-- `price_snapshot` freezes the price the user saw at record time (the item's current
-- cached quote at that moment). Even if a provider later adjusts its historical bars
-- (adjustments, revisions, intraday-vs-close), the observation keeps the value the user
-- actually saw. `recorded_at` is the moment the observation was made (server time) — it
-- may carry a time-of-day, so multiple observations per calendar day are expected and the
-- UNIQUE is deliberately on the row id, never on a date. `currency` freezes the quote
-- currency of that snapshot (CNY for China Market, USD otherwise) so the stored price can
-- be rendered correctly even if the item's quote source changes later.
--
-- Deleting a watchlist item removes its observations with it (ON DELETE CASCADE); the
-- existing watchlists → watchlist_items cascade makes removing a whole list clean the
-- observations too. Observations never hold system bars: re-reading the current chart
-- always goes back to the Market layer (the item's cached daily history).

CREATE TABLE IF NOT EXISTS watchlist_price_observations (
    id              UUID PRIMARY KEY,
    item_id         UUID NOT NULL REFERENCES watchlist_items(id) ON DELETE CASCADE,
    -- When this observation was made (server clock); carries the time of day.
    recorded_at     TIMESTAMPTZ NOT NULL,
    -- The frozen price the user saw at record time (positive). Never edited afterwards.
    price_snapshot  DOUBLE PRECISION NOT NULL CHECK (price_snapshot > 0),
    -- Quote currency of the snapshot: 'CNY' for China Market, 'USD' otherwise.
    currency        TEXT NOT NULL DEFAULT 'USD',
    -- Optional free-text note. Empty = the user just marked the price point.
    note            TEXT NOT NULL DEFAULT '',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_watchlist_price_observations_item
    ON watchlist_price_observations(item_id, recorded_at DESC);
