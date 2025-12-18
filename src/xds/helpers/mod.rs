//! Helper utilities for xDS resource manipulation.
//!
//! This module provides reusable abstractions for common xDS operations,
//! particularly around protobuf navigation and modification patterns.

mod listener_modifier;

pub use listener_modifier::ListenerModifier;
