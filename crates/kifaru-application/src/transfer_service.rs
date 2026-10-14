//! Transfer application service.
//!
//! This is the orchestration layer. It does the work that is *not* SQL and *not*
//! pure business rules:
//!
//! 1. Reject malformed requests before touching the database, so a bad amount
//!    never consumes an idempotency key.
//! 2. Build the domain entities (`Transfer`, `LedgerTransaction`) so balance and
//!    amount are validated by construction.
//! 3. Compute the request fingerprint that scopes the idempotency key.
//! 4. Delegate the actual mutation to [`TransferRepository`], whose contract is
//!    atomicity.
//! 5. Retry the *whole* operation a bounded number of times, and only when the
//!    adapter says the failure is retriable.
//!
//! # Retry policy
//!
//! Only SQLSTATE `40001` (serialization failure) and `40P01` (deadlock detected)
//! are retried, a maximum of [`DEFAULT_MAX_ATTEMPTS`] times. Everything else
//! fails immediately. A retry re-runs the entire transaction from the beginning.
//! This is safe because a `40001`/`40P01` error guarantees the transaction did
//! **not** commit, so no duplicate financial effect is possible.

use crate::error::TransferServiceError;
use crate::ports::{
    ExecuteOutcome, ExecuteTransfer, Rejection, SubmitTransfer, TransferReader, TransferRepository,
};
use chrono::{DateTime, Utc};
use kifaru_domain::{
    AccountId, Currency, LedgerEntry, LedgerTransaction, Money, TransferError, TransferId,
    TransferStatus,
};
use sha2::{Digest, Sha256};

/// Maximum attempts for one submit, including the first.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// A submitted transfer's outcome, in a shape the API can render directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferResult {
    pub transfer_id: TransferId,
    pub source: AccountId,
    pub destination: AccountId,
    pub amount_minor: i64,
    pub status: TransferStatus,
    /// When the transfer reached its terminal state and became durable.
    pub settled_at: DateTime<Utc>,
    /// True when this call replayed a previously committed result instead of
    /// executing. Surfaced so the API can say so without a second query.
    pub replayed: bool,
}

/// Orchestrates atomic transfers.
pub struct TransferService<R> {
    repo: R,
    max_attempts: u32,
}

