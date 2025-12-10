# Adding a New Envoy HTTP Filter

This guide explains how to add a new Envoy HTTP filter to the Flowplane control plane using the **Dynamic Filter Framework**.

## Overview

The Dynamic Filter Framework provides a schema-driven approach to adding new filters. The effort required depends on the type of filter:

| Filter Type | Steps Required | Rebuild Needed? |
|-------------|----------------|-----------------|
| **Custom WASM/Lua filter** | 1-2 (YAML schema + optional UI) | No |
| **Built-in Envoy filter** | 3-4 (YAML + Rust protobuf + xDS match arm) | Yes |

## Quick Reference

### Option A: Simple Filter (Custom WASM/Lua - Fully Dynamic)

| Step | File | Description |
|------|------|-------------|
| 1 | `filter-schemas/custom/my_filter.yaml` | Create YAML schema |
| 2 | Reload via API | `POST /api/v1/admin/filter-schemas/reload` |

UI forms are auto-generated from the JSON Schema - no custom components needed.

### Option B: Built-in Envoy Filter (Hybrid - Requires Rust Code)

| Step | File | Description |
|------|------|-------------|
| 1 | `filter-schemas/built-in/my_filter.yaml` | Create YAML schema |
| 2 | `src/xds/filters/http/my_filter.rs` | Create protobuf conversion module |
| 3 | `src/xds/filters/http/mod.rs` | Export module |
| 4 | `src/xds/filters/injection/listener.rs` | Add `try_typed_conversion()` match arm |

UI forms are auto-generated from the JSON Schema - no custom components needed.

---

## Option A: Adding a Custom Filter (Fully Dynamic)

Custom filters that accept `google.protobuf.Struct` configuration (like WASM or Lua filters) can be added without any code changes.

### Step 1: Create YAML Schema

**File**: `filter-schemas/custom/my_custom_filter.yaml`

```yaml
name: my_custom_filter
display_name: My Custom Filter
description: A custom filter for specific processing
version: "1.0"

envoy:
  http_filter_name: envoy.filters.http.wasm
  type_url: type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm
  per_route_type_url: null  # Optional

capabilities:
  attachment_points:
    - route
    - listener
  requires_listener_config: true
  per_route_behavior: full_config  # full_config | reference_only | disable_only | not_supported

# JSON Schema for configuration validation
config_schema:
  type: object
  required:
    - plugin_name
  properties:
    plugin_name:
      type: string
      description: Name of the WASM plugin
      minLength: 1
    plugin_config:
      type: object
      description: Plugin-specific configuration
      additionalProperties: true
    fail_open:
      type: boolean
      description: Whether to fail open on plugin errors
      default: false

# Per-route config schema (if different from main)
per_route_config_schema:
  type: object
  properties:
    disabled:
      type: boolean
      default: false

# UI hints for form generation
ui_hints:
  form_layout: sections
  sections:
    - name: Basic
      fields:
        - plugin_name
        - fail_open
    - name: Plugin Configuration
      fields:
        - plugin_config
      collapsible: true
```

### Step 2: Reload Schemas

```bash
curl -X POST http://localhost:8080/api/v1/admin/filter-schemas/reload \
  -H "Authorization: Bearer <token>"
```

**Done!** The filter is now available in the UI and API. The form is auto-generated from the `config_schema` in the YAML file.

---

## Option B: Adding a Built-in Envoy Filter (Hybrid)

Built-in Envoy filters require proper protobuf serialization, which needs Rust code.

### Step 1: Create YAML Schema

**File**: `filter-schemas/built-in/my_new_filter.yaml`

```yaml
name: my_new_filter
display_name: My New Filter
description: An Envoy built-in HTTP filter
version: "1.0"

envoy:
  http_filter_name: envoy.filters.http.my_new_filter
  type_url: type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilter
  per_route_type_url: type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilterPerRoute

capabilities:
  attachment_points:
    - route
    - listener
  requires_listener_config: true
  per_route_behavior: full_config

config_schema:
  type: object
  required:
    - some_field
  properties:
    some_field:
      type: string
      description: Required configuration field
      minLength: 1
    optional_field:
      type: integer
      description: Optional numeric field

per_route_config_schema:
  type: object
  properties:
    disabled:
      type: boolean
      default: false
    some_field:
      type: string

ui_hints:
  form_layout: flat
```

### Step 2: Create Protobuf Conversion Module

**File**: `src/xds/filters/http/my_new_filter.rs`

