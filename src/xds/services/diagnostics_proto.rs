//! Generated protobuf / tonic bindings for `flowplane.diagnostics.v1`.
//!
//! The proto source of truth lives at
//! `proto/flowplane/diagnostics/v1/diagnostics.proto` and is compiled by
//! `build.rs` via `tonic-build`. This module is intentionally a thin wrapper
//! that only `include!`s the generated code so that the rest of the codebase
//! can refer to types via `crate::xds::services::diagnostics_proto::*`
//! instead of a free-floating `tonic::include_proto!` call.
//!
//! The service trait implementation for `EnvoyDiagnosticsService` lives in
//! a SEPARATE module (to be added in fp-hsk.4) — this file is proto-only and
//! must remain free of business logic so that fp-hsk.1 stays a pure schema
//! definition and can be reviewed in isolation.

#![allow(clippy::doc_overindented_list_items)]
#![allow(missing_docs)]

tonic::include_proto!("flowplane.diagnostics.v1");