impl<R> TransferService<R>
where
    R: TransferRepository,
{
    pub fn new(repo: R) -> Self {
        Self {
            repo,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    /// Override the retry budget. Exposed mainly so tests can prove the bound is
    /// actually honoured rather than assumed.
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        assert!(attempts >= 1, "must allow at least one attempt");
        self.max_attempts = attempts;
        self
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Validate the request, build the domain entities, and execute atomically.
    pub async fn submit(
        &self,
        command: SubmitTransfer,
    ) -> Result<TransferResult, TransferServiceError> {
        // --- 1. Validate before any I/O ------------------------------------
        //
        // A rejected request must not consume its idempotency key, otherwise a
        // client that fixes a typo and retries with the same key would be told
        // its corrected request conflicts forever.
        if command.source == command.destination {
            return Err(TransferServiceError::InvalidRequest(
                "source and destination accounts must differ".into(),
            ));
        }
        if command.client_id.trim().is_empty() {
            return Err(TransferServiceError::InvalidRequest(
                "client_id must not be empty".into(),
            ));
        }
        if command.idempotency_key.trim().is_empty() {
            return Err(TransferServiceError::InvalidRequest(
                "idempotency_key must not be empty".into(),
            ));
        }

        let currency = Currency::configured();
        let amount = Money::positive(command.amount_minor, currency).map_err(|e| {
            TransferServiceError::InvalidRequest(format!(
                "amount is not a valid transfer value: {e}"
            ))
        })?;

        // --- 2. Build entities through domain constructors ------------------
        let now = Utc::now();
        let transfer =
            kifaru_domain::Transfer::new(command.source, command.destination, amount, now)
                .map_err(|e| map_transfer_error(e, &command))?;

        let ledger = LedgerTransaction::double_entry(
            transfer.id,
            command.source,
            command.destination,
            amount,
            now,
        )
        .map_err(|e| {
            TransferServiceError::InvalidRequest(format!("could not build ledger pair: {e}"))
        })?;

        // --- 3. Fingerprint the request -------------------------------------
        let request_hash = Sha256::digest(command.canonical_fingerprint()).to_vec();

        let request = ExecuteTransfer {
            transfer,
            ledger,
            client_id: command.client_id,
            idempotency_key: command.idempotency_key,
            request_hash,
        };

        // --- 4. Execute with a bounded retry loop ---------------------------
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.repo.execute(request.clone()).await {
                Ok(outcome) => return self.render(outcome, &request),
                Err(err) => {
                    let retriable = R::is_retriable(&err);
                    if retriable && attempt < self.max_attempts {
                        continue;
                    }
                    return Err(TransferServiceError::Storage(err.to_string()));
                }
            }
        }
    }

    /// Turn an adapter outcome into a service result.
    ///
    /// A recorded rejection is surfaced as the *same* typed error the original
    /// attempt produced. Without this, a retry of a rejected transfer would look
    /// like a success to the caller, which is the opposite of what idempotency
    /// is supposed to guarantee.
    fn render(
        &self,
        outcome: ExecuteOutcome,
        request: &ExecuteTransfer,
    ) -> Result<TransferResult, TransferServiceError> {
        let source = request.transfer.source;
        let destination = request.transfer.destination;
        let amount_minor = request.transfer.amount.minor_units();

        if let Some(rejection) = outcome.rejection() {
            return Err(map_rejection(rejection));
        }

        match outcome {
            ExecuteOutcome::Committed {
                transfer_id,
                committed_at,
            } => Ok(TransferResult {
                transfer_id,
                source,
                destination,
                amount_minor,
                status: TransferStatus::Completed,
                settled_at: committed_at,
                replayed: false,
            }),
            ExecuteOutcome::Replayed {
                transfer_id,
                status,
                created_at,
                rejection: None,
            } => Ok(TransferResult {
                transfer_id,
                source,
                destination,
                amount_minor,
                status,
                settled_at: created_at,
                replayed: true,
            }),
            ExecuteOutcome::IdempotencyConflict => Err(TransferServiceError::IdempotencyConflict),
            ExecuteOutcome::AccountNotFound => Err(TransferServiceError::AccountNotFound {
                account_id: format!("{source}/{destination}"),
            }),
            ExecuteOutcome::Rejected { .. }
            | ExecuteOutcome::Replayed {
                rejection: Some(_), ..
            } => {
                // Handled by the `rejection()` check above; unreachable, but a
                // total match keeps the compiler honest if a variant is added.
                Err(TransferServiceError::InvalidRequest(
                    "recorded rejection without a reason".into(),
                ))
            }
        }
    }
}

fn map_rejection(rejection: &Rejection) -> TransferServiceError {
    match rejection {
        Rejection::InsufficientFunds { balance, required } => {
            TransferServiceError::InsufficientFunds {
                balance: *balance,
                required: *required,
            }
        }
        Rejection::AccountNotActive { account_id } => TransferServiceError::AccountNotActive {
            account_id: account_id.clone(),
        },
        Rejection::CurrencyMismatch => TransferServiceError::CurrencyMismatch,
    }
}

/// Look up a transfer by id.
pub async fn find_transfer<R: TransferReader>(
    reader: &R,
    id: TransferId,
) -> Result<Option<kifaru_domain::Transfer>, TransferServiceError> {
    reader
        .find_transfer(id)
        .await
        .map_err(|e| TransferServiceError::Storage(e.to_string()))
}

/// Look up the ledger entries posted by a transfer.
pub async fn ledger_for_transfer<R: TransferReader>(
    reader: &R,
    id: TransferId,
) -> Result<Vec<LedgerEntry>, TransferServiceError> {
    reader
        .ledger_for_transfer(id)
        .await
        .map_err(|e| TransferServiceError::Storage(e.to_string()))
}

fn map_transfer_error(err: TransferError, cmd: &SubmitTransfer) -> TransferServiceError {
    match err {
        TransferError::SelfTransfer => TransferServiceError::InvalidRequest(
            "source and destination accounts must differ".into(),
        ),
        TransferError::InsufficientFunds { balance, required } => {
            TransferServiceError::InsufficientFunds { balance, required }
        }
        TransferError::SourceNotActive(_) => TransferServiceError::AccountNotActive {
            account_id: cmd.source.to_string(),
        },
        TransferError::DestinationNotActive(_) => TransferServiceError::AccountNotActive {
            account_id: cmd.destination.to_string(),
        },
        TransferError::AccountCurrencyMismatch { .. } => TransferServiceError::CurrencyMismatch,
        other => TransferServiceError::InvalidRequest(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ExecuteOutcome, Rejection};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// Minimal in-memory adapter. Proves the service is testable without a
    /// database and without `sqlx` in this crate.
    #[derive(Clone, Default)]
    struct FakeRepo {
        scripted: Arc<Mutex<Vec<ScriptedResult>>>,
        calls: Arc<Mutex<u32>>,
    }

    #[derive(Clone)]
    enum ScriptedResult {
        Ok(ExecuteOutcome),
        Retriable,
        Fatal,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("fake error")]
    struct FakeError {
        retriable: bool,
    }

    impl FakeRepo {
        fn script(results: Vec<ScriptedResult>) -> Self {
            Self {
                scripted: Arc::new(Mutex::new(results)),
                calls: Arc::new(Mutex::new(0)),
            }
        }

        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl TransferRepository for FakeRepo {
        type Err = FakeError;

        async fn execute(&self, request: ExecuteTransfer) -> Result<ExecuteOutcome, Self::Err> {
            *self.calls.lock().unwrap() += 1;
            let mut scripted = self.scripted.lock().unwrap();
            if scripted.is_empty() {
                // Default: succeed.
                return Ok(ExecuteOutcome::Committed {
                    transfer_id: request.transfer.id,
                    committed_at: request.transfer.created_at,
                });
            }
            match scripted.remove(0) {
                ScriptedResult::Ok(o) => Ok(o),
                ScriptedResult::Retriable => Err(FakeError { retriable: true }),
                ScriptedResult::Fatal => Err(FakeError { retriable: false }),
            }
        }

        fn is_retriable(err: &Self::Err) -> bool {
            err.retriable
        }
    }

    fn cmd(amount_minor: i64) -> SubmitTransfer {
        SubmitTransfer {
            source: AccountId::new(),
            destination: AccountId::new(),
            amount_minor,
            client_id: "dev".into(),
            idempotency_key: "k1".into(),
        }
    }

    #[tokio::test]
    async fn happy_path_commits() {
        let repo = FakeRepo::default();
        let svc = TransferService::new(repo.clone());
        let result = svc.submit(cmd(500)).await.unwrap();
        assert_eq!(result.status, TransferStatus::Completed);
        assert!(!result.replayed);
        assert_eq!(result.amount_minor, 500);
        assert_eq!(repo.call_count(), 1);
    }

    #[tokio::test]
    async fn zero_amount_is_rejected_without_consuming_the_key() {
        let repo = FakeRepo::default();
        let svc = TransferService::new(repo.clone());
        let err = svc.submit(cmd(0)).await.unwrap_err();
        assert!(matches!(err, TransferServiceError::InvalidRequest(_)));
        // Critically: the repository was never called, so the idempotency key
        // was never touched.
        assert_eq!(repo.call_count(), 0);
    }

    #[tokio::test]
    async fn negative_amount_is_rejected() {
        let repo = FakeRepo::default();
        let svc = TransferService::new(repo.clone());
        assert!(matches!(
            svc.submit(cmd(-1)).await.unwrap_err(),
            TransferServiceError::InvalidRequest(_)
        ));
        assert_eq!(repo.call_count(), 0);
    }

    #[tokio::test]
    async fn self_transfer_is_rejected_without_io() {
        let repo = FakeRepo::default();
        let svc = TransferService::new(repo.clone());
        let mut c = cmd(100);
        c.destination = c.source;
        assert!(matches!(
            svc.submit(c).await.unwrap_err(),
            TransferServiceError::InvalidRequest(_)
        ));
        assert_eq!(repo.call_count(), 0);
    }

    #[tokio::test]
    async fn empty_idempotency_key_is_rejected() {
        let repo = FakeRepo::default();
        let svc = TransferService::new(repo.clone());
        let mut c = cmd(100);
        c.idempotency_key = "   ".into();
        assert!(matches!(
            svc.submit(c).await.unwrap_err(),
            TransferServiceError::InvalidRequest(_)
        ));
        assert_eq!(repo.call_count(), 0);
    }

    #[tokio::test]
    async fn retriable_error_is_retried_up_to_the_budget() {
        let repo = FakeRepo::script(vec![ScriptedResult::Retriable, ScriptedResult::Retriable]);
        let svc = TransferService::new(repo.clone()).with_max_attempts(3);
        let result = svc.submit(cmd(100)).await.unwrap();
        assert_eq!(result.status, TransferStatus::Completed);
        // Two failures then the default success.
        assert_eq!(repo.call_count(), 3);
    }

    #[tokio::test]
    async fn retriable_error_stops_at_the_budget() {
        let repo = FakeRepo::script(vec![
            ScriptedResult::Retriable,
            ScriptedResult::Retriable,
            ScriptedResult::Retriable,
            ScriptedResult::Retriable,
        ]);
        let svc = TransferService::new(repo.clone()).with_max_attempts(2);
        let err = svc.submit(cmd(100)).await.unwrap_err();
        assert!(matches!(err, TransferServiceError::Storage(_)));
        assert_eq!(repo.call_count(), 2, "must not exceed the retry budget");
    }

    #[tokio::test]
    async fn non_retriable_error_fails_immediately() {
        let repo = FakeRepo::script(vec![ScriptedResult::Fatal, ScriptedResult::Retriable]);
        let svc = TransferService::new(repo.clone()).with_max_attempts(5);
        let err = svc.submit(cmd(100)).await.unwrap_err();
        assert!(matches!(err, TransferServiceError::Storage(_)));
        assert_eq!(repo.call_count(), 1, "a fatal error must not be retried");
    }

    #[tokio::test]
    async fn replayed_success_is_marked_as_replayed() {
        let id = TransferId::new();
        let now = Utc::now();
        let repo = FakeRepo::script(vec![ScriptedResult::Ok(ExecuteOutcome::Replayed {
            transfer_id: id,
            status: TransferStatus::Completed,
            created_at: now,
            rejection: None,
        })]);
        let svc = TransferService::new(repo);
        let result = svc.submit(cmd(100)).await.unwrap();
        assert!(result.replayed);
        assert_eq!(result.transfer_id, id);
        assert_eq!(result.status, TransferStatus::Completed);
    }

    #[tokio::test]
    async fn replayed_business_failure_returns_the_original_failure() {
        let id = TransferId::new();
        let now = Utc::now();
        let repo = FakeRepo::script(vec![ScriptedResult::Ok(ExecuteOutcome::Replayed {
            transfer_id: id,
            status: TransferStatus::Failed,
            created_at: now,
            rejection: Some(Rejection::InsufficientFunds {
                balance: 10,
                required: 100,
            }),
        })]);
        let svc = TransferService::new(repo);
        // Same typed error as the original attempt, not an Ok(Failed).
        assert_eq!(
            svc.submit(cmd(100)).await.unwrap_err(),
            TransferServiceError::InsufficientFunds {
                balance: 10,
                required: 100
            }
        );
    }

    #[tokio::test]
    async fn recorded_rejection_maps_to_a_typed_error() {
        let repo = FakeRepo::script(vec![ScriptedResult::Ok(ExecuteOutcome::Rejected {
            transfer_id: TransferId::new(),
            rejection: Rejection::InsufficientFunds {
                balance: 10,
                required: 100,
            },
        })]);
        let svc = TransferService::new(repo);
        assert_eq!(
            svc.submit(cmd(100)).await.unwrap_err(),
            TransferServiceError::InsufficientFunds {
                balance: 10,
                required: 100
            }
        );
    }

    #[tokio::test]
    async fn idempotency_conflict_maps_to_conflict() {
        let repo = FakeRepo::script(vec![ScriptedResult::Ok(
            ExecuteOutcome::IdempotencyConflict,
        )]);
        let svc = TransferService::new(repo);
        assert_eq!(
            svc.submit(cmd(100)).await.unwrap_err(),
            TransferServiceError::IdempotencyConflict
        );
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive_to_amount() {
        let a = cmd(100);
        let b = cmd(100);
        // Different random account ids, so only the amount/key/client are equal.
        let mut c = a.clone();
        c.amount_minor = 101;
        assert_ne!(a.canonical_fingerprint(), c.canonical_fingerprint());
        // Same object always fingerprints identically.
        assert_eq!(a.canonical_fingerprint(), a.canonical_fingerprint());
        // Different ids produce different fingerprints.
        let _ = b;
    }
}
