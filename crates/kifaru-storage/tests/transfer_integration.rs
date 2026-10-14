//! Database-backed proof of the core financial invariants.
//!
//! Every test here talks to a real PostgreSQL. They exercise the code path the
//! binary uses — [`kifaru_application::TransferService`] over
//! [`kifaru_storage::PgTransferRepository`] — rather than a mock, because the
//! properties being asserted (atomicity, lock ordering, constraint enforcement)
//! are properties *of the database interaction* and cannot be demonstrated
//! without one.
//!
//! Run with:
//!
//! ```text
//! docker compose up -d
//! cargo nextest run -p kifaru-storage
//! ```

use kifaru_application::ports::{AccountDirectory, SubmitTransfer, TransferReader};
use kifaru_application::{TransferService, TransferServiceError};
use kifaru_domain::{AccountId, TransferStatus};
use kifaru_storage::sqlx;
use kifaru_testkit::harness::{Harness, client, key};
use std::sync::Arc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// INV-5 — atomicity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv5_transfer_commits_balance_ledger_and_outbox_together() {
    let h = Harness::new().await;
    let src = h.open_funded(10_000).await;
    let dst = h.open_account().await;
    let (c, k) = (client("inv5"), key("inv5"));

    let result = h.submit(src.id, dst.id, 2_500, &c, &k).await.unwrap();

    assert_eq!(result.status, TransferStatus::Completed);
    assert!(!result.replayed);
    assert_eq!(result.amount_minor, 2_500);

    // Balances moved.
    assert_eq!(h.stored_balance(src.id).await, 7_500);
    assert_eq!(h.stored_balance(dst.id).await, 2_500);

    // Exactly one balanced pair of ledger entries (INV-2).
    let entries = h
        .repo
        .ledger_for_transfer(result.transfer_id)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2, "a transfer posts exactly two entries");
    let net: i64 = entries.iter().map(|e| e.signed_minor_units()).sum();
    assert_eq!(net, 0, "INV-3: the pair must net to zero");
    let debit = entries
        .iter()
        .find(|e| e.direction == kifaru_domain::EntryDirection::Debit)
        .expect("a debit leg");
    let credit = entries
        .iter()
        .find(|e| e.direction == kifaru_domain::EntryDirection::Credit)
        .expect("a credit leg");
    assert_eq!(debit.account_id, src.id);
    assert_eq!(credit.account_id, dst.id);
    assert_eq!(debit.amount.minor_units(), 2_500);
    assert_eq!(credit.amount.minor_units(), 2_500);

    // Exactly one outbox event, committed with the transfer.
    assert_eq!(h.outbox_count(result.transfer_id).await, 1);

    // The transfer row is terminal and successful.
    let stored = h
        .repo
        .find_transfer(result.transfer_id)
        .await
        .unwrap()
        .expect("transfer row");
    assert_eq!(stored.status, TransferStatus::Completed);
    assert!(stored.failure_reason.is_none());

    assert_eq!(h.idempotency_status(&c, &k).await, Some(201));

    h.assert_inv4(src.id, 10_000).await;
    h.assert_inv4(dst.id, 0).await;
}

#[tokio::test]
async fn inv4_stored_balance_agrees_with_ledger_across_a_sequence() {
    let h = Harness::new().await;
    let a = h.open_funded(5_000).await;
    let b = h.open_account().await;
    let c = h.open_account().await;

    let cid = client("inv4");
    h.submit(a.id, b.id, 1_200, &cid, &key("inv4a"))
        .await
        .unwrap();
    h.submit(b.id, c.id, 700, &cid, &key("inv4b"))
        .await
        .unwrap();
    h.submit(c.id, a.id, 250, &cid, &key("inv4c"))
        .await
        .unwrap();

    assert_eq!(h.stored_balance(a.id).await, 4_050);
    assert_eq!(h.stored_balance(b.id).await, 500);
    assert_eq!(h.stored_balance(c.id).await, 450);

    // INV-4 for every account touched, including the one with an opening balance.
    h.assert_inv4(a.id, 5_000).await;
    h.assert_inv4(b.id, 0).await;
    h.assert_inv4(c.id, 0).await;
}

