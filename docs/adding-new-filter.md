# Adding a New Envoy HTTP Filter

This guide explains how to add a new Envoy HTTP filter to the Flowplane control plane using the unified filter framework.

## Overview

The filter framework provides a centralized, type-safe approach to adding new filters. Adding a new filter requires modifications to 6 specific locations, and all validation, attachment rules, and injection behavior are handled automatically.

## Prerequisites

Before adding a new filter, ensure you understand:
1. The Envoy filter's protobuf definition and type URLs
2. Whether the filter supports per-route configuration
3. Valid attachment points (route-only, listener-only, or both)
4. Whether the filter requires configuration at listener level

## Step-by-Step Guide

### Step 1: Add FilterType Variant

**File**: `src/domain/filter.rs`

Add a new variant to the `FilterType` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    HeaderMutation,
    JwtAuth,
    // ... existing variants ...
    MyNewFilter,  // Add your new filter
}
```

Also update the `Display` and `FromStr` implementations:

```rust
impl fmt::Display for FilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // ... existing arms ...
            FilterType::MyNewFilter => write!(f, "my_new_filter"),
        }
    }
}

impl std::str::FromStr for FilterType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // ... existing arms ...
            "my_new_filter" => Ok(FilterType::MyNewFilter),
            _ => Err(format!("Unknown filter type: {}", s)),
        }
    }
}
```

### Step 2: Add Metadata Entry

**File**: `src/domain/filter.rs`

Add a metadata entry in the `filter_registry()` function:

```rust
fn filter_registry(filter_type: FilterType) -> FilterTypeMetadata {
    match filter_type {
        // ... existing entries ...
        FilterType::MyNewFilter => FilterTypeMetadata {
            filter_type: FilterType::MyNewFilter,
            http_filter_name: "envoy.filters.http.my_new_filter",
            type_url: "type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilter",
            per_route_type_url: Some("type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilterPerRoute"),
            attachment_points: ROUTE_AND_LISTENER,  // or ROUTE_ONLY
            requires_listener_config: true,  // or false if can be empty placeholder
            per_route_behavior: PerRouteBehavior::FullConfig,  // See options below
            is_implemented: true,
            description: "Description of what this filter does",
        },
    }
}
```

Also add the filter type to the `from_http_filter_name()` lookup array:

```rust
pub fn from_http_filter_name(name: &str) -> Option<Self> {
    [
        FilterType::HeaderMutation,
        // ... existing types ...
        FilterType::MyNewFilter,
    ]
    .into_iter()
    .find(|filter_type| filter_type.http_filter_name() == name)
}
```

#### PerRouteBehavior Options

| Behavior | Description | Example Filters |
|----------|-------------|-----------------|
| `FullConfig` | Full configuration override at per-route level | HeaderMutation, LocalRateLimit, CustomResponse |
| `ReferenceOnly` | Reference to listener-level config by name | JwtAuth (uses requirement_name) |
| `DisableOnly` | Only supports disabling the filter per-route | MCP |
| `NotSupported` | No per-route configuration support | - |

### Step 3: Create Filter Configuration Module

**File**: `src/xds/filters/http/my_new_filter.rs`

Create a new module with:
1. Configuration structs with serde serialization
2. Validation logic
3. Protobuf conversion methods
4. Per-route configuration (if supported)

```rust
//! My New Filter HTTP filter configuration helpers

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
// Import the relevant Envoy proto types
use envoy_types::pb::envoy::extensions::filters::http::my_new_filter::v3::{
    MyNewFilter as MyNewFilterProto,
    MyNewFilterPerRoute as MyNewFilterPerRouteProto,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const MY_NEW_FILTER_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilter";
const MY_NEW_FILTER_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilterPerRoute";

/// Configuration for my new filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct MyNewFilterConfig {
    /// Configuration field
    pub some_field: String,
    /// Optional field with default
    #[serde(default)]
    pub optional_field: Option<u32>,
}

impl MyNewFilterConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.some_field.is_empty() {
            return Err(invalid_config("MyNewFilter some_field cannot be empty"));
        }
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = MyNewFilterProto {
            some_field: self.some_field.clone(),
            optional_field: self.optional_field,
        };

        Ok(any_from_message(MY_NEW_FILTER_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &MyNewFilterProto) -> Result<Self, crate::Error> {
        let config = Self {
            some_field: proto.some_field.clone(),
            optional_field: proto.optional_field,
        };
        config.validate()?;
        Ok(config)
    }
}

/// Per-route configuration (if supported)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct MyNewFilterPerRouteConfig {
    #[serde(default)]
    pub disabled: bool,
    // Add per-route specific fields
}

impl MyNewFilterPerRouteConfig {
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let proto = MyNewFilterPerRouteProto {
            disabled: self.disabled,
        };
        Ok(any_from_message(MY_NEW_FILTER_PER_ROUTE_TYPE_URL, &proto))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = MyNewFilterConfig {
            some_field: "".to_string(),
            optional_field: None,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_to_any() {
        let config = MyNewFilterConfig {
            some_field: "test".to_string(),
            optional_field: Some(42),
        };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, MY_NEW_FILTER_TYPE_URL);
    }
}
```

### Step 4: Export the Module

**File**: `src/xds/filters/http/mod.rs`

Add the module export:

```rust
pub mod my_new_filter;
```

Add imports at the top:

```rust
use crate::xds::filters::http::my_new_filter::{MyNewFilterConfig, MyNewFilterPerRouteConfig};
```

Add to `HttpFilterKind` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HttpFilterKind {
    // ... existing variants ...
    /// My New Filter description
    MyNewFilter(MyNewFilterConfig),
}
```