```rust
//! My New Filter HTTP filter configuration

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use envoy_types::pb::envoy::extensions::filters::http::my_new_filter::v3::{
    MyNewFilter as MyNewFilterProto,
    MyNewFilterPerRoute as MyNewFilterPerRouteProto,
};
use serde::{Deserialize, Serialize};

const TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilter";
const PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.my_new_filter.v3.MyNewFilterPerRoute";

/// Configuration for my new filter (matches JSON Schema in YAML)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MyNewFilterConfig {
    pub some_field: String,
    #[serde(default)]
    pub optional_field: Option<i32>,
}

impl MyNewFilterConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.some_field.is_empty() {
            return Err(invalid_config("some_field cannot be empty"));
        }
        Ok(())
    }

    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = MyNewFilterProto {
            some_field: self.some_field.clone(),
            optional_field: self.optional_field,
        };

        Ok(any_from_message(TYPE_URL, &proto))
    }

    /// Build from JSON value
    pub fn from_json(value: &serde_json::Value) -> Result<Self, crate::Error> {
        serde_json::from_value(value.clone())
            .map_err(|e| invalid_config(&format!("Invalid config: {}", e)))
    }
}

/// Per-route configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MyNewFilterPerRouteConfig {
    #[serde(default)]
    pub disabled: bool,
    pub some_field: Option<String>,
}

impl MyNewFilterPerRouteConfig {
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let proto = MyNewFilterPerRouteProto {
            disabled: self.disabled,
            some_field: self.some_field.clone().unwrap_or_default(),
        };
        Ok(any_from_message(PER_ROUTE_TYPE_URL, &proto))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation() {
        let config = MyNewFilterConfig {
            some_field: "".to_string(),
            optional_field: None,
        };
        assert!(config.validate().is_err());

        let valid = MyNewFilterConfig {
            some_field: "test".to_string(),
            optional_field: Some(42),
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_to_any() {
        let config = MyNewFilterConfig {
            some_field: "test".to_string(),
            optional_field: None,
        };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, TYPE_URL);
    }
}
```

### Step 3: Export the Module

**File**: `src/xds/filters/http/mod.rs`

```rust
pub mod my_new_filter;

// Re-export
pub use my_new_filter::{MyNewFilterConfig, MyNewFilterPerRouteConfig};
```

### Step 4: Add xDS Conversion Match Arm

**File**: `src/xds/filters/injection/listener.rs`

Add the filter to `try_typed_conversion()`:

```rust
fn try_typed_conversion(
    filter_type: &str,
    config: &serde_json::Value,
) -> Option<Result<EnvoyAny, crate::Error>> {
    match filter_type {
        // Existing static conversions (4 filters):
        "header_mutation" => { /* protobuf conversion */ }
        "local_rate_limit" => { /* protobuf conversion */ }
        "custom_response" => { /* protobuf conversion */ }
        "mcp" => { /* protobuf conversion */ }
        // Note: jwt_auth is handled separately in process_filters()

        // Add new filter
        "my_new_filter" => {
            Some(
                MyNewFilterConfig::from_json(config)
                    .and_then(|cfg| cfg.to_any())
            )
        }

        _ => None, // Falls back to dynamic Struct conversion
    }
}
```

**File**: `src/xds/filters/injection/route.rs`

If per-route config is supported, add to `convert_filter_to_scoped_config()`:

```rust
fn convert_filter_to_scoped_config(
    filter_data: &FilterData,
    converter: &DynamicFilterConverter,
) -> Option<(String, HttpScopedConfig)> {
    // Try static conversion first
    if let Ok(filter_config) = serde_json::from_str::<FilterConfig>(&filter_data.configuration) {
        match filter_config.to_per_route_config() {
            Ok(Some((name, config))) => return Some((name, config)),
            Ok(None) => return None,
            Err(_) => { /* Fall through to dynamic */ }
        }
    }

    // Dynamic fallback...
}
```

### Step 5: Rebuild and Test

```bash
cargo fmt && cargo clippy && cargo test
```

---

## Schema File Reference

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique filter identifier (snake_case) |
| `display_name` | string | Human-readable name |
| `description` | string | Filter description |
| `envoy.http_filter_name` | string | Envoy HTTP filter name |
| `envoy.type_url` | string | Protobuf type URL |
| `capabilities.attachment_points` | array | `["route"]`, `["listener"]`, or both |
| `capabilities.requires_listener_config` | boolean | Whether filter needs listener-level config |
| `capabilities.per_route_behavior` | string | See below |
| `config_schema` | object | JSON Schema for configuration |

### Per-Route Behavior Options

| Value | Description | Example |
|-------|-------------|---------|
| `full_config` | Full configuration override | header_mutation, local_rate_limit |
| `reference_only` | Reference by name only | jwt_auth (requirement_name) |
| `disable_only` | Only supports disable flag | mcp |
| `not_supported` | No per-route config | - |

### UI Hints (Optional)

```yaml
ui_hints:
  form_layout: sections  # flat | sections | tabs
  sections:
    - name: Section Name
      fields:
        - field_name
      collapsible: true  # optional
```

