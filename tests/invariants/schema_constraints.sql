-- Database-level invariant enforcement tests.
--
-- Purpose: prove that each invariant PostgreSQL *can* enforce is actually
-- enforced, rather than assumed to be because a constraint was written.
--
-- Method: for each invariant, deliberately attempt the forbidden operation
-- inside a PL/pgSQL block that catches the expected error class, and record
-- whether the database refused. At the end the script raises an exception if any
-- database-enforceable invariant was NOT enforced, so this fails loudly in CI.
--
-- Every forbidden operation is attempted on SYNTHETIC data inside a single
-- transaction that is rolled back at the end. Nothing is left behind.
--
-- Run:  kifaru-repro-schema-check   (or: psql -f tests/invariants/schema_constraints.sql)
-- Requires the schema from migrations/0001_init.sql to be applied.

\set ON_ERROR_STOP on

BEGIN;

CREATE TEMP TABLE _inv_results (
    invariant TEXT PRIMARY KEY,
    enforced  BOOLEAN NOT NULL,
    note      TEXT
) ON COMMIT DROP;

CREATE FUNCTION _record(inv TEXT, square BOOLEAN, detail TEXT DEFAULT NULL)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO _inv_results (invariant, enforced, note)
    VALUES (inv, square, detail)
    ON CONFLICT (invariant) DO UPDATE SET enforced = EXCLUDED.enforced, note = EXCLUDED.note;
END;
$$;

-- Shared fixtures. Two active KES accounts, a third suspended one.
CREATE TEMP TABLE _fx ON COMMIT DROP AS
SELECT
    gen_random_uuid() AS alice,
    gen_random_uuid() AS bob,
    gen_random_uuid() AS suspended,
    gen_random_uuid() AS transfer;

INSERT INTO accounts (id, status, currency, balance_minor, version, created_at)
SELECT alice, 'active', 'KES', 100000, 0, now() FROM _fx
UNION ALL
SELECT bob, 'active', 'KES', 50000, 0, now() FROM _fx
UNION ALL
SELECT suspended, 'suspended', 'KES', 0, 0, now() FROM _fx;

-- ===========================================================================
-- INV-1: an account balance can never become negative.
-- ===========================================================================
DO $$
DECLARE fx RECORD;
BEGIN
    SELECT * INTO fx FROM _fx;
    BEGIN
        INSERT INTO accounts (id, status, currency, balance_minor, version, created_at)
        VALUES (gen_random_uuid(), 'active', 'KES', -1, 0, now());
        PERFORM _record('INV-1', false, 'negative balance was ACCEPTED on insert');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('INV-1', true, 'negative balance rejected at insert');
    END;

    BEGIN
        UPDATE accounts SET balance_minor = -1 WHERE id = fx.alice;
        PERFORM _record('INV-1-update', false, 'negative balance was ACCEPTED on update');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('INV-1-update', true, 'negative balance rejected at update');
    END;
END $$;

-- ===========================================================================
-- INV-2: exactly one debit and one credit per transfer.
-- ===========================================================================
DO $$
DECLARE fx RECORD;
BEGIN
    SELECT * INTO fx FROM _fx;
    INSERT INTO transfers (
        id, source_account_id, destination_account_id, amount_minor,
        currency, status, failure_reason, created_at, updated_at
    ) VALUES (fx.transfer, fx.alice, fx.bob, 1000, 'KES', 'completed', NULL, now(), now());

    BEGIN
        INSERT INTO ledger_entries (id, transfer_id, account_id, direction, amount_minor, currency, created_at)
        VALUES (gen_random_uuid(), fx.transfer, fx.alice, 'debit', 1000, 'KES', now());
        PERFORM _record('INV-2', true, 'first debit accepted');

        -- Second debit for the same transfer must be refused.
        BEGIN
            INSERT INTO ledger_entries (id, transfer_id, account_id, direction, amount_minor, currency, created_at)
            VALUES (gen_random_uuid(), fx.transfer, fx.bob, 'debit', 1000, 'KES', now());
            PERFORM _record('INV-2-duplicate', false, 'a SECOND debit was ACCEPTED for one transfer');
        EXCEPTION WHEN unique_violation THEN
            PERFORM _record('INV-2-duplicate', true, 'second debit rejected');
        END;
    EXCEPTION WHEN others THEN
        PERFORM _record('INV-2', false, 'first debit was unexpectedly rejected: ' || SQLERRM);
    END;
