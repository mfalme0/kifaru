//! Proof of the outbox delivery contract.
//!
//! These tests pin down the four properties that make a transactional outbox
//! worth having, and none of them are observable without a real database:
//!
//! 1. A committed transfer leaves exactly one pending event.
//! 2. Claiming is exclusive — concurrent workers take disjoint batches.
//! 3. A crashed worker's lease expires and its event is redelivered
//!    (at-least-once, never at-most-once).
//! 4. Redelivery is harmless because the consumer deduplicates on `event_id`,
//!    and a poison event is dead-lettered rather than retried forever.
//!
//! Run with:
//!
//! ```text
//! docker compose up -d
//! cargo nextest run -p kifaru-api --test outbox_worker
//! ```

use async_trait::async_trait;
use kifaru_api::worker::{
    DedupeConsumer, EventPublisher, OutboxWorker, PublishError, WorkerConfig,
};
use kifaru_storage::OutboxEvent;
use kifaru_testkit::harness::{Harness, client, key};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Fault-injecting publishers
// ---------------------------------------------------------------------------

/// Always succeeds.
struct OkPublisher;

#[async_trait]
impl EventPublisher for OkPublisher {
    async fn publish(&self, _: &OutboxEvent) -> Result<(), PublishError> {
        Ok(())
    }
}

/// Fails the first `failures` attempts of every event, then succeeds.
///
/// Counts total failures so a test can assert it was actually retried rather
/// than silently succeeding first time.
struct FlakyPublisher {
    failures_remaining: AtomicUsize,
    failures: AtomicUsize,
}

impl FlakyPublisher {
    fn new(failures: usize) -> Self {
        Self {
            failures_remaining: AtomicUsize::new(failures),
            failures: AtomicUsize::new(0),
        }
    }