Update `HttpFilterKind` methods:

```rust
impl HttpFilterKind {
    fn default_name(&self) -> &'static str {
        match self {
            // ... existing arms ...
            Self::MyNewFilter(_) => "envoy.filters.http.my_new_filter",
        }
    }

    fn to_any(&self) -> Result<Option<EnvoyAny>, crate::Error> {
        match self {
            // ... existing arms ...
            Self::MyNewFilter(cfg) => cfg.to_any().map(Some),
        }
    }
}
```

Add to `HttpScopedConfig` if per-route is supported:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "filter_type", rename_all = "snake_case")]
pub enum HttpScopedConfig {
    // ... existing variants ...
    /// My New Filter per-route overrides
    MyNewFilter(MyNewFilterPerRouteConfig),
}
```

Update `HttpScopedConfig::to_any()` and `from_any()` methods.

### Step 5: Add FilterConfig Variant

**File**: `src/domain/filter.rs`

Add to the `FilterConfig` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "config", rename_all = "snake_case")]
pub enum FilterConfig {
    // ... existing variants ...
    MyNewFilter(MyNewFilterConfig),
}
```

Update `FilterConfig::filter_type()`:

```rust
impl FilterConfig {
    pub fn filter_type(&self) -> FilterType {
        match self {
            // ... existing arms ...
            FilterConfig::MyNewFilter(_) => FilterType::MyNewFilter,
        }
    }
}
```

### Step 6: Update Conversion Module

**File**: `src/xds/filters/conversion.rs`

Add match arms to the unified conversion methods:

```rust
impl FilterConfig {
    pub fn to_listener_any(&self) -> Result<EnvoyAny> {
        match self {
            // ... existing arms ...
            FilterConfig::MyNewFilter(config) => config.to_any(),
        }
    }

    pub fn to_per_route_config(&self) -> Result<Option<(String, HttpScopedConfig)>> {
        // ... existing code ...
        match self {
            // ... existing arms ...
            FilterConfig::MyNewFilter(config) => {
                let per_route = MyNewFilterPerRouteConfig::from_listener_config(config);
                Ok(Some((
                    metadata.http_filter_name.to_string(),
                    HttpScopedConfig::MyNewFilter(per_route),
                )))
            }
        }
    }
}
```

If the filter can be an empty placeholder, update `create_empty_listener_filter()`:

```rust
pub fn create_empty_listener_filter(filter_type: FilterType) -> Option<HttpFilterKind> {
    match filter_type {
        // ... existing arms ...
        FilterType::MyNewFilter => {
            Some(HttpFilterKind::MyNewFilter(MyNewFilterConfig::default()))
        }
    }
}
```

## Testing Checklist

After implementing the filter, ensure you have tests for:

- [ ] Configuration validation (valid and invalid cases)
- [ ] Protobuf serialization (`to_any()`)
- [ ] Protobuf deserialization (`from_proto()`)
- [ ] Round-trip conversion (serialize then deserialize)
- [ ] Per-route configuration (if supported)
- [ ] Attachment point validation
- [ ] Integration with route/listener injection

Run the test suite:

```bash
cargo fmt && cargo clippy && cargo test
```

## File Summary

| File | Changes |
|------|---------|
| `src/domain/filter.rs` | Add `FilterType` variant, metadata, `FilterConfig` variant |
| `src/domain/mod.rs` | Export new types (if any) |
| `src/xds/filters/http/my_new_filter.rs` | Create new module with config structs |
| `src/xds/filters/http/mod.rs` | Export module, add `HttpFilterKind` and `HttpScopedConfig` variants |
| `src/xds/filters/conversion.rs` | Add match arms for unified conversion |

## Framework Benefits

By following this framework:

1. **Single source of truth**: All filter metadata is in `filter_registry()`
2. **Automatic validation**: Attachment points and per-route behavior are checked automatically
3. **Consistent injection**: The unified conversion trait handles both listener and route injection
4. **Type safety**: Compile-time checks for filter capabilities
5. **Minimal boilerplate**: Most behavior is derived from metadata

## Common Patterns

### Filter with DisableOnly per-route support

```rust
FilterType::MyFilter => FilterTypeMetadata {
    // ...
    per_route_behavior: PerRouteBehavior::DisableOnly,
    // ...
},
```

In conversion.rs, return `Ok(None)` since we don't inject per-route config:
```rust
FilterConfig::MyFilter(_) => Ok(None),
```

### Route-only filter (no listener attachment)

```rust
FilterType::MyFilter => FilterTypeMetadata {
    // ...
    attachment_points: ROUTE_ONLY,
    requires_listener_config: false,
    // ...
},
```

### Filter requiring external service

```rust
FilterType::MyFilter => FilterTypeMetadata {
    // ...
    requires_listener_config: true,  // Cannot be empty placeholder
    // ...
},
```

## Example: Full HeaderMutation Implementation

For a complete reference implementation, see:
- `src/xds/filters/http/header_mutation.rs` - Filter configuration
- `src/domain/filter.rs:65-75` - Metadata entry
- `src/xds/filters/conversion.rs:44-69` - Listener conversion
- `src/xds/filters/conversion.rs:103-131` - Per-route conversion
