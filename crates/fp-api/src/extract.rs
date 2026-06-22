//! Request extractors that keep failures on the standard error envelope.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use fp_domain::{DomainError, RequestId};

use crate::error::ApiError;

/// JSON body extractor that renders deserialization failures as the standard
/// [`ApiError`] envelope (`validation_failed` → 400) instead of axum's default
/// bare `422` plain-text [`JsonRejection`]. Use this for every REST request body
/// so the documented contract ("every failure is the envelope; status derived
/// from `code`") holds on the malformed-JSON path too.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // RequestId is injected into extensions by the request-id middleware before
        // any extractor runs; fall back to a fresh one only if it is somehow absent.
        let rid = req
            .extensions()
            .get::<RequestId>()
            .copied()
            .unwrap_or_else(RequestId::generate);

        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::new(
                DomainError::validation(rejection_message(&rejection)),
                rid,
            )),
        }
    }
}

/// axum's `JsonRejection::body_text()` already carries an informative,
/// non-secret message (e.g. "Failed to deserialize the JSON body into the
/// target type: ..."). Surface it as the envelope message, matching the
/// existing well-typed-but-invalid validation path.
fn rejection_message(rejection: &JsonRejection) -> String {
    rejection.body_text()
}