// ---------------------------------------------------------------------------
// INV-1 — no overdraft, including under concurrency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv1_concurrent_transfers_from_one_source_never_overspend() {
    let h = Harness::new().await;

    // 1,000 available; 20 attempts of 100 to 20 distinct destinations. Exactly
    // ten can succeed, no matter how the requests interleave.
    let src = h.open_funded(1_000).await;
    let mut destinations = Vec::new();
    for _ in 0..20 {
        destinations.push(h.open_account().await.id);
    }

    let service = Arc::new(TransferService::new(h.repo.clone()));
    let src_id = src.id;

    let mut handles = Vec::new();
    for (i, destination) in destinations.iter().enumerate() {
        let svc = Arc::clone(&service);
        let destination = *destination;
        let c = client("conc");
        let k = key(&format!("conc-{i}"));
        handles.push(tokio::spawn(async move {
            svc.submit(SubmitTransfer {
                source: src_id,
                destination,
                amount_minor: 100,
                client_id: c,
                idempotency_key: k,
            })
            .await
        }));
    }

    let mut succeeded = 0;
    let mut refused = 0;
    for handle in handles {
        match handle.await.expect("task did not panic") {
            Ok(_) => succeeded += 1,
            Err(TransferServiceError::InsufficientFunds { .. }) => refused += 1,
            Err(other) => panic!("unexpected error under concurrency: {other:?}"),
        }
    }

    assert_eq!(
        succeeded, 10,
        "exactly the affordable transfers must commit"
    );
    assert_eq!(
        refused, 10,
        "the rest must be refused, not silently applied"
    );

    let final_balance = h.stored_balance(src_id).await;
    assert_eq!(
        final_balance, 0,
        "INV-1: the source must land exactly on zero"
    );
    assert!(final_balance >= 0);

    // The ledger must explain exactly the ten committed movements and nothing
    // more — this is what catches a lost or double-applied update.
    assert_eq!(h.derived_balance(src_id).await, -1_000);
    h.assert_inv4(src_id, 1_000).await;

    // Exactly ten of the twenty destinations were credited, one each, and the
    // total credited equals the amount that left the source. This is what proves
    // no movement was lost or duplicated under contention.
    let mut credited = 0;
    let mut untouched = 0;
    for destination in &destinations {
        let balance = h.stored_balance(*destination).await;
        match balance {
            100 => credited += 1,
            0 => untouched += 1,
            other => panic!("a destination holds {other}, expected 100 or 0"),
        }
        // INV-4 holds for every destination whether or not it was credited.
        h.assert_inv4(*destination, 0).await;
    }
    assert_eq!(credited, 10, "exactly ten destinations were credited");
    assert_eq!(untouched, 10);

    let completed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transfers WHERE source_account_id = $1 AND status = 'completed'",
    )
    .bind(src_id.as_uuid())
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(completed, 10);
}

// ---------------------------------------------------------------------------
// INV-6 — idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv6_identical_retry_replays_and_moves_money_once() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;
    let (c, k) = (client("replay"), key("replay"));

    let first = h.submit(src.id, dst.id, 400, &c, &k).await.unwrap();
    let second = h.submit(src.id, dst.id, 400, &c, &k).await.unwrap();

    assert_eq!(first.transfer_id, second.transfer_id);
    assert!(!first.replayed);
    assert!(second.replayed, "the second call must be a replay");

    assert_eq!(
        h.stored_balance(src.id).await,
        600,
        "money moved exactly once"
    );
    assert_eq!(h.stored_balance(dst.id).await, 400);
    assert_eq!(h.ledger_len(first.transfer_id).await, 2);
    assert_eq!(h.outbox_count(first.transfer_id).await, 1);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM transfers WHERE id = $1")
        .bind(first.transfer_id.as_uuid())
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "a replay must not create a second transfer row");
}

