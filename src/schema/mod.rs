//! Schema inference module for automatic API schema discovery
//!
//! This module provides schema inference capabilities for JSON payloads,
//! automatically learning API structure from observed traffic without storing
//! actual payload data.

pub mod inference;

pub use inference::{InferredSchema, SchemaInferenceEngine, SchemaType};
