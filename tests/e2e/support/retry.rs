//! Retry and synchronization utilities for E2E tests
//!
//! Provides robust retry logic with exponential backoff for handling
//! timing issues between Control Plane and Envoy xDS convergence.

use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Retry configuration with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries (for exponential backoff)
    pub max_delay: Duration,
    /// Multiplier for exponential backoff (e.g., 2.0 doubles each time)
    pub backoff_multiplier: f64,
    /// Description for logging purposes
    pub description: String,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 60,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 1.5,
            description: "operation".to_string(),
        }
    }
}

impl RetryConfig {
    /// Create a fast retry config for quick operations (e.g., HTTP health checks)
    pub fn fast() -> Self {
        Self {
            max_attempts: 100,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(2),
            backoff_multiplier: 1.2,
            description: "fast operation".to_string(),
        }
    }

    /// Create a slow retry config for xDS convergence (needs more time)
    #[allow(dead_code)]
    pub fn xds_convergence() -> Self {
        Self {
            max_attempts: 60,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 1.5,
            description: "xDS convergence".to_string(),
        }
    }

    /// Create a custom retry config with a description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Retry a future until it succeeds or max attempts is reached
///
/// # Arguments
/// * `config` - Retry configuration
/// * `f` - Async function that returns Result<T, E>
///
/// # Returns
/// Ok(T) if successful within max_attempts, Err(E) with last error otherwise
pub async fn retry_with_backoff<F, Fut, T, E>(config: RetryConfig, f: F) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut delay = config.initial_delay;
    let mut last_error: Option<E> = None;

    for attempt in 1..=config.max_attempts {
        match f().await {
            Ok(value) => {
                if attempt > 1 {
                    debug!(
                        attempt,
                        description = %config.description,
                        "Retry succeeded"
                    );
                }
                return Ok(value);
            }
            Err(e) => {
                if attempt < config.max_attempts {
                    debug!(
                        attempt,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis(),
                        error = %e,
                        description = %config.description,
                        "Retry attempt failed, will retry"
                    );
                    sleep(delay).await;
                    // Exponential backoff with max cap
                    delay = Duration::from_millis(
                        ((delay.as_millis() as f64) * config.backoff_multiplier) as u64,
                    )
                    .min(config.max_delay);
                } else {
                    warn!(
                        attempt,
                        max_attempts = config.max_attempts,
                        error = %e,
                        description = %config.description,
                        "Retry exhausted all attempts"
                    );
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap())
}

/// Retry a future until the predicate returns true
///
/// # Arguments
/// * `config` - Retry configuration
/// * `f` - Async function that returns T
/// * `predicate` - Function that checks if T is acceptable
///
/// # Returns
/// Ok(T) if predicate succeeds within max_attempts, Err with message otherwise
pub async fn retry_until<F, Fut, T, P>(
    config: RetryConfig,
    mut f: F,
    mut predicate: P,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = T>,
    P: FnMut(&T) -> bool,
{
    let mut delay = config.initial_delay;

    for attempt in 1..=config.max_attempts {
        let value = f().await;
        if predicate(&value) {
            if attempt > 1 {
                debug!(
                    attempt,
                    description = %config.description,
                    "Retry until predicate succeeded"
                );
            }
            return Ok(value);
        }

        if attempt < config.max_attempts {
            debug!(
                attempt,
                max_attempts = config.max_attempts,
                delay_ms = delay.as_millis(),
                description = %config.description,
                "Predicate not satisfied, will retry"
            );
            sleep(delay).await;
            delay = Duration::from_millis(
                ((delay.as_millis() as f64) * config.backoff_multiplier) as u64,
            )
            .min(config.max_delay);
        }
    }

    anyhow::bail!("{} failed after {} attempts", config.description, config.max_attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let config = RetryConfig::default();
        let result = retry_with_backoff(config, || async { Ok::<_, String>(42) }).await;
        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn test_retry_success_after_retries() {
        let config = RetryConfig::fast();
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        let result = retry_with_backoff(config, move || {
            let attempt = attempt_clone.clone();
            async move {
                let current = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if current < 3 {
                    Err("not yet".to_string())
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(attempt.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_until_predicate() {
        let config = RetryConfig::fast();
        let value = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let value_clone = value.clone();
        let result = retry_until(
            config,
            || {
                let v = value_clone.clone();
                async move { v.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 }
            },
            |v| *v >= 5,
        )
        .await;
        assert_eq!(result.unwrap(), 5);
    }
}
