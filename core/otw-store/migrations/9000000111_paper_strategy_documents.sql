-- One current Editor strategy document per paper strategy + instrument.
-- The Portfolio asset remains the identity source; Editor owns rich text.
-- Deleting a page detaches the link without deleting the asset or daily history.
CREATE TABLE paper_strategy_documents (
    strategy_id UUID NOT NULL REFERENCES paper_trading_strategies(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES portfolio_assets(id) ON DELETE CASCADE,
    document_id UUID UNIQUE REFERENCES documents(id) ON DELETE SET NULL,
    PRIMARY KEY (strategy_id, asset_id)
);
CREATE INDEX idx_paper_strategy_documents_asset ON paper_strategy_documents(asset_id);
