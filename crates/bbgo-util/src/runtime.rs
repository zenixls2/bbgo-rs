//! Tokio cancellation-aware orchestration. Retried operations must be drop-safe
//! and idempotent; retrying order submission blindly is not safe.
use crate::Backoff;
use std::{future::Future, num::NonZeroU32, time::Duration};
use tokio::{sync::Mutex, time::Instant};
pub use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq, Eq)]
pub enum RetryFailure<E> {
    Retry(E),
    Permanent(E),
}
#[derive(Debug, PartialEq, Eq)]
pub enum RetryError<E> {
    Cancelled,
    Deadline,
    InvalidDeadline,
    Failed(E),
}
pub async fn retry<T, E, F, Fut>(
    attempts: NonZeroU32,
    budget: Duration,
    mut backoff: Backoff,
    cancel: &CancellationToken,
    mut operation: F,
) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RetryFailure<E>>>,
{
    let deadline = Instant::now()
        .checked_add(budget)
        .ok_or(RetryError::InvalidDeadline)?;
    if budget.is_zero() {
        return Err(RetryError::Deadline);
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(RetryError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(RetryError::Deadline),
        result = async {
            for attempt in 1..=attempts.get() {
                match operation().await {
                    Ok(value) => return Ok(value),
                    Err(RetryFailure::Permanent(error)) => return Err(RetryError::Failed(error)),
                    Err(RetryFailure::Retry(error)) if attempt == attempts.get() => return Err(RetryError::Failed(error)),
                    Err(RetryFailure::Retry(_)) => tokio::time::sleep(backoff.next_delay()).await,
                }
            }
            unreachable!("NonZeroU32 guarantees an attempt")
        } => result,
    }
}

/// A shared no-burst pacing limiter. Cancelled waiters consume no future slots.
pub struct RateLimiter {
    period: Duration,
    next: Mutex<Instant>,
}
impl RateLimiter {
    pub fn new(period: Duration) -> Result<Self, &'static str> {
        if period.is_zero() || Instant::now().checked_add(period).is_none() {
            return Err("invalid pacing period");
        }
        Ok(Self {
            period,
            next: Mutex::new(Instant::now()),
        })
    }
    pub async fn acquire(&self, cancel: &CancellationToken) -> Result<(), &'static str> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err("cancelled"),
            result = async {
                let mut next = self.next.lock().await;
                tokio::time::sleep_until(*next).await;
                *next = Instant::now().checked_add(self.period).ok_or("pacing clock overflow")?;
                Ok(())
            } => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn retry_limits_permanent_failure_and_deadline() {
        let token = CancellationToken::new();
        let backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(4)).unwrap();
        let mut calls = 0;
        let start = Instant::now();
        let result = retry(
            NonZeroU32::new(3).unwrap(),
            Duration::from_secs(20),
            backoff.clone(),
            &token,
            || {
                calls += 1;
                async { Err::<(), _>(RetryFailure::Retry("offline")) }
            },
        )
        .await;
        assert_eq!(result, Err(RetryError::Failed("offline")));
        assert_eq!(calls, 3);
        assert_eq!(Instant::now() - start, Duration::from_secs(3));
        assert_eq!(
            retry(
                NonZeroU32::new(3).unwrap(),
                Duration::from_secs(1),
                backoff.clone(),
                &token,
                || async { std::future::pending::<Result<(), RetryFailure<()>>>().await }
            )
            .await,
            Err(RetryError::Deadline)
        );
        assert_eq!(
            retry(
                NonZeroU32::new(3).unwrap(),
                Duration::from_secs(1),
                backoff,
                &token,
                || async { Err::<(), _>(RetryFailure::Permanent(42)) }
            )
            .await,
            Err(RetryError::Failed(42))
        );
    }
    #[tokio::test(start_paused = true)]
    async fn cancellation_interrupts_inflight_work_and_pacing() {
        let cancel = CancellationToken::new();
        let limiter = RateLimiter::new(Duration::from_secs(10)).unwrap();
        limiter.acquire(&cancel).await.unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(limiter.acquire(&cancelled).await.is_err());
        let start = Instant::now();
        limiter.acquire(&cancel).await.unwrap();
        assert_eq!(Instant::now() - start, Duration::from_secs(10));
        let result = retry(
            NonZeroU32::new(1).unwrap(),
            Duration::from_secs(100),
            Backoff::new(Duration::from_secs(1), Duration::from_secs(1)).unwrap(),
            &cancelled,
            || async {
                panic!("must not run");
                #[allow(unreachable_code)]
                Ok::<(), RetryFailure<()>>(())
            },
        )
        .await;
        assert_eq!(result, Err(RetryError::Cancelled));
        let child = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            child.cancel();
        });
        assert_eq!(
            retry(
                NonZeroU32::new(1).unwrap(),
                Duration::from_secs(100),
                Backoff::new(Duration::from_secs(1), Duration::from_secs(1)).unwrap(),
                &cancel,
                std::future::pending::<Result<(), RetryFailure<()>>>
            )
            .await,
            Err(RetryError::Cancelled)
        );
    }
}
