//! Flowplane domain model.
//!
//! Pure types only: no async, no IO, no SQL (spec/10 §2). Everything observable by other
//! crates — errors, identifiers, lifecycle states, event types — originates here so all
//! surfaces speak one language.

pub mod error;
pub mod id;

pub use error::{DomainError, DomainResult, ErrorCode};
pub use id::{OrgId, RequestId, TeamId};
