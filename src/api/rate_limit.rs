//! Rate limiting for API endpoints.
//!
//! Provides token bucket rate limiting to prevent abuse and protect
//! backend resources like Vault PKI.
//!
//! # Configuration
//!
//! - `FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR`: Max certificates per hour per team (default: 100)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Token bucket for rate limiting.
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Current number of tokens available
    tokens: f64,
    /// Maximum tokens in the bucket
    max_tokens: f64,
    /// Time of last token refill
    last_refill: Instant,
    /// Token refill rate (tokens per second)
    refill_rate_per_sec: f64,
}

impl TokenBucket {
    fn new(max_tokens: u32, refill_period: Duration) -> Self {
        let refill_rate_per_sec = max_tokens as f64 / refill_period.as_secs_f64();
        Self {
            tokens: max_tokens as f64,
            max_tokens: max_tokens as f64,
            last_refill: Instant::now(),
            refill_rate_per_sec,
        }
    }

    /// Try to consume a token from the bucket.
    ///
    /// Returns `Ok(())` if successful, `Err(retry_after_secs)` if rate limited.
    fn try_consume(&mut self) -> Result<(), u32> {
        // Refill tokens based on elapsed time
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate_per_sec).min(self.max_tokens);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            // Calculate seconds until next token available
            let seconds_until_refill = (1.0 - self.tokens) / self.refill_rate_per_sec;
            Err(seconds_until_refill.ceil() as u32)
        }
    }
}

/// Rate limiter using token bucket algorithm.
///
/// Each key (typically team name) has its own token bucket with
/// configurable capacity and refill rate.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    max_tokens: u32,
    refill_period: Duration,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    /// - `max_tokens`: Maximum requests allowed in the refill period
    /// - `refill_period`: Time period for full token bucket refill
    pub fn new(max_tokens: u32, refill_period: Duration) -> Self {
        Self { buckets: Arc::new(Mutex::new(HashMap::new())), max_tokens, refill_period }
    }

    /// Create rate limiter from environment variables.
    ///
    /// - `FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR`: Max certificates per hour (default: 100)
    pub fn from_env() -> Self {
        let max_tokens = std::env::var("FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(100);

        Self::new(max_tokens, Duration::from_secs(3600))
    }

    /// Check if request is allowed under rate limit.
    ///
    /// # Arguments
    /// - `key`: Rate limit key (e.g., team name)
    ///
    /// # Returns
    /// - `Ok(())` if request allowed
    /// - `Err(retry_after_secs)` if rate limited
    pub async fn check_rate_limit(&self, key: &str) -> Result<(), u32> {
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(self.max_tokens, self.refill_period));

        match bucket.try_consume() {
            Ok(()) => {
                debug!(
                    key = %key,
                    remaining_tokens = bucket.tokens as u32,
                    "Rate limit check passed"
                );
                Ok(())
            }
            Err(retry_after) => {
                warn!(
                    key = %key,
                    retry_after_seconds = retry_after,
                    "Rate limit exceeded"
                );
                Err(retry_after)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(5, Duration::from_secs(3600));

        // First 5 requests should succeed
        for i in 0..5 {
            assert!(
                limiter.check_rate_limit("team-a").await.is_ok(),
                "Request {} should succeed",
                i + 1
            );
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(3600));

        // Consume all tokens
        for _ in 0..3 {
            limiter.check_rate_limit("team-b").await.unwrap();
        }

        // Next request should fail
        let result = limiter.check_rate_limit("team-b").await;
        assert!(result.is_err(), "4th request should be rate limited");
    }

    #[tokio::test]
    async fn test_rate_limiter_isolates_teams() {
        let limiter = RateLimiter::new(2, Duration::from_secs(3600));

        // Team A consumes all tokens
        limiter.check_rate_limit("team-a").await.unwrap();
        limiter.check_rate_limit("team-a").await.unwrap();
        assert!(limiter.check_rate_limit("team-a").await.is_err(), "Team A should be rate limited");

        // Team B should still have tokens
        assert!(
            limiter.check_rate_limit("team-b").await.is_ok(),
            "Team B should not be rate limited"
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_refills_over_time() {
        // 2 tokens per second (for fast testing)
        let limiter = RateLimiter::new(2, Duration::from_secs(1));

        // Consume all tokens
        limiter.check_rate_limit("team-c").await.unwrap();
        limiter.check_rate_limit("team-c").await.unwrap();
        assert!(limiter.check_rate_limit("team-c").await.is_err());

        // Wait for refill
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Should have tokens again
        assert!(limiter.check_rate_limit("team-c").await.is_ok(), "Tokens should have refilled");
    }

    #[tokio::test]
    async fn test_rate_limiter_returns_retry_after() {
        let limiter = RateLimiter::new(1, Duration::from_secs(3600)); // 1 per hour

        // Consume the token
        limiter.check_rate_limit("team-d").await.unwrap();

        // Next request should return retry_after
        let result = limiter.check_rate_limit("team-d").await;
        assert!(result.is_err());

        let retry_after = result.unwrap_err();
        // Should be between 1 and 3600 seconds
        assert!(retry_after > 0 && retry_after <= 3600, "retry_after should be reasonable");
    }
}
