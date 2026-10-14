-- Allow retrying outbox events to record WHY the last attempt failed.
--
-- Background
-- ----------
-- 0001 declared:
--
--     CONSTRAINT outbox_dead_has_error
--         CHECK ((state = 'dead') = (last_error IS NOT NULL))
--
-- That is a biconditional, and it is stricter than the comment above it claims.
-- The stated intent was "a dead-lettered event must carry the error that killed
-- it", but the biconditional also forbids `last_error` on a `pending` event.
--
-- With retries implemented, that is the wrong trade. A pending event that has
-- been attempted three times and keeps failing is the single most important
-- thing to diagnose, and under the biconditional it can show you only that it
-- has been attempted three times, never why. The operator has to wait for the
-- event to exhaust its attempts and die before the reason becomes visible.
--
-- Change
-- ------
-- Relaxed to a one-directional implication:
--
--     CHECK (state <> 'dead' OR last_error IS NOT NULL)
--
-- The guarantee that actually matters is preserved exactly: a dead event can
-- never exist without the error that killed it. What is newly permitted is a
-- pending event carrying its most recent failure for diagnostics. That state was
-- previously *unrepresentable*, not merely discouraged.
--
-- This touches no INV-n. It is operational diagnosability of the retry path,
-- not a financial invariant.
--
-- No explicit BEGIN/COMMIT: sqlx manages the transaction.

ALTER TABLE outbox_events DROP CONSTRAINT outbox_dead_has_error;

ALTER TABLE outbox_events ADD CONSTRAINT outbox_dead_has_error
    CHECK (state <> 'dead' OR last_error IS NOT NULL);

COMMENT ON CONSTRAINT outbox_dead_has_error ON outbox_events IS
    'A dead event must carry the error that killed it. A pending event MAY carry '
    'its most recent failure for diagnostics (relaxed from a biconditional in '
    'migration 0002 because the retrying state was otherwise undiagnosable).';