    fn failures(&self) -> usize {
        self.failures.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EventPublisher for FlakyPublisher {
    async fn publish(&self, _: &OutboxEvent) -> Result<(), PublishError> {
        if self
            .failures_remaining
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(PublishError::Transient("injected fault".into()));
        }
        Ok(())
    }
}

/// Always fails, so attempts can be exhausted.
struct AlwaysFailPublisher;

#[async_trait]
impl EventPublisher for AlwaysFailPublisher {
    async fn publish(&self, _: &OutboxEvent) -> Result<(), PublishError> {
        Err(PublishError::Permanent("injected poison event".into()))
    }
}

/// Fails only events of a given type; everything else succeeds.
struct SelectiveFailingPublisher {
    failing_type: &'static str,
}

impl SelectiveFailingPublisher {
    fn new(failing_type: &'static str) -> Self {
        Self { failing_type }
    }
}

#[async_trait]
impl EventPublisher for SelectiveFailingPublisher {
    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        if event.event_type == self.failing_type {
            return Err(PublishError::Permanent(format!(
                "injected failure for {}",
                self.failing_type
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Worker config tuned for tests: no backoff delay, so a retry is immediately
/// claimable and the test does not have to sleep.
fn fast_config(worker_id: &str, max_attempts: i32) -> WorkerConfig {
    WorkerConfig {
        worker_id: worker_id.to_string(),
        batch_size: 50,
        lease: Duration::from_secs(30),
        max_attempts,
        poll_interval: Duration::from_millis(10),
        backoff_base: Duration::ZERO,
        backoff_max: Duration::ZERO,
        consumer_name: "simulated-consumer".to_string(),
    }
}

fn worker(h: &Harness, publisher: Arc<dyn EventPublisher>, config: WorkerConfig) -> OutboxWorker {
    OutboxWorker::new(h.outbox.clone(), publisher, config).expect("valid worker config")
}

// ---------------------------------------------------------------------------
// 1. A transfer produces exactly one event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_committed_transfer_produces_exactly_one_pending_event() {
    let h = Harness::new().await;
    let src = h.open_funded(1_000).await;
    let dst = h.open_account().await;

    let result = h
        .submit(src.id, dst.id, 400, &client("outbox"), &key("outbox"))
        .await
        .unwrap();

    assert_eq!(h.outbox_count(result.transfer_id).await, 1);

    let event_id = h.outbox_id_for(result.transfer_id).await;
    let row = h.outbox_row(event_id).await;

    assert_eq!(row.state.as_str(), "pending");
    assert_eq!(
        row.attempts, 0,
        "an undelivered event has not been attempted"
    );
    assert!(row.delivered_at.is_none());
    assert!(row.locked_by.is_none(), "nothing has claimed it yet");

    // The event exists in the SAME transaction as the money movement, so an
    // event without a transfer (or a transfer without an event) is the failure
    // this design exists to prevent.
    let transfers: i64 = sqlx::query_scalar("SELECT count(*) FROM transfers WHERE id = $1")
        .bind(result.transfer_id.as_uuid())
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(transfers, 1);
}

// ---------------------------------------------------------------------------
// 2. Delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_worker_delivers_a_pending_event() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;

    let w = worker(&h, Arc::new(OkPublisher), fast_config("w1", 3));
    let report = w.run_once().await.unwrap();

    assert_eq!(report.claimed, 1);
    assert_eq!(report.delivered, 1);
    assert_eq!(report.retried, 0);
    assert_eq!(report.dead, 0);

    let row = h.outbox_row(event_id).await;
    assert_eq!(row.state.as_str(), "delivered");
    assert!(
        row.delivered_at.is_some(),
        "a delivered event must say when"
    );
    assert!(row.locked_by.is_none(), "the lease must be released");
    assert!(row.locked_until.is_none());
    assert!(row.last_error.is_none());

    // A delivered event is terminal: it is never claimed again.
    let second = w.run_once().await.unwrap();
    assert_eq!(second.claimed, 0);
}

#[tokio::test]
async fn claiming_is_exclusive_across_concurrent_workers() {
    let h = Harness::new().await;
    let total = 25;
    for _ in 0..total {
        h.insert_pending_event("transfer.completed").await;
    }

    let a = h.outbox.clone();
    let b = h.outbox.clone();

    // Both workers claim in genuinely overlapping tasks.
    let (left, right) = tokio::join!(
        a.claim_batch("worker-a", Duration::from_secs(30), total as i64),
        b.claim_batch("worker-b", Duration::from_secs(30), total as i64),
    );
    let left = left.unwrap();
    let right = right.unwrap();

    let left_ids: Vec<_> = left.iter().map(|e| e.id).collect();
    let right_ids: Vec<_> = right.iter().map(|e| e.id).collect();

    assert_eq!(
        left.len() + right.len(),
        total as usize,
        "every event must be claimed exactly once"
    );
    for id in &left_ids {
        assert!(
            !right_ids.contains(id),
            "event {id} was claimed by both workers"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. At-least-once: a crashed worker's lease expires
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_expired_lease_makes_an_event_claimable_again() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;

    // Worker A claims with a 1-second lease, then "crashes": it never marks the
    // event delivered and never releases the lease.
    let claimed = h
        .outbox
        .claim_batch("crashed-worker", Duration::from_secs(1), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, event_id);
    assert_eq!(claimed[0].attempts, 1, "claiming counts as an attempt");

    // While the lease is live, nobody else may take it.
    let during = h
        .outbox
        .claim_batch("worker-b", Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert!(during.is_empty(), "a live lease must exclude other workers");

    // After it expires, the event is redelivered rather than lost. This is the
    // property that makes the queue at-least-once instead of at-most-once.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let after = h
        .outbox
        .claim_batch("worker-b", Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(after.len(), 1, "an expired lease must be reclaimable");
    assert_eq!(after[0].id, event_id);
    assert_eq!(after[0].attempts, 2, "the redelivery is the second attempt");

    h.outbox.mark_delivered(event_id).await.unwrap();
    let row = h.outbox_row(event_id).await;
    assert_eq!(row.state.as_str(), "delivered");
}

#[tokio::test]
async fn a_backoff_delay_holds_an_event_back_until_it_is_due() {
    let h = Harness::new().await;
    h.insert_pending_event("transfer.completed").await;

    let w = worker(
        &h,
        Arc::new(FlakyPublisher::new(1)),
        WorkerConfig {
            backoff_base: Duration::from_secs(30),
            backoff_max: Duration::from_secs(30),
            ..fast_config("w1", 5)
        },
    );

    let report = w.run_once().await.unwrap();
    assert_eq!(report.retried, 1);

    // Backed off: claimable, but not due.
    let now = chrono::Utc::now();
    let row = h
        .outbox
        .claim_batch("w1", Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert!(
        row.is_empty(),
        "an event inside its backoff window must not be claimable"
    );
    assert!(now < chrono::Utc::now());
}

// ---------------------------------------------------------------------------
// 4. Consumer-side deduplication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_redelivered_event_is_deduplicated_by_the_consumer() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;
    let consumer_name = format!("consumer-{}", uuid::Uuid::new_v4());

    let consumer = DedupeConsumer::new(h.outbox.clone(), consumer_name.clone());

    let claimed = h
        .outbox
        .claim_batch("w1", Duration::from_secs(30), 10)
        .await
        .unwrap();
    let event = claimed.into_iter().next().unwrap();

    // First delivery: the side effect runs.
    assert!(
        !consumer.consume(&event).await.unwrap(),
        "first is not a duplicate"
    );
    assert_eq!(h.inbox_count(&consumer_name, event_id).await, 1);

    // Redelivery of the same event, exactly as an expired lease would produce.
    assert!(
        consumer.consume(&event).await.unwrap(),
        "second must be a duplicate"
    );
    assert_eq!(
        h.inbox_count(&consumer_name, event_id).await,
        1,
        "at-least-once plus dedupe must yield exactly-once side effects"
    );
    assert!(
        h.outbox
            .has_consumed(&consumer_name, event_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn dedupe_is_scoped_per_consumer() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;

    let claimed = h
        .outbox
        .claim_batch("w1", Duration::from_secs(30), 10)
        .await
        .unwrap();
    let event = claimed.into_iter().next().unwrap();

    let a = DedupeConsumer::new(h.outbox.clone(), "consumer-a");
    let b = DedupeConsumer::new(h.outbox.clone(), "consumer-b");

    // A second, independent consumer has not seen the event, so suppressing its
    // side effect would be a data-loss bug rather than safe deduplication.
    assert!(!a.consume(&event).await.unwrap());
    assert!(!b.consume(&event).await.unwrap());

    assert_eq!(h.inbox_count("consumer-a", event_id).await, 1);
    assert_eq!(h.inbox_count("consumer-b", event_id).await, 1);
    assert!(
        a.consume(&event).await.unwrap() && b.consume(&event).await.unwrap(),
        "each consumer deduplicates only against its own history"
    );
}

// ---------------------------------------------------------------------------
// 5. Retry and dead-lettering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_transient_failure_is_retried_and_then_succeeds() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;
    let publisher = Arc::new(FlakyPublisher::new(2));

    let w = worker(&h, publisher.clone(), fast_config("w1", 5));

    let first = w.run_once().await.unwrap();
    assert_eq!(first.retried, 1);
    assert_eq!(first.dead, 0);

    let second = w.run_once().await.unwrap();
    assert_eq!(second.retried, 1);

    let third = w.run_once().await.unwrap();
    assert_eq!(third.delivered, 1, "the third attempt should succeed");

    assert_eq!(publisher.failures(), 2);
    let row = h.outbox_row(event_id).await;
    assert_eq!(row.state.as_str(), "delivered");
    assert_eq!(row.attempts, 3);
    assert!(
        row.last_error.is_none(),
        "a delivered event must not keep a stale failure reason"
    );
}

#[tokio::test]
async fn a_retry_records_why_the_last_attempt_failed() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;
    let w = worker(&h, Arc::new(AlwaysFailPublisher), fast_config("w1", 5));

    w.run_once().await.unwrap();

    let row = h.outbox_row(event_id).await;
    assert_eq!(row.state.as_str(), "pending");
    assert_eq!(
        row.last_error.as_deref(),
        Some("permanent: injected poison event"),
        "a retrying event must stay diagnosable (migration 0002)"
    );
    assert!(
        row.locked_by.is_none(),
        "a retrying event releases its lease"
    );
}

#[tokio::test]
async fn an_event_that_exhausts_its_attempts_is_dead_lettered() {
    let h = Harness::new().await;
    let event_id = h.insert_pending_event("transfer.completed").await;
    let w = worker(&h, Arc::new(AlwaysFailPublisher), fast_config("w1", 3));

    for expected_attempt in 1..=3 {
        let report = w.run_once().await.unwrap();
        assert_eq!(report.claimed, 1);
        assert_eq!(report.dead, if expected_attempt == 3 { 1 } else { 0 });
    }

    let row = h.outbox_row(event_id).await;
    assert_eq!(row.state.as_str(), "dead");
    assert_eq!(row.attempts, 3);
    assert!(
        row.last_error.is_some(),
        "INV: a dead event must carry the error that killed it"
    );
    assert!(
        row.delivered_at.is_none(),
        "a dead event was never delivered"
    );
    assert!(row.locked_by.is_none());

    // Terminal: a dead event is never picked up again, so a poison event cannot
    // consume a worker slot forever.
    let after = w.run_once().await.unwrap();
    assert_eq!(after.claimed, 0);
}

#[tokio::test]
async fn one_poison_event_does_not_stall_the_rest_of_the_batch() {
    let h = Harness::new().await;
    let poison_a = h.insert_pending_event("poison").await;
    let poison_b = h.insert_pending_event("poison").await;
    let good_a = h.insert_pending_event("transfer.completed").await;
    let good_b = h.insert_pending_event("transfer.completed").await;

    // Fails only the poison events. If the worker aborted a batch on the first
    // failure, the good events would be stranded behind a lease until it
    // expired.
    let w = worker(
        &h,
        Arc::new(SelectiveFailingPublisher::new("poison")),
        fast_config("w1", 3),
    );
    let report = w.run_once().await.unwrap();

    assert_eq!(report.claimed, 4);
    assert_eq!(report.retried, 2, "only the poison events fail");
    assert_eq!(report.dead, 0);
    assert_eq!(report.delivered, 2, "the healthy events must still go out");

    assert_eq!(h.outbox_row(good_a).await.state.as_str(), "delivered");
    assert_eq!(h.outbox_row(good_b).await.state.as_str(), "delivered");
    assert_eq!(h.outbox_row(poison_a).await.state.as_str(), "pending");
    assert_eq!(h.outbox_row(poison_b).await.state.as_str(), "pending");
}

// ---------------------------------------------------------------------------
// 6. Batch behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_is_processed_in_one_pass() {
    let h = Harness::new().await;
    let count = 40;
    for _ in 0..count {
        h.insert_pending_event("transfer.completed").await;
    }

    let w = worker(&h, Arc::new(OkPublisher), fast_config("w1", 3));
    let report = w.run_once().await.unwrap();

    assert_eq!(report.claimed, count);
    assert_eq!(report.delivered, count);

    let counts = h.outbox.count_by_state().await.unwrap();
    let delivered: i64 = counts
        .iter()
        .filter(|(s, _)| s.as_str() == "delivered")
        .map(|(_, c)| *c)
        .sum();
    assert!(delivered >= count as i64);
}

#[tokio::test]
async fn an_empty_queue_is_not_an_error() {
    let h = Harness::new().await;
    let w = worker(&h, Arc::new(OkPublisher), fast_config("w1", 3));
    let report = w.run_once().await.unwrap();
    assert_eq!(report.claimed, 0);
    assert_eq!(report.delivered, 0);
}

#[tokio::test]
async fn the_loop_stops_when_shutdown_resolves() {
    let h = Harness::new().await;
    h.insert_pending_event("transfer.completed").await;
    let w = worker(&h, Arc::new(OkPublisher), fast_config("w1", 3));

    // Shut down immediately; the loop must exit promptly rather than spinning.
    let result = tokio::time::timeout(Duration::from_secs(5), w.run_until(async {}))
        .await
        .expect("worker must stop promptly on shutdown");

    assert!(result.is_ok());
}