END $$;

-- ===========================================================================
-- INV-5 (shape): a transfer cannot reference its own account, and the amount
-- must be strictly positive. Both are structural preconditions of a transfer
-- that has real ledger entries.
-- ===========================================================================
DO $$
DECLARE fx RECORD;
BEGIN
    SELECT * INTO fx FROM _fx;

    BEGIN
        INSERT INTO transfers (id, source_account_id, destination_account_id, amount_minor,
                               currency, status, failure_reason, created_at, updated_at)
        VALUES (gen_random_uuid(), fx.alice, fx.alice, 100, 'KES', 'pending', NULL, now(), now());
        PERFORM _record('self-transfer', false, 'a transfer to the SAME account was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('self-transfer', true, 'self-transfer rejected');
    END;

    BEGIN
        INSERT INTO transfers (id, source_account_id, destination_account_id, amount_minor,
                               currency, status, failure_reason, created_at, updated_at)
        VALUES (gen_random_uuid(), fx.alice, fx.bob, 0, 'KES', 'pending', NULL, now(), now());
        PERFORM _record('zero-amount', false, 'a ZERO-amount transfer was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('zero-amount', true, 'zero amount rejected');
    END;

    -- A failed transfer must carry a reason; a non-failed one must not.
    BEGIN
        INSERT INTO transfers (id, source_account_id, destination_account_id, amount_minor,
                               currency, status, failure_reason, created_at, updated_at)
        VALUES (gen_random_uuid(), fx.alice, fx.bob, 100, 'KES', 'failed', NULL, now(), now());
        PERFORM _record('failure-reason-required', false, 'FAILED transfer without a reason was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('failure-reason-required', true, 'failed transfer without reason rejected');
    END;

    BEGIN
        INSERT INTO transfers (id, source_account_id, destination_account_id, amount_minor,
                               currency, status, failure_reason, created_at, updated_at)
        VALUES (gen_random_uuid(), fx.alice, fx.bob, 100, 'KES', 'completed', 'stale reason', now(), now());
        PERFORM _record('failure-reason-absent', false, 'COMPLETED transfer with a failure reason was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('failure-reason-absent', true, 'completed transfer with a reason rejected');
    END;

    -- Referential integrity: a transfer cannot reference a non-existent account.
    BEGIN
        INSERT INTO transfers (id, source_account_id, destination_account_id, amount_minor,
                               currency, status, failure_reason, created_at, updated_at)
        VALUES (gen_random_uuid(), fx.alice, gen_random_uuid(), 100, 'KES', 'pending', NULL, now(), now());
        PERFORM _record('fk-account', false, 'a transfer to a NON-EXISTENT account was ACCEPTED');
    EXCEPTION WHEN foreign_key_violation THEN
        PERFORM _record('fk-account', true, 'transfer to unknown account rejected');
    END;
END $$;

-- ===========================================================================
-- INV-7: committed ledger entries are immutable.
-- ===========================================================================
DO $$
BEGIN
    BEGIN
        UPDATE ledger_entries SET amount_minor = 999999;
        PERFORM _record('INV-7-update', false, 'UPDATE on ledger_entries was ALLOWED');
    EXCEPTION WHEN raise_exception THEN
        PERFORM _record('INV-7-update', true, 'UPDATE on ledger_entries blocked by trigger');
    END;

    BEGIN
        DELETE FROM ledger_entries;
        PERFORM _record('INV-7-delete', false, 'DELETE on ledger_entries was ALLOWED');
    EXCEPTION WHEN raise_exception THEN
        PERFORM _record('INV-7-delete', true, 'DELETE on ledger_entries blocked by trigger');
    END;
END $$;

-- ===========================================================================
-- INV-6: idempotency is unique per (client, key) at the database level.
-- ===========================================================================
DO $$
DECLARE fx RECORD;
BEGIN
    SELECT * INTO fx FROM _fx;
    INSERT INTO idempotency_keys (client_id, key, request_hash, created_at)
    VALUES ('client-a', 'key-1', '\x00'::bytea, now());

    BEGIN
        INSERT INTO idempotency_keys (client_id, key, request_hash, created_at)
        VALUES ('client-a', 'key-1', '\x00'::bytea, now());
        PERFORM _record('INV-6', false, 'a DUPLICATE (client_id, key) was ACCEPTED');
    EXCEPTION WHEN unique_violation THEN
        PERFORM _record('INV-6', true, 'duplicate (client_id, key) rejected');
    END;

    -- A different client may reuse the same key string; scoping must work.
    BEGIN
        INSERT INTO idempotency_keys (client_id, key, request_hash, created_at)
        VALUES ('client-b', 'key-1', '\x00'::bytea, now());
        PERFORM _record('INV-6-scoping', true, 'same key allowed for a different client');
    EXCEPTION WHEN others THEN
        PERFORM _record('INV-6-scoping', false, 'key wrongly global, not per-client: ' || SQLERRM);
    END;
END $$;

-- ===========================================================================
-- Outbox: state and its dependent columns cannot disagree.
-- ===========================================================================
DO $$
DECLARE fx RECORD;
BEGIN
    SELECT * INTO fx FROM _fx;

    BEGIN
        INSERT INTO outbox_events (id, aggregate_id, event_type, payload, state, attempts,
                                   next_attempt_at, created_at, delivered_at)
        VALUES (gen_random_uuid(), fx.transfer, 'transfer.completed', '{}'::jsonb, 'delivered',
                0, now(), now(), NULL);
        PERFORM _record('outbox-delivered-at', false, 'DELIVERED event with NULL delivered_at was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('outbox-delivered-at', true, 'delivered event without delivered_at rejected');
    END;

    BEGIN
        INSERT INTO outbox_events (id, aggregate_id, event_type, payload, state, attempts,
                                   next_attempt_at, last_error, created_at)
        VALUES (gen_random_uuid(), fx.transfer, 'transfer.completed', '{}'::jsonb, 'dead',
                3, now(), NULL, now());
        PERFORM _record('outbox-dead-error', false, 'DEAD event with NULL last_error was ACCEPTED');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('outbox-dead-error', true, 'dead event without last_error rejected');
    END;

    -- Migration 0002 relaxed a biconditional to an implication. A PENDING event
    -- may now carry its most recent failure, which is what makes a retrying
    -- event diagnosable instead of merely countable. This was previously
    -- *unrepresentable*, not merely discouraged, and is asserted here so the
    -- relaxation cannot be silently reverted.
    BEGIN
        INSERT INTO outbox_events (id, aggregate_id, event_type, payload, state, attempts,
                                   next_attempt_at, last_error, created_at)
        VALUES (gen_random_uuid(), fx.transfer, 'transfer.completed', '{}'::jsonb, 'pending',
                2, now(), 'broker unavailable', now());
        PERFORM _record('outbox-pending-error', true, 'pending event may record its last failure');
    EXCEPTION WHEN check_violation THEN
        PERFORM _record('outbox-pending-error', false,
            'a PENDING event carrying last_error was REFUSED — migration 0002 appears reverted');
    END;
END $$;

-- ===========================================================================
-- Consumer dedupe: a duplicate (consumer, event_id) is refused, which is what
-- makes at-least-once delivery safe rather than at-least-twice.
-- ===========================================================================
DO $$
DECLARE eid UUID := gen_random_uuid();
BEGIN
    INSERT INTO consumer_inbox (consumer, event_id) VALUES ('simulated', eid);
    BEGIN
        INSERT INTO consumer_inbox (consumer, event_id) VALUES ('simulated', eid);
        PERFORM _record('consumer-dedupe', false, 'a DUPLICATE delivery was ACCEPTED');
    EXCEPTION WHEN unique_violation THEN
        PERFORM _record('consumer-dedupe', true, 'duplicate delivery deduped');
    END;
END $$;

-- ===========================================================================
-- Report
-- ===========================================================================
\echo ''
\echo '--- database-enforced invariant results ---'
SELECT invariant, CASE WHEN enforced THEN 'ENFORCED' ELSE '*** NOT ENFORCED ***' END AS result, note
FROM _inv_results ORDER BY invariant;

DO $$
DECLARE bad TEXT;
BEGIN
    SELECT string_agg(invariant, ', ' ORDER BY invariant) INTO bad
    FROM _inv_results WHERE NOT enforced;
    IF bad IS NOT NULL THEN
        RAISE EXCEPTION 'THE FOLLOWING INVARIANTS ARE NOT ENFORCED BY THE DATABASE: %', bad;
    END IF;
    RAISE NOTICE 'PASS: every database-enforceable invariant was verified (% checks).',
        (SELECT count(*) FROM _inv_results);
END $$;

ROLLBACK;
