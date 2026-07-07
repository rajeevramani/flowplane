use fp_domain::{DomainError, ErrorCode};
use serde_json::{Map, Value};

pub(crate) const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "x-auth-token",
    "cookie",
    "set-cookie",
];

pub(crate) const DROPPED_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "date",
    "server",
    "traceparent",
    "tracestate",
    "x-b3-sampled",
    "x-b3-spanid",
    "x-b3-traceid",
    "x-envoy-attempt-count",
    "x-envoy-decorator-operation",
    "x-envoy-expected-rq-timeout-ms",
    "x-envoy-internal",
    "x-forwarded-client-cert",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-request-id",
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct ObservationQuotaState {
    pub sample_count: i64,
    pub target_sample_count: i32,
    pub byte_count: i64,
    pub max_bytes: i64,
    pub path_count: i64,
    pub max_distinct_paths: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObservationQuotaChange {
    pub sample_delta: i64,
    pub byte_delta: i64,
    pub path_delta: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationQuotaDrop {
    TargetSampleCount,
    ByteLimit,
    DistinctPathLimit,
}

impl ObservationQuotaDrop {
    pub(crate) fn into_error(self, session_kind: &str) -> DomainError {
        match self {
            Self::TargetSampleCount => DomainError::new(
                ErrorCode::QuotaExceeded,
                format!("{session_kind} session has reached its target sample count"),
            )
            .with_hint(format!(
                "start a new {session_kind} session for additional samples"
            )),
            Self::ByteLimit => DomainError::new(
                ErrorCode::QuotaExceeded,
                format!("{session_kind} session has reached its raw observation byte limit"),
            )
            .with_hint(format!(
                "raise max_bytes or start a narrower {session_kind} session"
            )),
            Self::DistinctPathLimit => DomainError::new(
                ErrorCode::QuotaExceeded,
                format!("{session_kind} session has reached its distinct path limit"),
            )
            .with_hint(format!(
                "raise max_distinct_paths or scope {session_kind} to fewer routes"
            )),
        }
    }
}

pub(crate) fn sanitize_headers(headers: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if DROPPED_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        if REDACTED_HEADERS.contains(&lower.as_str()) {
            out.insert(name.clone(), Value::String("[REDACTED]".to_string()));
        } else {
            out.insert(name.clone(), value.clone());
        }
    }
    Value::Object(out)
}

pub(crate) fn merge_headers(existing: Value, incoming: Value) -> Value {
    match incoming {
        Value::Object(map) if map.is_empty() => existing,
        value => value,
    }
}

pub(crate) fn body_bytes(value: Option<&str>) -> i64 {
    value.map_or(0, |body| body.len() as i64)
}

pub(crate) fn merged_body_bytes(
    body: Option<&str>,
    existing_bytes: Option<i64>,
    incoming_bytes: Option<i64>,
) -> i64 {
    [Some(body_bytes(body)), existing_bytes, incoming_bytes]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0)
}

pub(crate) fn decide_observation_quota(
    state: ObservationQuotaState,
    existing_observation: bool,
    existing_body_bytes: Option<i64>,
    merged_body_bytes: i64,
    path_already_present: bool,
) -> Result<ObservationQuotaChange, ObservationQuotaDrop> {
    let existing_body_bytes = existing_body_bytes.unwrap_or(0);
    let sample_delta = if existing_observation { 0 } else { 1 };
    let path_delta = if !existing_observation && !path_already_present {
        1
    } else {
        0
    };
    let byte_delta = merged_body_bytes - existing_body_bytes;
    let next_sample_count = state.sample_count + sample_delta;
    let next_byte_count = state.byte_count + byte_delta;
    if !existing_observation && next_sample_count > i64::from(state.target_sample_count) {
        return Err(ObservationQuotaDrop::TargetSampleCount);
    }
    if next_byte_count > state.max_bytes {
        return Err(ObservationQuotaDrop::ByteLimit);
    }
    if path_delta > 0 && state.path_count + path_delta > i64::from(state.max_distinct_paths) {
        return Err(ObservationQuotaDrop::DistinctPathLimit);
    }
    Ok(ObservationQuotaChange {
        sample_delta,
        byte_delta,
        path_delta,
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_redacts_and_drops_shared_header_names() {
        let mut headers = Map::new();
        for name in REDACTED_HEADERS {
            headers.insert((*name).to_string(), Value::String("secret".to_string()));
        }
        for name in DROPPED_HEADERS {
            headers.insert((*name).to_string(), Value::String("hop".to_string()));
        }
        headers.insert(
            "accept".to_string(),
            Value::String("application/json".to_string()),
        );

        let sanitized = sanitize_headers(&headers);
        for name in REDACTED_HEADERS {
            assert_eq!(sanitized[*name], "[REDACTED]");
        }
        for name in DROPPED_HEADERS {
            assert!(sanitized.get(*name).is_none(), "{name} must be dropped");
        }
        assert_eq!(sanitized["accept"], "application/json");
    }

    #[test]
    fn quota_decision_accepts_new_sample() {
        let change = decide_observation_quota(state(), false, None, 12, false).expect("accepted");
        assert_eq!(
            change,
            ObservationQuotaChange {
                sample_delta: 1,
                byte_delta: 12,
                path_delta: 1,
            }
        );
    }

    #[test]
    fn quota_decision_duplicate_request_does_not_consume_sample() {
        let mut state = state();
        state.byte_count = 5;
        let change = decide_observation_quota(state, true, Some(5), 12, true).expect("accepted");
        assert_eq!(
            change,
            ObservationQuotaChange {
                sample_delta: 0,
                byte_delta: 7,
                path_delta: 0,
            }
        );
    }

    #[test]
    fn quota_decision_rejects_byte_limit() {
        let err = decide_observation_quota(state(), false, None, 101, false).expect_err("quota");
        assert_eq!(err, ObservationQuotaDrop::ByteLimit);
    }

    #[test]
    fn quota_decision_rejects_distinct_path_limit() {
        let mut state = state();
        state.path_count = 10;
        let err = decide_observation_quota(state, false, None, 1, false).expect_err("quota");
        assert_eq!(err, ObservationQuotaDrop::DistinctPathLimit);
    }

    #[test]
    fn quota_decision_rejects_target_sample_count_exhaustion() {
        let mut state = state();
        state.sample_count = 10;
        let err = decide_observation_quota(state, false, None, 1, true).expect_err("quota");
        assert_eq!(err, ObservationQuotaDrop::TargetSampleCount);
    }

    fn state() -> ObservationQuotaState {
        ObservationQuotaState {
            sample_count: 0,
            target_sample_count: 10,
            byte_count: 0,
            max_bytes: 100,
            path_count: 0,
            max_distinct_paths: 10,
        }
    }
}
