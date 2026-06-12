//! Per-tenant write throttle (S2.6; v1 finding B11 carried forward): one tenant flooding
//! mutations must not degrade co-tenants. Fixed one-minute windows keyed by org (fallback:
//! user, then a shared anonymous bucket — fail closed, never unthrottled). Reads pass free.

use crate::error::ApiError;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fp_core::PrincipalCtx;
use fp_domain::{DomainError, ErrorCode, RequestId};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

pub struct WriteThrottle {
    limit_per_minute: u32,
    windows: Mutex<HashMap<String, (Instant, u32)>>,
}

impl WriteThrottle {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `Err(retry_after_seconds)` when the key is over its window budget.
    fn check(&self, key: &str) -> Result<(), u32> {
        let now = Instant::now();
        let mut windows = match self.windows.lock() {
            Ok(guard) => guard,
            // A poisoned lock means a panic elsewhere; throttling fails OPEN here because
            // denying all writes on a poisoned mutex turns one bug into an outage.
            Err(poisoned) => poisoned.into_inner(),
        };
        // Opportunistic cleanup keeps the map bounded by active tenants.
        if windows.len() > 10_000 {
            windows.retain(|_, (start, _)| start.elapsed().as_secs() < 120);
        }
        let entry = windows.entry(key.to_string()).or_insert((now, 0));
        if entry.0.elapsed().as_secs() >= 60 {
            *entry = (now, 0);
        }
        if entry.1 >= self.limit_per_minute {
            return Err(60u32
                .saturating_sub(entry.0.elapsed().as_secs() as u32)
                .max(1));
        }
        entry.1 += 1;
        Ok(())
    }
}

pub async fn tenant_write_throttle(
    State(state): State<crate::state::AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return next.run(request).await;
    }
    let key = match request.extensions().get::<PrincipalCtx>() {
        Some(PrincipalCtx::User { user_id, org, .. }) => match org {
            Some((org_id, _)) => format!("org:{org_id}"),
            None => format!("user:{user_id}"),
        },
        Some(PrincipalCtx::Agent { org_id, .. }) => format!("org:{org_id}"),
        None => "anonymous".to_string(),
    };
    if let Err(retry_after) = state.write_throttle.check(&key) {
        metrics::counter!("fp_tenant_write_throttled_total").increment(1);
        let rid = request
            .extensions()
            .get::<RequestId>()
            .copied()
            .unwrap_or_else(RequestId::generate);
        return ApiError::new(
            DomainError::new(
                ErrorCode::RateLimited,
                "per-tenant write rate limit exceeded",
            )
            .with_hint("space out mutating requests; reads are not limited")
            .with_retry_after(retry_after),
            rid,
        )
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn throttle_trips_at_limit_and_isolates_keys() {
        let throttle = WriteThrottle::new(3);
        for _ in 0..3 {
            assert!(throttle.check("org:a").is_ok());
        }
        let retry = throttle.check("org:a").expect_err("4th write must trip");
        assert!((1..=60).contains(&retry));
        // Tenant B is unaffected by A's exhaustion (the whole point).
        assert!(throttle.check("org:b").is_ok());
    }
}
