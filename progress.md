# KIFARU Progress

_Last updated: 2026-10-13_

**State: Phase 0 (foundation) CLOSED on the design side — architecture, invariants, crate layout and all decisions locked. ZERO lines of code written, ZERO commits in git. Build work has not started and is blocked on the Rust toolchain + Docker daemon, both owned by the user.**

## Current state

**Phase 0 (foundation) is closed on the design side.** KIFARU is a production-style
transaction-processing and reliability platform written in Rust that *simulates* mobile-money
transfers — a simulation only, not connected to any real payment network, moving no real
money, with no real counterparties. Everything Phase 0 was supposed to decide has been
decided: money representation, crate layout, isolation and locking strategy, idempotency,
outbox, reconciliation, retry policy, the test runner, Postgres version pinning, the
`client_id` story, and invariants INV-1..INV-8. The repo contains four artifacts and
nothing else: `README.md` (complete), `.gitignore` (complete), `progress.md` (this file), and
`skills-lock.json` (10 Rust/Postgres/reliability skills, pinned by SHA-256 hash). Git is
initialized on branch `main` with **zero commits**. What does not exist is any Rust code:
no `Cargo.toml`, no source tree, no schema, no migration, no test, no measurement. Every
compile, test, benchmark and reconciliation claim in this repo is **planned**, never
**verified**. Build work has not begun and is **blocked on two things the user owns**:
installing the Rust toolchain (`rustc`/`cargo`/`rustup` are absent) and starting the Docker
Desktop daemon (Docker CLI + Compose v5.3.1 are installed, but the daemon is not running, so
no Postgres container can start). Git identity is configured, so committing is not itself
blocked. **Next single action: the user confirms the Rust toolchain is installed, then the
Docker daemon is up; `Next` items 1–3 run and the build sequence starts.**

## Blockers

Status vocabulary used throughout this file: **verified** = observed/confirmed by running or
by inspection this session; **planned** = decided, not yet built; **blocked** = cannot proceed
until an external prerequisite clears.

- [x] ~~**BLOCKED (owner: user)** — Rust toolchain not installed.~~ **CLEARED 2026-10-13.
  `verified`** — Toolchain installed at `%USERPROFILE%\.cargo\bin` but NOT on the agent
  shell's PATH; commands must be run with that bin directory prepended. Observed:
  `rustc 1.99.0 (b940084d7 2026-09-28)`, `cargo 1.99.0 (5f94df478 2026-08-27)`,
  `rustup 1.29.1 (d95a37b6a 2026-08-13)`, toolchain
  `stable-x86_64-pc-windows-msvc (active, default)`. End-to-end verified by compiling,
  linking and executing a scratch binary: `cargo build` -> `Finished dev profile in 2.05s`,
  binary ran and exited 0. MSVC linker is present and working.
- [x] ~~**BLOCKED (owner: user)** — Docker daemon not running.~~ **CLEARED 2026-10-13.
  `verified`** — `docker version` reports `client=29.6.2 server=29.6.2`, so the daemon is
  live and reachable. Note: Compose is v5.3.1 while the engine reports 29.6.2; no Postgres
  container has been started yet, so image pull and container start are still unproven.
- [ ] Not a blocker, accepted: `psql` is not installed. Docker-hosted Postgres covers this
  for Phase 0. Revisit only if we ever need a direct client outside containers.
- [ ] Deferred by agreement, not a blocker: `k6` / `vegeta` load generators are absent.
  Load testing is a Phase 4 concern and does not gate Phase 0.

## Done

Each item is labeled with its evidence level. Nothing here implies working code.

- [x] `verified` — Project directory `D:\projects\kifaru` inspected and confirmed to have
  been completely empty at session start.
- [x] `verified` — `git init -b main` executed. Repo exists on branch `main`. **Zero commits**
  (`fatal: your current branch 'main' does not have any commits yet`).
- [x] `verified` — Architecture decisions agreed with the user (see
  "Open questions / decisions locked"). Design only; nothing implemented.
- [x] `verified` — Invariants INV-1..INV-7 defined in prose and agreed, plus **INV-8 proposed
  and agreed in principle**. Formalization of all eight into `docs/invariants.md` is still
  `planned`; none is enforced by code, because there is no code.
- [x] `verified` — **Phase 0 design decisions closed.** Money representation, crate layout,
  binary shape, isolation, idempotency, outbox, reconciliation, retry policy, test runner,
  Postgres pinning, and the `client_id` / testkit-override story are all locked. Phase 0 is
  closed *on the design side only* — no implementation work was in its scope and none was
  done.
