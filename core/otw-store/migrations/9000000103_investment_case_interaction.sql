-- Phase 1.5: Investment Case three-layer interaction model.
--
-- 1) Investment Thesis (low frequency) — lazy 1:1 row per case.
-- 2) thesis_assumptions — stable-id rows (1..3 per case) so Change Records can
--    reference exactly which assumption a change affected.
-- 3) Current State on investment_cases:
--      - current_action / current_confidence are NOT freely editable: they are
--        the projection of the LATEST Change Record and only change inside the
--        same transaction that creates one (see record_change in the store).
--      - valuation_view / current_biggest_question are standalone-autosaved.
-- 4) decision_records gains the Change-Log fields. The existing `confidence`
--    column keeps its name and is RE-DEFINED as confidence-after (no rename,
--    no data migration). Legacy text fields (reason/evidence/...) stay as-is
--    for historical rows; the new quick UI does not collect them.
--
-- All additive; 9000000102 is untouched.

CREATE TABLE investment_theses (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id  uuid NOT NULL UNIQUE REFERENCES investment_cases(id) ON DELETE CASCADE,
    core_thesis         text NOT NULL DEFAULT '',
    invalidation        text NOT NULL DEFAULT '',
    major_risks         text NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE thesis_assumptions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id  uuid NOT NULL REFERENCES investment_cases(id) ON DELETE CASCADE,
    position            smallint NOT NULL,
    text                text NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT thesis_assumptions_position_check
        CHECK (position BETWEEN 1 AND 3),
    CONSTRAINT thesis_assumptions_case_position_key
        UNIQUE (investment_case_id, position)
);

ALTER TABLE investment_cases
    ADD COLUMN current_action text,
    ADD COLUMN current_confidence smallint,
    ADD COLUMN valuation_view text,
    ADD COLUMN current_biggest_question text NOT NULL DEFAULT '',
    ADD CONSTRAINT investment_cases_current_action_check
        CHECK (current_action IN ('observe', 'hold', 'buy', 'add', 'reduce', 'exit')),
    ADD CONSTRAINT investment_cases_current_confidence_check
        CHECK (current_confidence BETWEEN 1 AND 5),
    ADD CONSTRAINT investment_cases_valuation_view_check
        CHECK (valuation_view IN ('cheap', 'fair', 'expensive', 'unclear'));

ALTER TABLE decision_records
    ADD COLUMN confidence_before smallint,
    ADD COLUMN what_happened text NOT NULL DEFAULT '',
    ADD COLUMN impact_direction text,
    ADD COLUMN affected_assumption_id uuid REFERENCES thesis_assumptions(id) ON DELETE SET NULL,
    ADD COLUMN affected_assumption_text text NOT NULL DEFAULT '',
    ADD CONSTRAINT decision_records_confidence_before_check
        CHECK (confidence_before BETWEEN 1 AND 5),
    ADD CONSTRAINT decision_records_impact_direction_check
        CHECK (impact_direction IN ('strengthen', 'weaken', 'neutral'));