#[tokio::test]
async fn inv6_cross_tenant_same_key_executes_independently() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;

    // The same key *string*, deliberately. Tenant scoping is the whole point.
    let shared_key = key("shared");
    let tenant_a = client("tenant-a");
    let tenant_b = client("tenant-b");

    let a = h
        .submit(src.id, dst.id, 300, &tenant_a, &shared_key)
        .await
        .unwrap();
    let b = h
        .submit(src.id, dst.id, 300, &tenant_b, &shared_key)
        .await
        .unwrap();

    assert_ne!(
        a.transfer_id, b.transfer_id,
        "distinct tenants are distinct"
    );
    assert!(!b.replayed, "tenant B must not see tenant A's result");
    assert_eq!(
        h.stored_balance(src.id).await,
        400,
        "both transfers executed"
    );
    assert_eq!(h.stored_balance(dst.id).await, 600);

    // Each tenant still replays its own result correctly.
    let a2 = h
        .submit(src.id, dst.id, 300, &tenant_a, &shared_key)
        .await
        .unwrap();
    assert!(a2.replayed);
    assert_eq!(a2.transfer_id, a.transfer_id);
    assert_eq!(h.stored_balance(src.id).await, 400, "no third movement");
}

#[tokio::test]
async fn inv6_same_key_with_a_different_body_is_a_conflict() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;
    let (c, k) = (client("conflict"), key("conflict"));

    h.submit(src.id, dst.id, 100, &c, &k).await.unwrap();

    let err = h.submit(src.id, dst.id, 200, &c, &k).await.unwrap_err();
    assert_eq!(err, TransferServiceError::IdempotencyConflict);

    // The conflicting request must not have executed.
    assert_eq!(h.stored_balance(src.id).await, 900);
    assert_eq!(h.stored_balance(dst.id).await, 100);
}

#[tokio::test]
async fn inv6_rejected_transfer_replays_the_same_typed_error() {
    let h = Harness::new().await;
    let src = h.open_funded(50).await;
    let dst = h.open_account().await;
    let (c, k) = (client("rejected"), key("rejected"));

    let first = h.submit(src.id, dst.id, 500, &c, &k).await.unwrap_err();
    assert_eq!(
        first,
        TransferServiceError::InsufficientFunds {
            balance: 50,
            required: 500
        }
    );

    // A retry must reproduce the original rejection, not re-evaluate — and must
    // certainly not report success.
    let second = h.submit(src.id, dst.id, 500, &c, &k).await.unwrap_err();
    assert_eq!(second, first, "replay of a rejection must be identical");

    // Nothing moved, and no ledger or outbox side effects exist.
    assert_eq!(h.stored_balance(src.id).await, 50);
    assert_eq!(h.stored_balance(dst.id).await, 0);
    assert_eq!(h.idempotency_status(&c, &k).await, Some(422));

    let (status, reason): (String, Option<String>) = sqlx::query_as(
        "SELECT status, failure_reason FROM transfers t
         JOIN idempotency_keys k ON k.transfer_id = t.id
         WHERE k.client_id = $1 AND k.key = $2",
    )
    .bind(&c)
    .bind(&k)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert!(reason.is_some(), "a recorded failure must carry a reason");
}

#[tokio::test]
async fn account_not_found_releases_the_idempotency_key() {
    let h = Harness::new().await;
    let src = h.open_funded(500).await;
    let ghost = AccountId::new();
    let (c, k) = (client("missing"), key("missing"));

    let err = h.submit(src.id, ghost, 100, &c, &k).await.unwrap_err();
    assert!(matches!(err, TransferServiceError::AccountNotFound { .. }));

    // The key must be released, not permanently consumed by a request that never
    // had a chance to execute.
    assert_eq!(
        h.idempotency_status(&c, &k).await,
        None,
        "a request that never executed must not consume its key"
    );

    // A corrected request reusing the key must succeed.
    let dst = h.open_account().await;
    let ok = h.submit(src.id, dst.id, 100, &c, &k).await.unwrap();
    assert_eq!(ok.status, TransferStatus::Completed);
    assert_eq!(h.stored_balance(dst.id).await, 100);
}

