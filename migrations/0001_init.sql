-- KIFARU initial schema.
--
-- Design rules applied throughout:
--   * Money is stored as BIGINT MINOR UNITS. Never NUMERIC for storage, never a
--     float. Reporting may cast to NUMERIC read-only (see INV-8).
--   * Financial invariants are enforced by CONSTRAINTS wherever PostgreSQL can
--     express them. Application checks are a second line of defence, never the
--     only one — two concurrent requests can both pass an application check.
--   * Timestamps are TIMESTAMPTZ. There is no naive time in this schema.
--
-- Invariant coverage is called out per constraint as INV-n. The authoritative
-- definitions live in docs/invariants.md.

BEGIN;

-- ---------------------------------------------------------------------------
-- accounts
-- ---------------------------------------------------------------------------
CREATE TABLE accounts (
    id            UUID        PRIMARY KEY,
    status        TEXT        NOT NULL,
    currency      TEXT        NOT NULL,
    -- Authoritative balance for reads. Kept in lockstep with ledger_entries in
    -- the same transaction; reconciliation (INV-4) exists to prove they agree.
    balance_minor BIGINT      NOT NULL,
    -- Optimistic-concurrency metadata. The transfer path does not depend on it
    -- for correctness (row locks do that); it makes the write count observable.
    version       BIGINT      NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- INV-1: the default overdraft policy prohibits negative balances. This is
    -- the check that makes an overdraw impossible even if application logic is
    -- wrong, because the INSERT/UPDATE simply fails.
    CONSTRAINT accounts_balance_non_negative CHECK (balance_minor >= 0),
    CONSTRAINT accounts_version_non_negative CHECK (version >= 0),
    CONSTRAINT accounts_status_valid
        CHECK (status IN ('active', 'suspended', 'closed')),
    -- Single configured currency (Phase 1). Loosened by a migration, not by
    -- removing the constraint.
    CONSTRAINT accounts_currency_valid CHECK (currency = 'KES')
);

-- ---------------------------------------------------------------------------
-- transfers
-- ---------------------------------------------------------------------------
CREATE TABLE transfers (
    id                     UUID        PRIMARY KEY,
    source_account_id      UUID        NOT NULL REFERENCES accounts (id),
    destination_account_id UUID        NOT NULL REFERENCES accounts (id),
    amount_minor           BIGINT      NOT NULL,
    currency               TEXT        NOT NULL,
    status                 TEXT        NOT NULL,
    failure_reason         TEXT,
    created_at             TIMESTAMPTZ NOT NULL,
    updated_at             TIMESTAMPTZ NOT NULL,

    CONSTRAINT transfers_amount_positive CHECK (amount_minor > 0),
    -- A self-transfer would post a debit and a credit against one account,
    -- which the ledger treats as malformed.
    CONSTRAINT transfers_distinct_accounts
        CHECK (source_account_id <> destination_account_id),
    CONSTRAINT transfers_status_valid
        CHECK (status IN ('pending', 'completed', 'failed')),
    CONSTRAINT transfers_currency_valid CHECK (currency = 'KES'),
    -- A failure reason exists if and only if the transfer failed. Makes it
    -- impossible to record a failure without a diagnosable cause, or to attach
    -- a stale reason to a successful transfer.
    CONSTRAINT transfers_failure_reason_matches_status
        CHECK ((status = 'failed') = (failure_reason IS NOT NULL))
);

CREATE INDEX transfers_source_account_idx
    ON transfers (source_account_id, created_at DESC);
CREATE INDEX transfers_destination_account_idx
    ON transfers (destination_account_id, created_at DESC);
CREATE INDEX transfers_created_at_idx ON transfers (created_at DESC);

-- ---------------------------------------------------------------------------
-- ledger_entries  (APPEND-ONLY — INV-7)
-- ---------------------------------------------------------------------------
CREATE TABLE ledger_entries (
    id           UUID        PRIMARY KEY,
    transfer_id  UUID        NOT NULL REFERENCES transfers (id),
    account_id   UUID        NOT NULL REFERENCES accounts (id),
    direction    TEXT        NOT NULL,
    -- Always positive; `direction` carries the sign. Equal magnitude on both
    -- legs is what makes a transaction balance.
    amount_minor BIGINT      NOT NULL,
    currency     TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,

    CONSTRAINT ledger_amount_positive CHECK (amount_minor > 0),
    CONSTRAINT ledger_direction_valid CHECK (direction IN ('debit', 'credit')),
    CONSTRAINT ledger_currency_valid CHECK (currency = 'KES')
);

