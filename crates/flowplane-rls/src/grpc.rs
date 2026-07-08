//! The Envoy-facing `RateLimitService/ShouldRateLimit` implementation.
//!
//! Envoy sends a `domain` plus a list of descriptors; we match each descriptor against the
//! cached policy for that namespace, increment its fixed-window counter, and answer OK or
//! OVER_LIMIT. The overall response is OVER_LIMIT if any descriptor is over (Envoy then
//! enforces, e.g. 429). Unmatched descriptors are not limited.

use std::collections::BTreeMap;
use std::sync::Arc;

use envoy_types::pb::envoy::service::ratelimit::v3 as rls;
use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_response::{
    rate_limit::Unit as ProtoUnit, Code, DescriptorStatus, RateLimit as ProtoRateLimit,
};
use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_service_server::RateLimitService;
use envoy_types::pb::google::protobuf::Duration;
use fp_domain::rate_limit::{descriptors_canonical, RateLimitUnit};
use tonic::{Request, Response, Status};

use crate::counter::{now_unix, CounterStore};
use crate::policy::{MatchedPolicy, PolicyCache};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcAuthMode {
    /// Plaintext, unauthenticated gRPC. Valid only for config-gated loopback development.
    InsecureDevOnly,
    /// Production path: request extensions must carry authenticated dataplane evidence.
    RequireAuthenticatedDataplane,
}

/// Marker inserted by the authenticated transport layer after validating the Envoy/dataplane
/// client. Unit tests insert it directly so handler authz can be tested without a live mTLS
/// server.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedDataplane;

pub struct RlsService {
    pub policies: Arc<PolicyCache>,
    pub counters: Arc<dyn CounterStore>,
    auth_mode: GrpcAuthMode,
}

impl RlsService {
    pub fn new(
        policies: Arc<PolicyCache>,
        counters: Arc<dyn CounterStore>,
        auth_mode: GrpcAuthMode,
    ) -> Self {
        Self {
            policies,
            counters,
            auth_mode,
        }
    }

    /// Separator between the namespace and the canonical descriptor key in the counter key. A
    /// record separator that domains/descriptors are validated never to contain, so the key is
    /// unambiguous.
    fn counter_key(domain: &str, canonical: &str) -> String {
        format!("{domain}\u{1e}{canonical}")
    }
}

fn unit_to_proto(unit: RateLimitUnit) -> i32 {
    let mapped = match unit {
        RateLimitUnit::Second => ProtoUnit::Second,
        RateLimitUnit::Minute => ProtoUnit::Minute,
        RateLimitUnit::Hour => ProtoUnit::Hour,
        RateLimitUnit::Day => ProtoUnit::Day,
    };
    mapped as i32
}

fn ok_status() -> DescriptorStatus {
    DescriptorStatus {
        code: Code::Ok as i32,
        current_limit: None,
        limit_remaining: 0,
        duration_until_reset: None,
        ..Default::default()
    }
}

fn enforced_status(policy: MatchedPolicy, count: u64, now: u64) -> (DescriptorStatus, bool) {
    let over = count > policy.requests_per_unit;
    let remaining = policy.requests_per_unit.saturating_sub(count);
    // Seconds until the current epoch-aligned fixed window `[id*W, (id+1)*W)` resets — the same
    // window the counter store uses (`now / W`), so this is the real reset for this counter. Feeds
    // Envoy's `x-ratelimit-reset` header (when enabled). `now % W == 0` ⇒ a full window remains.
    let window = policy.unit.window_seconds().max(1);
    let until_reset = window - (now % window);
    let status = DescriptorStatus {
        code: if over { Code::OverLimit } else { Code::Ok } as i32,
        current_limit: Some(ProtoRateLimit {
            name: String::new(),
            requests_per_unit: u32::try_from(policy.requests_per_unit).unwrap_or(u32::MAX),
            unit: unit_to_proto(policy.unit),
        }),
        limit_remaining: u32::try_from(remaining).unwrap_or(u32::MAX),
        duration_until_reset: Some(Duration {
            seconds: i64::try_from(until_reset).unwrap_or(i64::MAX),
            nanos: 0,
        }),
        ..Default::default()
    };
    (status, over)
}