- [x] `verified` — Host environment inspected: Windows 11 (build 26200), 16 logical cores,
  31.4 GB RAM. Re-runnable probes, for traceability:
  `(Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors` → `16`;
  `[math]::Round((Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize/1MB, 1)`
  → `31.4` GB.
- [x] `verified` — Toolchain probe run: `rustc`/`cargo`/`rustup` absent; Docker CLI +
  Compose v5.3.1 present but daemon dead; `psql` absent; `k6`/`vegeta` absent.
- [x] `verified` — Ten project-scope skills installed under `.agents/skills/` (copied, not
  symlinked) and recorded in `skills-lock.json`: `rust-best-practices`,
  `rust-async-patterns`, `rust-testing`, `test-driven-development`,
  `verification-before-completion`, `postgresql-table-design`,
  `supabase-postgres-best-practices`, `chaos-engineer`, `observability-guidelines`,
  `domain-modeling`. Note: the user already has a global `code-review` skill, so it was
  deliberately not duplicated here.
- [x] `verified` — This file (`progress.md`) created and adopted as the single-writer
  progress record.
- [x] `verified` — Working tree still fully uncommitted. Current `git status --short`
  output: `?? .agents/`, `?? .gitignore`, `?? README.md`, `?? progress.md`,
  `?? skills-lock.json`. `git log` still reports
  `fatal: your current branch 'main' does not have any commits yet`.
- [x] `verified` — `README.md` created by the main agent. Every feature claim carries an
  explicit `[IMPLEMENTED]` / `[PARTIAL]` / `[PLANNED]` / `[UNSUPPORTED]` label; nothing is
  labeled implemented. Quick start is explicitly marked not-yet-runnable. No benchmark or
  test results are claimed anywhere.
- [x] `verified` — `.gitignore` created (target/, .env but not .env.example, Postgres data
  volumes, chaos/benchmark raw output, editor cruft). `skills-lock.json` deliberately
  un-ignored so skill provenance stays pinned.
- [x] `verified` — Host probe re-confirmed independently: `git config user.name` = `Mfalme`,
  `user.email` = `76815758+mfalme0@users.noreply.github.com`. Git identity IS configured,
  so the first commit will not hard-fail on a missing identity.
- [x] `verified` — `skills-lock.json` inspected: schema `version: 1`, one entry per skill with
  `source`, `sourceType`, `skillPath` and a `computedHash` SHA-256 per skill. The hash is a
  content pin, so a future agent CAN detect whether `.agents/skills/` has drifted from the
  recorded source. This resolves the "no version pin" concern.
- [x] `verified` — **BUILD STARTED 2026-10-13/14. Toolchain chain proven end to end:**
  `rustc 1.99.0`, `cargo 1.99.0`, `rustup 1.29.1`, host
  `x86_64-pc-windows-msvc`. `rust-toolchain.toml` pins `channel = "1.99.0"` with
  `rustfmt` + `clippy`, and rustup auto-installed that toolchain on first build. MSVC
  linking verified by compiling, linking and executing a scratch binary.
  `cargo-nextest v0.9.148` installed via `cargo install cargo-nextest --locked`.
  NOTE FOR FUTURE AGENTS: `%USERPROFILE%\.cargo\bin` is NOT on the agent shell's PATH;
  prepend it or absolute paths fail.
- [x] `verified` — **Workspace created and compiling.** `Cargo.toml` (workspace,
  resolver 3, edition 2024, rust-version 1.99) plus five crates:
  `kifaru-domain`, `kifaru-application`, `kifaru-storage`, `kifaru-api`, `kifaru-testkit`.
  Only `kifaru-domain` has real content; the other four are compiling stubs.
  `cargo build --workspace` -> `Finished dev profile in 29.89s`.
- [x] `verified` — **`kifaru-domain` implemented.** `money.rs` (`Currency`, `Money` as
  non-negative `i64` minor units, checked add/sub, no float), `account.rs` (`AccountId`
  UUIDv7, `AccountStatus` state machine with terminal `Closed`, `debit`/`credit`),
  `transfer.rs` (`TransferId`, `TransferStatus` Pending->Completed/Failed with both
  terminal, self-transfer and zero-amount rejected), `ledger.rs` (`EntryDirection`,
  immutable `LedgerEntry` with no mutating API, `LedgerTransaction` that can only be built
  as a balanced pair, `derived_balance_minor_units` for reconciliation), `error.rs`.
- [x] `verified` — **`cargo nextest run --workspace` -> 31 tests run, 31 passed, 0 skipped**
  (0.296s). These are domain unit tests only; none touches a database.
- [x] `verified` — `cargo fmt --all --check` -> exit 0 after applying `cargo fmt --all`.
- [x] `verified` — `cargo clippy --workspace --all-targets -- -D warnings` -> exit 0,
  zero warnings.
