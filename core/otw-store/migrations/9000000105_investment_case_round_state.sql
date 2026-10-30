-- Phase 1.6: every decision round carries its own valuation judgement and its
-- biggest open question.
--
-- Before this change the case had a separate, standalone-autosaved "current
-- state" (investment_cases.valuation_view / current_biggest_question) that
-- lived independently of the decision timeline. The detail page moves those two
-- inputs INTO the "record this round" flow, because a round's judgement and
-- question are part of that round's history: later rounds must never overwrite
-- them. The case-level columns remain as a DERIVED projection of the LATEST
-- decision record (kept in sync by record_change), so the Research list and the
-- case payload keep working unchanged.
--
-- All additive; 9000000102/0103/0104 are untouched.

ALTER TABLE decision_records
    ADD COLUMN valuation_view text,
    ADD COLUMN biggest_question text NOT NULL DEFAULT '',
    ADD CONSTRAINT decision_records_valuation_view_check
        CHECK (valuation_view IN ('cheap', 'fair', 'expensive', 'unclear'));

-- One-time fold: copy each case's standalone current state into its LATEST
-- decision record (when a record exists), so a case that already had a current
-- judgement does not lose it when the UI starts deriving everything from the
-- records. Older (non-latest) records stay untouched — they never carried a
-- per-round state under the old model.
UPDATE decision_records dr
SET valuation_view = COALESCE(dr.valuation_view, ic.valuation_view),
    biggest_question = CASE
        WHEN dr.biggest_question = '' THEN ic.current_biggest_question
        ELSE dr.biggest_question
    END
FROM investment_cases ic
WHERE ic.id = dr.investment_case_id
  AND dr.id = (
      SELECT dr2.id FROM decision_records dr2
      WHERE dr2.investment_case_id = ic.id
      ORDER BY dr2.recorded_at DESC, dr2.id DESC
      LIMIT 1
  );