-- INV-2: at most one debit and at most one credit per transfer. Combined with
-- the application always inserting both legs, a transfer can have exactly two
-- entries and no more. A bug that tried to double-post fails here.
CREATE UNIQUE INDEX ledger_entries_one_per_direction
    ON ledger_entries (transfer_id, direction);

-- Supports balance recomputation per account, which reconciliation runs.
CREATE INDEX ledger_entries_account_idx
    ON ledger_entries (account_id, created_at);

-- INV-7: committed entries are immutable. Corrections are posted as
-- compensating transactions, never by rewriting history.
--
-- This trigger blocks ordinary UPDATE/DELETE. The stronger control is revoking
-- UPDATE/DELETE from the application role; that is a deployment concern and is
-- documented in docs/operations.md. Both are intended to be in force, not one
-- or the other.
CREATE FUNCTION reject_ledger_mutation() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION
        'ledger_entries is append-only (INV-7): corrections must be posted as '
        'compensating entries, not by % on %',
        TG_OP, TG_TABLE_NAME;
END;
$$;

CREATE TRIGGER ledger_entries_immutable
    BEFORE UPDATE OR DELETE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION reject_ledger_mutation();

-- ---------------------------------------------------------------------------
-- idempotency_keys
-- ---------------------------------------------------------------------------
-- INV-6: duplicate requests must never produce duplicate financial effects.
--
-- The PRIMARY KEY is the enforcement point. Uniqueness lives in the database,
-- not in application code, because two concurrent requests can both pass an
-- application-level "does this key exist?" check and then both proceed.
CREATE TABLE idempotency_keys (
    client_id       TEXT        NOT NULL,
    key             TEXT        NOT NULL,
    -- Hash of the canonical request payload. Detects the same key reused with a
    -- different body, which must be rejected rather than silently executed.
    request_hash    BYTEA       NOT NULL,
    -- Set once the original request committed.
    transfer_id     UUID        REFERENCES transfers (id),
    response_status SMALLINT,
    response_body   JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Retained until no retry can safely arrive. Deletion policy is documented
    -- in docs/operations.md; nothing prunes this on a schedule yet.
    completed_at    TIMESTAMPTZ,

    PRIMARY KEY (client_id, key)
);

CREATE INDEX idempotency_keys_created_at_idx
    ON idempotency_keys (created_at);

-- ---------------------------------------------------------------------------
-- outbox_events
-- ---------------------------------------------------------------------------
-- Durable publication intent. Committed in the SAME transaction as the
-- transfer, so an event cannot exist without its transfer and a committed
-- transfer cannot exist without its event.
--
-- Delivery guarantee is AT-LEAST-ONCE. Nothing here provides exactly-once.
CREATE TABLE outbox_events (
    id              UUID        PRIMARY KEY,
    aggregate_id    UUID        NOT NULL,
    event_type      TEXT        NOT NULL,
    payload         JSONB       NOT NULL,
    state           TEXT        NOT NULL DEFAULT 'pending',
    attempts        INTEGER     NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Lease: a worker sets these when claiming. If the worker dies, the lease
    -- expires and the event becomes claimable again. This is what makes
    -- recovery after a crash work without a coordinator.
    locked_until    TIMESTAMPTZ,
    locked_by       TEXT,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at    TIMESTAMPTZ,

    CONSTRAINT outbox_state_valid
        CHECK (state IN ('pending', 'delivered', 'dead')),
    CONSTRAINT outbox_attempts_non_negative CHECK (attempts >= 0),
    -- A delivered event must say when; a non-delivered one must not claim to.
    CONSTRAINT outbox_delivered_at_matches_state
        CHECK ((state = 'delivered') = (delivered_at IS NOT NULL)),
    -- A dead-lettered event must carry the error that killed it.
    CONSTRAINT outbox_dead_has_error
        CHECK ((state = 'dead') = (last_error IS NOT NULL))
);

-- The claim query's access path: oldest claimable pending events first.
CREATE INDEX outbox_claimable_idx
    ON outbox_events (next_attempt_at, created_at)
    WHERE state = 'pending';

-- Operational metric: pending backlog and dead-letter count.
CREATE INDEX outbox_state_idx ON outbox_events (state);

-- ---------------------------------------------------------------------------
-- consumer_inbox  (simulated consumer's dedupe table)
-- ---------------------------------------------------------------------------
-- Demonstrates idempotent consumer processing: the consumer records the event
-- id it has handled, and a duplicate delivery is detected here rather than
-- causing a second side effect. This is the consumer-side half of the
-- at-least-once contract. Without it, at-least-once becomes at-least-twice.
CREATE TABLE consumer_inbox (
    consumer     TEXT        NOT NULL,
    event_id     UUID        NOT NULL,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (consumer, event_id)
);

COMMIT;
