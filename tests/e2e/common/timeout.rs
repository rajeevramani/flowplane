//! Hard timeout utilities for E2E tests
//!
//! Every async operation in E2E tests MUST have a hard timeout.
//! Default maximum is 30 seconds - no test should ever hang.

use std::future::Future;
use std::time::Duration;
use tokio::time::timeout;

/// Default timeout for most operations (30 seconds)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Quick timeout for simple operations (5 seconds)
pub const QUICK_TIMEOUT: Duration = Duration::from_secs(5);

/// Extended timeout for slow operations like initial startup (60 seconds)
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout configuration for tests
#[derive(Debug, Clone)]
pub struct TestTimeout {
    /// Duration before timeout triggers
    pub duration: Duration,
    /// Human-readable label for error messages
    pub label: String,
}

impl TestTimeout {
    /// Create a new timeout with custom duration and label
    pub fn new(duration: Duration, label: impl Into<String>) -> Self {
        Self { duration, label: label.into() }
    }

    /// Default timeout (30s)
    pub fn default_with_label(label: impl Into<String>) -> Self {
        Self::new(DEFAULT_TIMEOUT, label)
    }

    /// Quick timeout (5s) for simple operations
    pub fn quick(label: impl Into<String>) -> Self {
        Self::new(QUICK_TIMEOUT, label)
    }

    /// Startup timeout (60s) for initial component startup
    pub fn startup(label: impl Into<String>) -> Self {
        Self::new(STARTUP_TIMEOUT, label)
    }
}

/// Wrap an async operation with a hard timeout
///
/// # Arguments
/// * `timeout_cfg` - Timeout configuration with duration and label
/// * `fut` - The future to execute
///
/// # Returns
/// `Ok(T)` if the future completes within the timeout, `Err` with descriptive message otherwise
///
/// # Example
/// ```ignore
/// let result = with_timeout(
///     TestTimeout::default_with_label("creating cluster"),
///     async { api.create_cluster(req).await }
/// ).await?;
/// ```
pub async fn with_timeout<F, T>(timeout_cfg: TestTimeout, fut: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    match timeout(timeout_cfg.duration, fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            anyhow::bail!(
                "TIMEOUT: '{}' exceeded {:?} - test is likely stuck",
                timeout_cfg.label,
                timeout_cfg.duration
            )
        }
    }
}

/// Wrap an async operation with a hard timeout (simplified version)
///
/// # Arguments
/// * `duration` - Maximum time to wait
/// * `label` - Human-readable label for error messages
/// * `fut` - The future to execute
pub async fn with_timeout_simple<F, T>(duration: Duration, label: &str, fut: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    with_timeout(TestTimeout::new(duration, label), fut).await
}

/// Retry an operation with a total timeout budget
///
/// # Arguments
/// * `total_timeout` - Maximum total time for all retries
/// * `interval` - Time between retries
/// * `label` - Human-readable label for error messages
/// * `f` - Async function that returns Result<T, E>
///
/// # Returns
/// First successful result, or error if timeout exceeded
pub async fn retry_with_timeout<F, Fut, T, E>(
    total_timeout: Duration,
    interval: Duration,
    label: &str,
    f: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let start = std::time::Instant::now();
    let mut last_error: Option<String> = None;
    let mut attempt = 0;

    loop {
        attempt += 1;
        let elapsed = start.elapsed();

        if elapsed >= total_timeout {
            let err_msg = last_error.map(|e| format!(": {}", e)).unwrap_or_default();
            anyhow::bail!(
                "TIMEOUT: '{}' exceeded {:?} after {} attempts{}",
                label,
                total_timeout,
                attempt - 1,
                err_msg
            );
        }

        match f().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_error = Some(e.to_string());
                let remaining = total_timeout.saturating_sub(elapsed);
                let sleep_time = interval.min(remaining);
                if sleep_time > Duration::ZERO {
                    tokio::time::sleep(sleep_time).await;
                }
            }
        }
    }
}

/// Wait for a condition to become true with timeout
///
/// # Arguments
/// * `total_timeout` - Maximum time to wait
/// * `check_interval` - Time between checks
/// * `label` - Human-readable label for error messages
/// * `condition` - Async function that returns bool
pub async fn wait_for_condition<F, Fut>(
    total_timeout: Duration,
    check_interval: Duration,
    label: &str,
    condition: F,
) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let start = std::time::Instant::now();

    loop {
        let elapsed = start.elapsed();

        if elapsed >= total_timeout {
            anyhow::bail!("TIMEOUT: '{}' condition not met within {:?}", label, total_timeout);
        }

        if condition().await {
            return Ok(());
        }

        let remaining = total_timeout.saturating_sub(elapsed);
        let sleep_time = check_interval.min(remaining);
        if sleep_time > Duration::ZERO {
            tokio::time::sleep(sleep_time).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_with_timeout_success() {
        let result =
            with_timeout(TestTimeout::quick("test op"), async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_timeout_exceeds() {
        let result = with_timeout(TestTimeout::new(Duration::from_millis(10), "slow op"), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, anyhow::Error>(42)
        })
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("TIMEOUT"));
        assert!(err.contains("slow op"));
    }

    #[tokio::test]
    async fn test_retry_with_timeout_success() {
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();

        let result = retry_with_timeout(
            Duration::from_secs(5),
            Duration::from_millis(10),
            "retry test",
            move || {
                let a = attempt_clone.clone();
                async move {
                    let n = a.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if n < 3 {
                        Err::<i32, _>("not yet")
                    } else {
                        Ok(42)
                    }
                }
            },
        )
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_wait_for_condition_success() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = wait_for_condition(
            Duration::from_secs(5),
            Duration::from_millis(10),
            "condition test",
            move || {
                let c = counter_clone.clone();
                async move {
                    let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    n >= 3
                }
            },
        )
        .await;

        assert!(result.is_ok());
    }
}