---

## Architecture: Fully Dynamic UI

All filters now use `DynamicFilterForm` - forms are auto-generated from JSON Schema:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Filter Create/Edit Page                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Load filter type from /api/v1/filter-types                      │
│         │                                                        │
│         ▼                                                        │
│  DynamicFilterForm                                               │
│    - Auto-generated from JSON Schema (config_schema)             │
│    - Works for ALL filter types                                  │
│    - No custom Svelte components needed                          │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

The `filter-form-registry.ts` exists for backward compatibility but always returns `false` for `hasCustomForm()` - all filters use dynamic forms.

---

## Why Built-in Filters Need Rust Code

Envoy built-in filters expect **proper protobuf messages**, not `google.protobuf.Struct`. When we send:

```
type_url: "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit"
value: <bytes>
```

The `value` bytes must be a serialized `LocalRateLimit` protobuf message. Envoy cannot deserialize a `Struct` into the expected type.

The `envoy-types` crate provides Rust structs that serialize to correct protobuf, but using them requires match arms.

### What's Dynamic vs Static

| Component | Custom Filters | Built-in Filters |
|-----------|----------------|------------------|
| Filter type definitions | Dynamic (YAML) | Dynamic (YAML) |
| UI form generation | Dynamic (JSON Schema) | Dynamic (JSON Schema) |
| Configuration validation | Dynamic (JSON Schema) | Dynamic (JSON Schema) |
| xDS conversion | Dynamic (Struct) | **Static (Rust protobuf)** |

---

## Database: No Migration Needed

The Dynamic Filter Framework removed the database CHECK constraint on `filter_type`. New filter types are validated at the API layer using the schema registry.

Migration `20251210000001_remove_filter_type_check_constraint.sql` already applied this change.

---

## API Endpoints

### List Available Filter Types

```bash
curl http://localhost:8080/api/v1/filter-types \
  -H "Authorization: Bearer <token>"
```

Returns all registered filter types with their schemas and metadata.

### Reload Custom Schemas

```bash
curl -X POST http://localhost:8080/api/v1/admin/filter-schemas/reload \
  -H "Authorization: Bearer <token>"
```

Hot-reloads custom schemas from `filter-schemas/custom/` without restart.

---

## Testing Checklist

### For Custom Filters (WASM/Lua)

- [ ] YAML schema is valid
- [ ] Schema reload succeeds
- [ ] Filter appears in `/api/v1/filter-types`
- [ ] UI form generates correctly
- [ ] Filter can be created via API/UI
- [ ] Filter can be attached to routes/listeners
- [ ] xDS snapshot includes filter (check Envoy admin)

### For Built-in Filters

All of the above, plus:

- [ ] Rust module compiles
- [ ] `to_any()` produces valid protobuf
- [ ] Unit tests pass
- [ ] Envoy accepts the configuration
- [ ] Per-route config works (if supported)

```bash
# Backend tests
cargo fmt && cargo clippy && cargo test

# Frontend build
cd ui && npm run check && npm run build
```

---

## Migration from Legacy (12+ Steps)

If you have filters implemented the old way (before Dynamic Filter Framework), they continue to work. The framework is backward compatible.

To migrate an existing filter to schema-driven:

1. Create YAML schema file
2. Keep Rust protobuf code (still needed for xDS)
3. Remove hardcoded UI components (use `DynamicFilterForm`)
4. Remove validation match arms (schema validates)

The static `filter_registry()` and `FilterSchemaRegistry` coexist - the schema registry is authoritative for the API.

---

## Example: Complete Built-in Filter

For reference implementations, see:

| Filter | Schema | Rust Module |
|--------|--------|-------------|
| header_mutation | `filter-schemas/built-in/header_mutation.yaml` | `src/xds/filters/http/header_mutation.rs` |
| local_rate_limit | `filter-schemas/built-in/local_rate_limit.yaml` | `src/xds/filters/http/local_rate_limit.rs` |
| jwt_auth | `filter-schemas/built-in/jwt_auth.yaml` | `src/xds/filters/http/jwt_authentication.rs` |

---

## Troubleshooting

### Filter not appearing in UI

1. Check YAML syntax: `python -c "import yaml; yaml.safe_load(open('filter-schemas/custom/my_filter.yaml'))"`
2. Reload schemas: `POST /api/v1/admin/filter-schemas/reload`
3. Check server logs for parsing errors

### Envoy rejects configuration

1. For built-in filters: Ensure Rust protobuf code matches Envoy version
2. For custom filters: Verify Envoy supports Struct config for that filter type
3. Check `type_url` matches exactly

### Validation errors

1. Check `config_schema` in YAML matches expected fields
2. Verify `required` array lists mandatory fields
3. Check field types match (string vs integer vs boolean)