- [x] `verified` — `docker-compose.yml` written: `postgres:17` **pinned by major, never
  `:latest`**, UTF8/`locale=C` initdb args for deterministic collation, healthcheck,
  `max_connections=200`, and `log_lock_waits=on` / `deadlock_timeout=200ms` /
  `log_min_duration_statement=250` so lock contention is visible in logs.
- [x] `verified` — `.env.example` written with placeholders only. Compose fails loudly on
  an unset `POSTGRES_PASSWORD` rather than defaulting, so there is no hardcoded credential.
- [x] `verified` — **PORT CONFLICT RESOLVED, and this is why the port differs from the
  standard.** Host port 5432 is held by an unrelated project (`betterfarm_db`,
  `postgis/postgis:16-3.4`) and 5433 by another (`neo-learn-postgres`). KIFARU therefore
  runs on **5434**. Those containers were NOT touched. `POSTGRES_PORT=5434` and
  `DATABASE_URL` updated in `.env`.
- [x] `verified` — Postgres 17 container `kifaru-postgres` up and **`healthy`** on
  `0.0.0.0:5434->5432/tcp`.
- [x] `verified` — `migrations/0001_init.sql` applied cleanly to the live database
  (BEGIN..COMMIT, 6 tables, 9 indexes, 1 trigger function + trigger, exit 0). Tables:
  `accounts`, `transfers`, `ledger_entries`, `idempotency_keys`, `outbox_events`,
  `consumer_inbox`.
- [x] `verified` — **`tests/invariants/schema_constraints.sql` executed against live
  Postgres: 16 checks, ALL ENFORCED, script exit 0.** This is real evidence, not an
  assumption: each invariant was deliberately violated and the database refused.
  Confirmed: INV-1 (insert and update), INV-2 (second debit for one transfer),
  INV-6 + per-client key scoping, INV-7 (UPDATE and DELETE both blocked by trigger),
  consumer dedupe, self-transfer, zero amount, failure-reason required/absent,
  FK to unknown account, outbox `delivered`/`dead` column consistency.
  Script rolls back, so it leaves no data behind.

## In progress

- [x] `verified` — `README.md` is **complete**, no longer in flight. It remains owned by the
  main agent and is never edited from this file. Its claims are self-labelled
  `[IMPLEMENTED]` / `[PARTIAL]` / `[PLANNED]` / `[UNSUPPORTED]`, and nothing in it is labeled
  implemented, so it stays consistent with the zero-code reality.
- [x] `verified` — `.gitignore` is **complete**, no longer in flight. It is not maintained
  from this file either; changes to it are handed to the main agent.
- [ ] `planned` — **Nothing is actively in flight.** Phase 0 design work is closed; the build
  sequence in `Next` has not started. **Next single action: the user installs the Rust
  toolchain and starts the Docker daemon** — both are owned by the user and neither has been
  confirmed done. Once confirmed, `Next` items 1–3 run immediately and nothing else blocks.

## Next

Phase 0's design work is closed. What follows is the **build sequence** — the start of Phase
1 implementation. Every item below is **planned**; **none has started**. Items 1–3 are gated
on the two user-owned blockers above and cannot start until those clear.

Build sequence:

1. `planned` — Confirm `rustc` / `cargo` / `rustup` present; record exact versions here.
2. `planned` — Install the test runner: `cargo install cargo-nextest --locked`.
   **`cargo-nextest` is a separately installed binary, not part of `cargo`.** A fresh Rust
   install does **not** include it, and `cargo nextest run` in the gate will fail until this
   step is done. `--locked` pins the resolved dependency versions.
3. `planned` — Confirm the Docker daemon is up; record Docker version here.
4. `planned` — **Pin the Postgres major version** in `docker-compose.yml` to an explicit tag
   (e.g. `postgres:16`). Never `:latest`, never a floating tag. Must be settled *before* the
   first migration is written. Decision is made; the specific tag is a one-line config choice.
5. `planned` — Create the 5-crate workspace `Cargo.toml` and per-crate manifests:
   `kifaru-domain`, `kifaru-application`, `kifaru-storage`, `kifaru-api`, `kifaru-testkit`.
6. `planned` — Write `docs/invariants.md` formalizing INV-1..INV-7 **and INV-8** with the
   exact SQL/assertion that will enforce each one in tests.
7. `planned` — Write the first Postgres migration: accounts, ledger entries, transfers,
   idempotency keys, outbox. `BIGINT` minor units, `UNIQUE (client_id, key)` on idempotency,
   outbox lease columns (`locked_until`, `locked_by`, `attempts`, `next_attempt_at`).
8. `planned` — Compose file for a local Postgres instance, on the pinned major version from
   item 4 (must be replaceable with a remote one later; nothing hardcoded to `localhost`
   inside the crates).