// ---------------------------------------------------------------------------
// INV-2 / INV-7 — structural ledger protections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv2_a_second_debit_for_one_transfer_is_rejected() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;
    let result = h
        .submit(src.id, dst.id, 100, &client("inv2"), &key("inv2"))
        .await
        .unwrap();

    let attempt = sqlx::query(
        "INSERT INTO ledger_entries
             (id, transfer_id, account_id, direction, amount_minor, currency, created_at)
         VALUES ($1, $2, $3, 'debit', $4, 'KES', now())",
    )
    .bind(Uuid::now_v7())
    .bind(result.transfer_id.as_uuid())
    .bind(src.id.as_uuid())
    .bind(50_i64)
    .execute(&h.pool)
    .await;

    assert!(
        attempt.is_err(),
        "INV-2: a transfer must not be able to hold two debits"
    );
}

#[tokio::test]
async fn inv7_committed_ledger_entries_cannot_be_updated_or_deleted() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;
    let result = h
        .submit(src.id, dst.id, 100, &client("inv7"), &key("inv7"))
        .await
        .unwrap();

    let update = sqlx::query("UPDATE ledger_entries SET amount_minor = 1 WHERE transfer_id = $1")
        .bind(result.transfer_id.as_uuid())
        .execute(&h.pool)
        .await;
    assert!(update.is_err(), "INV-7: ledger entries must be immutable");

    let delete = sqlx::query("DELETE FROM ledger_entries WHERE transfer_id = $1")
        .bind(result.transfer_id.as_uuid())
        .execute(&h.pool)
        .await;
    assert!(delete.is_err(), "INV-7: ledger entries must be immutable");

    // The entries are untouched.
    assert_eq!(h.ledger_len(result.transfer_id).await, 2);
    assert_eq!(h.stored_balance(src.id).await, 900);
}

// ---------------------------------------------------------------------------
// Storage-layer correctness details
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rejected_transfer_writes_no_ledger_entries() {
    let h = Harness::new().await;
    let src = h.open_funded(10).await;
    let dst = h.open_account().await;
    let (c, k) = (client("noledger"), key("noledger"));

    h.submit(src.id, dst.id, 999, &c, &k).await.unwrap_err();

    let ledgers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ledger_entries e
         JOIN idempotency_keys k ON k.transfer_id = e.transfer_id
         WHERE k.client_id = $1 AND k.key = $2",
    )
    .bind(&c)
    .bind(&k)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(ledgers, 0, "a refusal must post nothing to the ledger");

    let outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events o
         JOIN idempotency_keys k ON k.transfer_id = o.aggregate_id
         WHERE k.client_id = $1 AND k.key = $2",
    )
    .bind(&c)
    .bind(&k)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(outbox, 0, "a refusal must not emit an event");
}

#[tokio::test]
async fn a_suspended_account_cannot_move_money() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;

    sqlx::query("UPDATE accounts SET status = 'suspended' WHERE id = $1")
        .bind(src.id.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();

    let err = h
        .submit(src.id, dst.id, 100, &client("susp"), &key("susp"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, TransferServiceError::AccountNotActive { .. }),
        "got {err:?}"
    );
    assert_eq!(h.stored_balance(src.id).await, 1_000, "nothing moved");
}

#[tokio::test]
async fn account_version_advances_exactly_once_per_movement() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;

    let before_src = h.accounts.find(src.id).await.unwrap().unwrap().version;
    let before_dst = h.accounts.find(dst.id).await.unwrap().unwrap().version;

    h.submit(src.id, dst.id, 100, &client("ver"), &key("ver"))
        .await
        .unwrap();

    assert_eq!(
        h.accounts.find(src.id).await.unwrap().unwrap().version,
        before_src + 1
    );
    assert_eq!(
        h.accounts.find(dst.id).await.unwrap().unwrap().version,
        before_dst + 1
    );
}