#[tonic::async_trait]
impl RateLimitService for RlsService {
    async fn should_rate_limit(
        &self,
        request: Request<rls::RateLimitRequest>,
    ) -> Result<Response<rls::RateLimitResponse>, Status> {
        if self.auth_mode == GrpcAuthMode::RequireAuthenticatedDataplane
            && request
                .extensions()
                .get::<AuthenticatedDataplane>()
                .is_none()
        {
            return Err(Status::unauthenticated(
                "authenticated RLS dataplane client required",
            ));
        }
        let req = request.into_inner();
        let now = now_unix();
        let request_addend = req.hits_addend;

        let mut statuses = Vec::with_capacity(req.descriptors.len());
        let mut overall_over = false;

        for descriptor in &req.descriptors {
            // Caller-shaped descriptors are untrusted (design Security). A duplicate key is
            // ambiguous, so refuse to match it rather than silently collapsing to one entry and
            // matching the wrong policy — matching must be deterministic.
            let mut entries: BTreeMap<String, String> = BTreeMap::new();
            let mut duplicate_key = false;
            for entry in &descriptor.entries {
                if entries
                    .insert(entry.key.clone(), entry.value.clone())
                    .is_some()
                {
                    duplicate_key = true;
                }
            }
            if duplicate_key {
                statuses.push(ok_status());
                continue;
            }

            let canonical = descriptors_canonical(&entries);
            match self.policies.lookup(&req.domain, &canonical) {
                Some(policy) => {
                    // A descriptor-level hits_addend overrides the request-level one; an
                    // effective addend of 0 is treated as 1 (Envoy semantics).
                    let effective = descriptor
                        .hits_addend
                        .as_ref()
                        .map(|v| v.value)
                        .unwrap_or_else(|| u64::from(request_addend));
                    let hits = if effective == 0 { 1 } else { effective };
                    let key = Self::counter_key(&req.domain, &canonical);
                    let count = self
                        .counters
                        .incr(&key, policy.unit.window_seconds(), hits, now);
                    let (status, over) = enforced_status(policy, count, now);
                    overall_over |= over;
                    statuses.push(status);
                }
                None => statuses.push(ok_status()),
            }
        }

        let overall_code = if overall_over {
            Code::OverLimit
        } else {
            Code::Ok
        } as i32;

        Ok(Response::new(rls::RateLimitResponse {
            overall_code,
            statuses,
            ..Default::default()
        }))
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::counter::InMemoryFixedWindow;
    use crate::policy::{PolicyPush, PushedPolicy};
    use envoy_types::pb::envoy::extensions::common::ratelimit::v3 as common;

    fn descriptor(pairs: &[(&str, &str)]) -> common::RateLimitDescriptor {
        common::RateLimitDescriptor {
            entries: pairs
                .iter()
                .map(|(k, v)| common::rate_limit_descriptor::Entry {
                    key: k.to_string(),
                    value: v.to_string(),
                })
                .collect(),
            limit: None,
            hits_addend: None,
        }
    }

    fn service_with(policy: PushedPolicy) -> RlsService {
        let cache = Arc::new(PolicyCache::new());
        cache.replace(PolicyPush {
            policies: vec![policy],
        });
        RlsService::new(
            cache,
            Arc::new(InMemoryFixedWindow::new()),
            GrpcAuthMode::InsecureDevOnly,
        )
    }

    fn pushed(domain: &str, pairs: &[(&str, &str)], rpu: u64) -> PushedPolicy {
        PushedPolicy {
            domain: domain.to_string(),
            descriptors: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            requests_per_unit: rpu,
            unit: RateLimitUnit::Minute,
        }
    }

    fn request(
        domain: &str,
        descriptors: Vec<common::RateLimitDescriptor>,
    ) -> Request<rls::RateLimitRequest> {
        Request::new(rls::RateLimitRequest {
            domain: domain.to_string(),
            descriptors,
            hits_addend: 0,
        })
    }

    #[derive(Default)]
    struct RecordingCounter {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl CounterStore for RecordingCounter {
        fn incr(&self, _key: &str, _window_seconds: u64, hits: u64, _now_unix: u64) -> u64 {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            hits
        }
    }

    #[tokio::test]
    async fn production_auth_mode_rejects_unauthenticated_call_before_counter_mutation() {
        let cache = Arc::new(PolicyCache::new());
        cache.replace(PolicyPush {
            policies: vec![pushed("orgA|teamA|checkout", &[("client_id", "bob")], 2)],
        });
        let counters = Arc::new(RecordingCounter::default());
        let svc = RlsService::new(
            cache,
            counters.clone(),
            GrpcAuthMode::RequireAuthenticatedDataplane,
        );

        let err = svc
            .should_rate_limit(request(
                "orgA|teamA|checkout",
                vec![descriptor(&[("client_id", "bob")])],
            ))
            .await
            .expect_err("unauthenticated production RLS call must fail");

        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            counters.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "unauthenticated request must not increment counters"
        );
    }

    #[tokio::test]
    async fn production_auth_mode_accepts_authenticated_dataplane_marker() {
        let cache = Arc::new(PolicyCache::new());
        cache.replace(PolicyPush {
            policies: vec![pushed("orgA|teamA|checkout", &[("client_id", "bob")], 2)],
        });
        let counters = Arc::new(RecordingCounter::default());
        let svc = RlsService::new(
            cache,
            counters.clone(),
            GrpcAuthMode::RequireAuthenticatedDataplane,
        );
        let mut req = request(
            "orgA|teamA|checkout",
            vec![descriptor(&[("client_id", "bob")])],
        );
        req.extensions_mut().insert(AuthenticatedDataplane);

        let resp = svc.should_rate_limit(req).await.unwrap().into_inner();

        assert_eq!(resp.overall_code, Code::Ok as i32);
        assert_eq!(
            counters.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "authenticated request may increment counters"
        );
    }

    #[tokio::test]
    async fn under_limit_is_ok_then_over_limit_trips() {
        let svc = service_with(pushed("orgA|teamA|checkout", &[("client_id", "bob")], 2));
        for expected_ok in [true, true, false] {
            let resp = svc
                .should_rate_limit(request(
                    "orgA|teamA|checkout",
                    vec![descriptor(&[("client_id", "bob")])],
                ))
                .await
                .unwrap()
                .into_inner();
            let ok = resp.overall_code == Code::Ok as i32;
            assert_eq!(ok, expected_ok, "overall_code={}", resp.overall_code);
        }
    }

    #[tokio::test]
    async fn matched_status_reports_a_nonzero_duration_until_reset() {
        // Regression for the hardcoded `duration_until_reset: None` that left Envoy's
        // `x-ratelimit-reset` header stuck at 0. A matched descriptor must carry the seconds left
        // in its epoch-aligned window — for a minute policy that is 1..=60 (never 0/None).
        let svc = service_with(pushed("orgA|teamA|checkout", &[("client_id", "bob")], 5));
        let resp = svc
            .should_rate_limit(request(
                "orgA|teamA|checkout",
                vec![descriptor(&[("client_id", "bob")])],
            ))
            .await
            .unwrap()
            .into_inner();
        let reset = resp.statuses[0]
            .duration_until_reset
            .as_ref()
            .expect("matched descriptor must report duration_until_reset");
        assert!(
            (1..=60).contains(&reset.seconds),
            "minute-window reset must be 1..=60s, got {}",
            reset.seconds
        );
        assert_eq!(reset.nanos, 0);
        // An unmatched descriptor still has no window, so it stays None.
        let unmatched = svc
            .should_rate_limit(request(
                "orgA|teamA|checkout",
                vec![descriptor(&[("client_id", "nobody")])],
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(unmatched.statuses[0].duration_until_reset.is_none());
    }

    #[test]
    fn duration_until_reset_is_the_seconds_left_in_the_epoch_window() {
        // Deterministic boundary math (no wall clock). Minute window W=60, epoch-aligned:
        // window 16 = [960, 1020). now=960 -> a full window remains; now=1019 -> 1s; now=1020 ->
        // the next window just opened, so a full 60s again.
        let policy = || MatchedPolicy {
            requests_per_unit: 5,
            unit: RateLimitUnit::Minute,
        };
        for (now, expected) in [(960u64, 60i64), (1019, 1), (1020, 60), (1000, 20)] {
            let (status, _) = enforced_status(policy(), 1, now);
            assert_eq!(
                status.duration_until_reset.expect("reset present").seconds,
                expected,
                "now={now}"
            );
        }
    }

    #[tokio::test]
    async fn unmatched_descriptor_is_not_limited() {
        let svc = service_with(pushed("orgA|teamA|checkout", &[("client_id", "bob")], 1));
        // Different value -> no policy -> never over limit, even after many calls.
        for _ in 0..10 {
            let resp = svc
                .should_rate_limit(request(
                    "orgA|teamA|checkout",
                    vec![descriptor(&[("client_id", "alice")])],
                ))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(resp.overall_code, Code::Ok as i32);
        }
    }

    #[tokio::test]
    async fn other_namespace_never_shares_a_counter() {
        let svc = service_with(pushed("orgA|teamA|checkout", &[("client_id", "bob")], 1));
        // Same descriptor, different namespace (another team) -> no policy -> not limited.
        let resp = svc
            .should_rate_limit(request(
                "orgA|teamB|checkout",
                vec![descriptor(&[("client_id", "bob")])],
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.overall_code, Code::Ok as i32);
    }

    #[tokio::test]
    async fn descriptor_entry_order_does_not_matter() {
        let svc = service_with(pushed("orgA|teamA|checkout", &[("a", "1"), ("b", "2")], 1));
        // First call OK, second over — even though entries are sent in the opposite order.
        let r1 = svc
            .should_rate_limit(request(
                "orgA|teamA|checkout",
                vec![descriptor(&[("b", "2"), ("a", "1")])],
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r1.overall_code, Code::Ok as i32);
        let r2 = svc
            .should_rate_limit(request(
                "orgA|teamA|checkout",
                vec![descriptor(&[("a", "1"), ("b", "2")])],
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r2.overall_code, Code::OverLimit as i32);
    }
}