9. `planned` — Domain types: `Money` as `i64` minor units, single configured currency.
10. `planned` — First vertical slice end-to-end: transfer request → atomic DB transaction →
    two ledger entries + both balance updates + outbox row. Assert INV-1..INV-5 hold.
11. `planned` — Idempotency replay path: identical retry replays the stored response,
    conflicting payload returns 409, concurrent same-key is serialized by the unique index.
    Assert INV-6.
12. `planned` — **Cross-tenant idempotency key isolation tests (mandatory).** Exercise at
    least two distinct `client_id` values sending the **same** idempotency key, injected via
    `kifaru-testkit`; assert both execute independently and that INV-6 holds per-tenant. The
    single dev identity must never be allowed to leave this path untested.
13. `planned` — Immutability check on committed ledger entries; corrections only via
    compensating entries. Assert INV-7.
14. `planned` — Per-row storage round-trip test for monetary values (`i64` → `BIGINT` → `i64`
    bit-identical, `BIGINT` → `NUMERIC` exact, no silent overflow on balance arithmetic).
    Assert INV-8. Scoped to storage fidelity, **not** balance-domain headroom.
15. `planned` — Outbox claim loop with `FOR UPDATE SKIP LOCKED`, consumer dedupe on
    `event_id`. At-least-once semantics documented, no exactly-once claims.
16. `planned` — `kifaru reconcile` subcommand: `REPEATABLE READ` read-only snapshot, JSON
    report, non-zero exit on critical violation, never auto-repairs.
17. `planned` — `serve`, `worker`, `reconcile` subcommands on a single `kifaru` binary.
18. `planned` — Retry policy limited to SQLSTATE `40001` / `40P01`, max 3 attempts, jittered
    backoff, retrying the whole transaction only.
19. `planned` — First commit. Everything above is currently uncommitted.

Phase gates — do not advance until **every** listed condition is met. Each is a separate
required condition; satisfying one implies none of the others.

- **Gate 0 → 1**, all of:
  1. `rustc` / `cargo` / `rustup` present, versions recorded in this file.
  2. `cargo nextest run` is runnable (`cargo install cargo-nextest --locked` completed).
  3. Docker daemon up, Postgres container started on the pinned major version.
  4. `cargo fmt --check` — clean, no diff.
  5. `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings.
  6. `cargo build --workspace` — clean build.
  7. Migrations apply against the running Postgres, from empty to head.
  8. `cargo nextest run` — full suite green.
  9. First transfer passes end-to-end with INV-1..INV-8 asserted in automated tests,
     including the mandatory cross-tenant idempotency isolation tests from `Next` item 12.
  Actual command output for each condition must be pasted into this file. A gate is not
  passed by assertion, expectation, or reasoning.
- **Gate 1 → 2:** (not yet defined in detail; observability and fault injection phases come
  next.)

## Open questions / decisions locked

**Locked (Phase 0, agreed with the user):**

- **Money representation** — `i64` minor units stored as PostgreSQL `BIGINT`. One single
  configured currency for the whole system. No floating point anywhere.
  `rust_decimal` was considered and **rejected** for the MVP.
- **Crate layout** — lean 5 crates, deliberately fewer than the original spec's 8:
  `kifaru-domain`, `kifaru-application`, `kifaru-storage`, `kifaru-api`, `kifaru-testkit`.
  The background worker and observability live as *modules inside `kifaru-api`* for now,
  not as separate crates.
- **Binary shape** — a single `kifaru` binary with subcommands `serve`, `worker`,
  `reconcile`.
- **Isolation** — `READ COMMITTED` plus `SELECT ... FOR UPDATE`, always locking accounts in
  ascending `account_id` order. This ordering kills A→B / B→A deadlocks without needing
  retries. `SERIALIZABLE` was **explicitly rejected**.
- **Idempotency** — `UNIQUE (client_id, key)` enforced by the database, plus a stored
  `request_hash` and a committed response snapshot. Identical retry replays the stored
  response; conflicting payload returns 409; concurrent same-key requests are serialized by
  the unique index itself.
- **Outbox** — transactional outbox with lease columns (`locked_until`, `locked_by`,
  `attempts`, `next_attempt_at`), claimed via `FOR UPDATE SKIP LOCKED`. Delivery is
  **at-least-once only**. Consumers dedupe on `event_id`. **No exactly-once claims are made
  anywhere.**
- **Reconciliation** — `REPEATABLE READ` read-only snapshot, emits a JSON report, exits
  non-zero on a critical violation, and **never auto-repairs**. Repair is always a separate,
  explicit, human-invoked act.
- **Retry policy** — only SQLSTATE `40001` (serialization failure) and `40P01` (deadlock
  detected). Max 3 attempts, jittered backoff, and the retry always covers the whole
  transaction — never a fragment of one.
- **Skills** — the 10 project-scope skills listed under `Done`; `code-review` is
  intentionally left global.
- **`client_id` in Phase 0** — the running application uses a **single server-issued
  development `client_id`**. This is explicitly **NOT a security control and NOT
  authentication**, and must never be documented as either. Chosen over deferring auth so the
  build is not blocked, while the schema keeps its `UNIQUE (client_id, key)` scoping intact.
- **Test-only `client_id` override** — `kifaru-testkit` **must** allow tests to inject
  distinct `client_id` values directly at the application-service boundary, bypassing the dev
  identity. This is a hard requirement, not a convenience: a single-client dev identity must
  never be allowed to leave the 409-conflict and cross-tenant isolation paths untested.
- **Cross-tenant test obligation** — automated tests must exercise at least two distinct
  `client_id` values submitting the **same** idempotency key, asserting that both requests
  execute independently and that INV-6 holds per-tenant.
- **Postgres version pinning** — `docker-compose.yml` pins an explicit major-version tag
  (e.g. `postgres:16`). Never `:latest`, never a floating tag. Rationale: the design rests on
  `READ COMMITTED`, `SELECT ... FOR UPDATE`, `FOR UPDATE SKIP LOCKED`, `REPEATABLE READ`, and
  SQLSTATE `40001` / `40P01` behaviour; a silent major bump from `docker compose pull` is a
  **correctness risk**, not a compatibility nit.
- **Test runner** — `cargo-nextest` is the runner for the gate, installed via
  `cargo install cargo-nextest --locked`. It is a separately installed binary, **not** part of
  `cargo`, and a fresh Rust install will not have it.

**Invariants to be formalized in `docs/invariants.md` (defined, not yet enforced by code):**

- INV-1 No account balance is negative (default overdraft policy).
- INV-2 Every completed transfer has exactly two ledger entries: same `transfer_id`, same
  currency, equal magnitude, opposite direction.
- INV-3 Every ledger transaction balances (sum debits == sum credits).
- INV-4 Stored account balance == balance derived from ledger entries.
- INV-5 No completed transfer without its ledger entries and both account updates committed
  atomically.
- INV-6 Duplicate idempotent requests never produce duplicate ledger entries or a second
  balance movement.
- INV-7 Committed ledger entries are immutable; corrections only via compensating entries.

**INV-8 (proposed, NOT yet formalized)** — agreed this session; still needs to be written into
`docs/invariants.md` alongside INV-1..INV-7.

- **INV-8** Monetary values round-trip exactly through storage and reporting. Specifically:
  (a) an `i64` minor-unit value stored in `BIGINT` and read back is **bit-identical**;
  (b) a `BIGINT` cast to `NUMERIC` for reporting **preserves the exact value**; and
  (c) **no arithmetic on stored balances silently overflows**.
  **Scope: per-row storage round-trip fidelity. This is NOT a claim about balance-domain
  headroom** — the ledger is not expected to hold balances anywhere near `i64::MAX`, and no
  test should assert that it can.
  INV-1..INV-7 do not cover this: they constrain ledger *consistency*, not numeric fidelity.
  Two real risks remain live: (i) **`SUM()` aggregate overflow** across many rows — a
  documented, tested **upper bound on total system value** is required and is a named
  consequence to settle when the schema is written, not a blocker now; and (ii) lossy or
  silently-widening `BIGINT` → `NUMERIC` casts in reporting SQL.

**Carried consequences (settled direction, one-line config choice outstanding — not blockers):**

- Postgres major-version tag to write into `docker-compose.yml`. The *decision* to pin is
  locked above; only the specific tag remains, and it must be fixed before the first
  migration.
- Documented, tested upper bound on total system value (follows from INV-8's aggregate
  overflow risk). Settle when the schema is written.
- Which currency and minor-unit scale to configure (e.g. KES with 2 decimals vs. a
  zero-decimal minor unit). Affects seed data and test fixtures, not the schema.

**Genuinely open (deferred, deliberately not blocking Phase 0):**

- Auth token strategy for a real deployment. Phase 0's dev `client_id` is explicitly not a
  security control; this is the later work that replaces it.
- Whether the observability module in `kifaru-api` should be promoted to its own crate later,
  and at what threshold. Not a Phase 0 question.
- Whether idempotency keys should eventually be scoped per-client-tenant rather than only by
  `client_id`. Tied to the auth work above.

## Session log

### 2026-10-13

- Session 1 (foundation + design).
- Confirmed `D:\projects\kifaru` was completely empty. Ran `git init -b main`. No commits
  have been made since.
- Worked through the architecture with the user and locked: `i64` minor units in `BIGINT`
  with one configured currency (rejecting `rust_decimal`); a lean 5-crate workspace instead
  of the spec's 8, with worker and observability folded into `kifaru-api`; a single `kifaru`
  binary exposing `serve` / `worker` / `reconcile`; `READ COMMITTED` + `SELECT ... FOR UPDATE`
  with ascending-`account_id` lock ordering as the deadlock-avoidance strategy, explicitly
  rejecting `SERIALIZABLE`; database-enforced idempotency via `UNIQUE (client_id, key)` with a
  stored `request_hash` and response snapshot; a transactional outbox with lease columns
  claimed via `FOR UPDATE SKIP LOCKED` at at-least-once delivery only; reconciliation as a
  `REPEATABLE READ` read-only snapshot that reports and never repairs; and retries limited to
  SQLSTATE `40001`/`40P01`, 3 attempts max, whole transaction only.
- Defined invariants INV-1 through INV-7 in prose. Formalization in `docs/invariants.md` is
  not started.
- Probed the host: Windows 11 build 26200, 16 logical cores, 31.4 GB RAM. `rustc`, `cargo`
  and `rustup` are **not installed**. Docker CLI and Compose v5.3.1 are installed but the
  **daemon is not running** (Linux engine pipe dead). `psql` absent (acceptable — Docker
  covers it). `k6` and `vegeta` absent (deferred to Phase 4).
- User agreed to install the Rust toolchain and start Docker Desktop. **Neither has been
  confirmed done.** Until both are, every Phase 0 build item is blocked.
- Installed 10 project-scope skills into `.agents/skills/` (copied, not symlinked), tracked
  via `skills-lock.json`. Deliberately did not duplicate the user's existing global
  `code-review` skill.
- Adopted the single-writer, append-only protocol for `progress.md`. `README.md` is owned by
  the main agent and is being written concurrently; it is out of scope for this file.
- Learned / constraint to carry forward: status must always be labeled verified vs. planned
  vs. blocked, and no test result, benchmark number or measurement may be written down unless
  it came from an actual command run and was observed. Right now there are none.

### 2026-10-13 — reconciliation pass (phase-boundary)

- A second writing agent produced `README.md` and `.gitignore`. Both are **complete**; neither
  is in flight any more. Both are out of scope for this file's writers — requests to change
  them go to the main agent.
- Keeper ran a self-critique and raised seven concerns. Seven were adjudicated with hard
  evidence or accepted as real gaps:
  - Host specs (16 cores / 31.4 GB) **stay `verified`** — genuinely observed via
    `Win32_ComputerSystem.NumberOfLogicalProcessors` and
    `Win32_OperatingSystem.TotalVisibleMemorySize/1MB`. The probe commands are now recorded
    inline so any future reader can re-run them. Not demoted.
  - The "skills have no version pin" concern is **wrong**. `skills-lock.json` carries a
    `computedHash` (SHA-256) per skill, which is a content pin; a future agent can hash-diff
    `.agents/skills/` against the lockfile to detect drift.
  - The git-identity concern is **wrong** — identity is configured (`Mfalme`,
    `76815758+mfalme0@users.noreply.github.com`). Never recorded as a blocker.
  - Gate 0→1 was too loose: it did not name `fmt`, `clippy`, or the test runner. **Fixed** —
    it is now eight separately-required conditions with commands named, and passing output
    must be pasted in. A gate is not passed by assertion.
  - **New risk recorded:** Postgres major version is unpinned. An unpinned tag plus
    `docker compose pull` can silently change the isolation and locking semantics the entire
    design depends on. Must be pinned before the first migration.
  - **New open decision recorded:** idempotency scoping assumes a `client_id` that nothing
    authenticates in Phase 0. Schema must not be frozen until auth/`client_id` is decided.
  - **INV-8 added as proposed:** exact monetary round-trip, including at `i64` boundary
    magnitudes. INV-1..INV-7 constrain ledger consistency, not numeric fidelity, so this was
    a genuine coverage gap — aggregate `SUM()` overflow and `BIGINT` → `NUMERIC` casts in
    reporting were both uncovered.
- `Next` renumbered 15 → 18 items; the two new decisions were inserted ahead of the first
  migration so they cannot be skipped past. Still zero commits, still zero code.
- Learned: "the gate says the workspace builds clean" is not the same as "fmt, clippy, build
  and tests were each run and observed". Gates must enumerate commands, not adjectives.

### 2026-10-13 — final reconciliation, Phase 0 closed

- Four remaining concerns were adjudicated and all four are now **decided**, not open:
  - **Test runner** — `cargo nextest run` stays the gate command, but
    `cargo install cargo-nextest --locked` is now `Next` item 2, *before* anything depends on
    it, and it is now also Gate condition 2. The file states plainly that `cargo-nextest` is a
    separately installed binary, not part of `cargo`, and absent from a fresh Rust install.
    This satisfies the spec's "cargo-nextest or an equivalent reliable test runner". The
    corresponding open question was removed.
  - **`client_id` / auth** — option (a) adopted. The running app uses a single server-issued
    development `client_id`, documented as **not** a security control and **not**
    authentication. Paired with two new hard requirements: `kifaru-testkit` must let tests
    inject distinct `client_id` values at the application-service boundary, and cross-tenant
    idempotency isolation is a **mandatory** test obligation — two distinct `client_id`
    values, the same idempotency key, asserting independent execution and INV-6 per-tenant.
    The point is that a single-client dev identity must not leave the 409-conflict and
    cross-tenant isolation paths untested. Both open questions about auth and tenant scoping
    were removed; the decision is recorded under locked decisions.
  - **INV-8 narrowed** — now scoped to **per-row storage round-trip fidelity**: `i64` →
    `BIGINT` → `i64` bit-identical; `BIGINT` → `NUMERIC` exact; no silent overflow in balance
    arithmetic. Explicitly **not** a claim about balance-domain headroom — the ledger is not
    expected to hold balances anywhere near `i64::MAX`. Both real risks kept: `SUM()`
    aggregate overflow, and `BIGINT` → `NUMERIC` casts in reporting SQL.
  - **`git status` duplication** — raw command output now lives only in `Done`; `Current
    state` states zero commits without repeating it. Already satisfied by the previous pass
    and confirmed here.
- **Phase 0 is CLOSED.** Design complete: architecture, INV-1..INV-8, crate layout,
  isolation, idempotency, outbox, reconciliation, retry policy, test runner, Postgres
  pinning, `client_id` story. Build sequence renumbered 18 → 19 items. A minor markdown
  duplication in the gate list (a stray repeated `Gate 1 → 2` line) was found and fixed during
  this pass.
- Added a "Carried consequences" subsection for settled-direction items that still need one
  concrete choice (the exact Postgres tag, the total-system-value bound, the currency scale).
  These are explicitly **not blockers**.
- Unchanged and must stay honest: **zero code, zero commits, zero tests run, zero
  measurements taken.** Nothing in this file implies otherwise.
- Learned: "no auth in Phase 0" was never really a security posture — it was an unassigned
  value in a column the unique index depends on. Deciding who *sets* `client_id`, and giving
  tests a way to vary it, is what makes INV-6 and the 409 path testable at all.

## How to use this file

**Protocol — single writer, append-only.**

- `progress.md` has exactly one writer at a time: the session acting as progress keeper. The
  main agent appends **single lines** at checkpoints and never performs full rewrites.
- Reconciling (ticking boxes, moving items between sections, refreshing `Current state`) is
  done **only at phase boundaries and at session end**, never mid-stream.
- Any other agent that needs to record something must hand it to the keeper rather than
  editing this file. `README.md` and `.gitignore` are owned by a different agent and are
  never touched from here; this file only records their status.
- Every item carries an explicit status: **verified**, **planned**, or **blocked**. Nothing
  may be phrased so as to imply that code exists, that tests pass, or that a benchmark ran,
  unless that was directly observed. Never invent a test result or a measurement.

**Resuming after a token cutoff.**

This file is the resume point on its own. Read it top to bottom and you should be able to
continue without re-deriving context:

1. `Current state` gives you the whole picture in one paragraph.
2. `Blockers` tells you what you cannot do yet and who owns it.
3. `In progress` names the single next action. As of the final Phase 0 reconciliation, that
   action belongs to the **user**, not to an agent: install Rust, start Docker.
4. `Next` is the ordered build sequence, with the current phase gate spelled out.
5. `Open questions / decisions locked` tells you what is already decided — do not relitigate
   these — and what genuinely still needs a decision.
6. The newest entry under `Session log` tells you what just happened and what was learned.

**Phase 0 is closed.** If you are resuming cold, you are starting implementation. Do not
reopen the locked decisions; read them and build. The honest starting state is: zero code,
zero commits, zero test runs. The first thing worth doing is item 1 of `Next`.

If a claim in this file cannot be traced to a command that was actually run, treat it as
unverified. When a blocker clears, update `Blockers`, update `Done` with the observed version
output, and note the clearing in the session log. A full rewrite like this one is only
authorized at a phase boundary or at session end — never mid-stream.
### 2026-10-14 - Phase 1 and 2 core VERIFIED: atomic transfer + outbox worker

Append-only checkpoint. Statuses are as observed by actually running the commands
named below; nothing here is projected.

**erified - core correctness (Phase 1), observed by cargo nextest run --workspace:**

- Atomic transfer execution. One PostgreSQL transaction covers both balance
  updates, the transfer row, both ledger entries, the outbox event, and the
  idempotency record. Port design was changed first: the original per-entity
  repository traits each took a `PgPool` and would therefore have opened
  separate transactions, making atomicity structurally impossible. Replaced with
  a single coarse `TransferRepository::execute` whose contract *is* atomicity.
- Deadlock avoidance by lock ordering: one `SELECT ... WHERE id = ANY()
  ORDER BY id FOR UPDATE`. `READ COMMITTED` retained; row locks are the
  correctness mechanism. `SERIALIZABLE` still rejected.
- Idempotency via `INSERT ... ON CONFLICT DO NOTHING` to claim the key, then
  `FOR UPDATE` which blocks a duplicate until the winner commits and replays
  the committed outcome. Same key + different body = 409.
- A rejected transfer is recorded (`failed` row + idempotency record, no money
  moved, no ledger entries) so a retry is deterministic.
- 87 tests total: domain 31, application 13, storage 15 (2 unit + 13
  integration), api 28 (13 unit + 14 integration + 7 config).
- Concurrency proof: 20 simultaneous 100-minor-unit transfers from a 1000
  balance yield exactly 10 commits and 10 refusals, landing on exactly zero.

**erified - outbox worker (Phase 2), observed by `cargo nextest run` plus a
live `serve` + `worker` smoke run:**

- Claim-and-lease in a single statement with `FOR UPDATE SKIP LOCKED`, so two
  concurrent workers take disjoint batches and there is no select-but-not-lease
  window.
- At-least-once: an expired lease makes an event redeliverable. Tested by
  claiming with a 1s lease, asserting no other worker may take it during the
  lease, then asserting redelivery after expiry.
- Consumer-side dedupe via `consumer_inbox`; a redelivered event yields exactly
  one side effect, and dedupe is scoped per consumer.
- Retry with exponential backoff, dead-lettering at `max_attempts`, per-event
  failure isolation (a poison event does not abort its batch).
- Two bugs found by these tests and fixed: `1u32 << 32` shift overflow in
  `backoff_for` (panicked in debug), and a claim/lease query that could report
  `attempts` inconsistently.
- Smoke: HTTP POST returned 201, identical retry returned 200 `replayed:true`,
  same key + different body returned 409; `worker` drained 60 pending to 0
  (112 -> 172 delivered).

**erified - two schema/procedural problems found and fixed:**

1. `migrations/0001_init.sql` contained explicit `BEGIN;`/`COMMIT;` while
   the harness applied it via psql. sqlx manages its own transaction and records
   rows in `_sqlx_migrations`, which did not exist. Migrations are now
   sqlx-owned and `migrate` is idempotent. Verified: `_sqlx_migrations`
   records versions 1 and 2.
2. `outbox_dead_has_error` was a biconditional,
   `(state='dead') = (last_error IS NOT NULL)`, which made a *pending* event's
   last failure **unrepresentable**. A retrying event could show only that it had
   been attempted N times, never why. Migration 0002 relaxes it to an implication,
   preserving the guarantee that mattered (a dead event always carries its error)
   while making the retry path diagnosable. This is not an INV-n change.

**erified - DB invariant script: 17/17 ENFORCED** (was 16). Added
`outbox-pending-error` to assert the 0002 relaxation has not been reverted.

**erified - test isolation fixed.** The outbox is one global queue, so parallel
tests drained each other's events and assertions about claim counts were racy.
Each test now gets a private PostgreSQL schema (`Harness` sets `search_path`
via `after_connect` and runs migrations into it, dropping it on cleanup). The
suite is parallel and non-flaky. The DB harness moved into `kifaru-testkit`
so storage and api tests share one definition.

**planned - next, in order:**

1. `kifaru reconcile`: read-only `REPEATABLE READ` snapshot, per-account
   stored-vs-derived comparison, JSON report, non-zero exit on critical, never
   auto-repair.
2. INV-8 assertions: per-row `i64` -> `BIGINT` -> `i64` round-trip.
3. HTTP integration tests over the router (currently the API is covered by unit
   tests plus a manual smoke run, not by an automated end-to-end test).
4. Known gap to close or document: no money-*in* path, so a transfer cannot be
   driven over HTTP without seeding a balance directly.

**Learned:** a coarse port whose contract is atomicity is a better shape than
several fine-grained ports that each open their own connection. And an integration
suite that shares a global queue is not parallel-safe no matter how careful the
individual assertions are; isolation belongs at the schema level.
