# Adding a New Envoy HTTP Filter

This guide explains how to add a new Envoy HTTP filter to the Flowplane control plane using the unified filter framework.

## Overview

The filter framework provides a centralized, type-safe approach to adding new filters. Adding a new filter requires modifications to **12 specific locations** across backend, database, and frontend, and all validation, attachment rules, and injection behavior are handled automatically.

## Quick Reference: All Steps

| Step | File | Description |
|------|------|-------------|
| **Backend - Domain & Types** |||
| 1 | `src/domain/filter.rs` | Add `FilterType` enum variant |
| 2 | `src/domain/filter.rs` | Add metadata entry in `filter_registry()` |
| 3 | `src/xds/filters/http/my_filter.rs` | Create filter configuration module |
| 4 | `src/xds/filters/http/mod.rs` | Export module, add to `HttpFilterKind` |
| 5 | `src/domain/filter.rs` | Add `FilterConfig` enum variant |
| 6 | `src/xds/filters/conversion.rs` | Add match arms for conversion |
| **Backend - Validation** |||
| 7 | `src/api/handlers/filters/validation.rs` | Add API validation match arm |
| 8 | `src/services/filter_service.rs` | Add service layer validation match arm |
| **Database** |||
| 9 | `migrations/YYYYMMDD_*.sql` | Add filter type to CHECK constraint |
| **Frontend** |||
| 10 | `ui/src/lib/api/types.ts` | Add TypeScript types |
| 11 | `ui/src/lib/components/filters/*.svelte` | Create config form component |
| 12 | `ui/src/routes/(authenticated)/filters/create/+page.svelte` | Update create page |
| 13 | `ui/src/routes/(authenticated)/filters/[id]/edit/+page.svelte` | Update edit page |

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

### Step 7: Add Validation Match Arms

**File**: `src/api/handlers/filters/validation.rs`

Add the filter type to the validation match statement for create requests:

```rust
pub fn validate_create_filter_request(payload: &CreateFilterRequest) -> Result<(), ApiError> {
    // ... existing validation ...

    match (&payload.filter_type, &payload.config) {
        // ... existing arms ...
        (crate::domain::FilterType::MyNewFilter, FilterConfig::MyNewFilter(_)) => Ok(()),
        _ => {
            Err(ApiError::validation("Filter type and configuration do not match"))
        }
    }
}
```

### Step 8: Add Service Layer Validation

**File**: `src/services/filter_service.rs`

Add the filter type to the service validation match statement:

```rust
// In create_filter() method
match (&filter_type, &config) {
    (FilterType::HeaderMutation, FilterConfig::HeaderMutation(_)) => {}
    (FilterType::JwtAuth, FilterConfig::JwtAuth(_)) => {}
    // ... existing arms ...
    (FilterType::MyNewFilter, FilterConfig::MyNewFilter(_)) => {}
    _ => return Err(Error::validation("Filter type and configuration do not match")),
}
```

### Step 9: Create Database Migration

**File**: `migrations/YYYYMMDD000001_add_my_new_filter_type.sql`

Create a new migration to add the filter type to the database CHECK constraint:

```sql
-- Add my_new_filter to the filter_type CHECK constraint
-- SQLite requires table recreation to modify CHECK constraints

-- Drop old table (no data to preserve - pre-production)
DROP TABLE IF EXISTS filters;

-- Create table with updated constraint
CREATE TABLE filters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    filter_type TEXT NOT NULL CHECK (filter_type IN ('header_mutation', 'jwt_auth', 'cors', 'local_rate_limit', 'rate_limit', 'ext_authz', 'custom_response', 'mcp', 'my_new_filter')),
    description TEXT,
    configuration TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'ui', 'openapi_import')),
    team TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(team, name)
);

CREATE INDEX idx_filters_team ON filters(team);
CREATE INDEX idx_filters_type ON filters(filter_type);
CREATE INDEX idx_filters_team_name ON filters(team, name);
```

**Important**: Update the CHECK constraint to include ALL existing filter types plus your new one.

---

## Frontend Changes

### Step 10: Add TypeScript Types

**File**: `ui/src/lib/api/types.ts`

Add the filter type and configuration interface:

```typescript
// Add to FilterType union
export type FilterType =
    | 'header_mutation'
    | 'jwt_auth'
    // ... existing types ...
    | 'my_new_filter';

// Add configuration interface
export interface MyNewFilterConfig {
    some_field: string;
    optional_field?: number;
}

// Add to FilterConfig union
export type FilterConfig =
    | { type: 'header_mutation'; config: HeaderMutationFilterConfig }
    // ... existing types ...
    | { type: 'my_new_filter'; config: MyNewFilterConfig };
```

### Step 11: Create Filter Configuration Form Component

**File**: `ui/src/lib/components/filters/MyNewFilterConfigForm.svelte`

Create a Svelte component for the filter configuration form:

```svelte
<script lang="ts">
    import type { MyNewFilterConfig } from '$lib/api/types';

    interface Props {
        config: MyNewFilterConfig;
        onConfigChange: (config: MyNewFilterConfig) => void;
    }

    let { config, onConfigChange }: Props = $props();

    function handleFieldChange(field: keyof MyNewFilterConfig, value: string | number) {
        onConfigChange({
            ...config,
            [field]: value
        });
    }
</script>

<div class="space-y-4">
    <div>
        <label class="block text-sm font-medium text-gray-700 mb-1">
            Some Field <span class="text-red-500">*</span>
        </label>
        <input
            type="text"
            value={config.some_field}
            oninput={(e) => handleFieldChange('some_field', e.currentTarget.value)}
            class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
        />
    </div>
    <!-- Add more fields as needed -->
</div>
```

### Step 12: Update Create and Edit Pages

**File**: `ui/src/routes/(authenticated)/filters/create/+page.svelte`

1. Import the new config form component and types:

```typescript
import type { MyNewFilterConfig } from '$lib/api/types';
import MyNewFilterConfigForm from '$lib/components/filters/MyNewFilterConfigForm.svelte';
```

2. Add to FILTER_TYPE_INFO:

```typescript
const FILTER_TYPE_INFO: Record<FilterType, { label: string; description: string; attachmentPoints: string[]; available: boolean }> = {
    // ... existing entries ...
    my_new_filter: {
        label: 'My New Filter',
        description: 'Description of what this filter does',
        attachmentPoints: ['Routes', 'Listeners'],  // or ['Routes'] for route-only
        available: true
    }
};
```

3. Add state variable:

```typescript
let myNewFilterConfig = $state<MyNewFilterConfig>({
    some_field: '',
    optional_field: undefined
});
```

4. Add to `buildFilterConfig()`:

```typescript
if (filterType === 'my_new_filter') {
    return {
        type: 'my_new_filter',
        config: myNewFilterConfig
    };
}
```

5. Add validation function:

```typescript
function validateMyNewFilterConfig(): string | null {
    if (!myNewFilterConfig.some_field.trim()) {
        return 'Some field is required';
    }
    return null;
}
```

6. Add to `validateForm()`:

```typescript
if (filterType === 'my_new_filter') {
    return validateMyNewFilterConfig();
}
```

7. Add config change handler:

```typescript
function handleMyNewFilterConfigChange(config: MyNewFilterConfig) {
    myNewFilterConfig = config;
}
```

8. Add to template configuration section:

```svelte
{:else if filterType === 'my_new_filter'}
    <h2 class="text-lg font-semibold text-gray-900 mb-4">My New Filter Configuration</h2>
    <MyNewFilterConfigForm config={myNewFilterConfig} onConfigChange={handleMyNewFilterConfigChange} />
```

**File**: `ui/src/routes/(authenticated)/filters/[id]/edit/+page.svelte`

Apply the same changes as the create page, plus:

1. Add config loading in `loadFilter()`:

```typescript
} else if (data.config.type === 'my_new_filter') {
    myNewFilterConfig = data.config.config;
}
```

2. Update attachment points info section if needed:

```svelte
{#if filter.filterType === 'jwt_auth' || filter.filterType === 'local_rate_limit' || filter.filterType === 'mcp' || filter.filterType === 'my_new_filter'}
    <Badge variant="blue">Routes</Badge>
    <Badge variant="blue">Listeners</Badge>
```

3. Add description in attachment points text:

```svelte
{:else if filter.filterType === 'my_new_filter'}
    My New Filter filters can attach to routes or listeners (L7 HTTP filter)
```

---

## Testing Checklist

After implementing the filter, ensure you have tests for:

- [ ] Configuration validation (valid and invalid cases)
- [ ] Protobuf serialization (`to_any()`)
- [ ] Protobuf deserialization (`from_proto()`)
- [ ] Round-trip conversion (serialize then deserialize)
- [ ] Per-route configuration (if supported)
- [ ] Attachment point validation
- [ ] Integration with route/listener injection
- [ ] **API validation (type/config match)**
- [ ] **Service layer validation**
- [ ] **Database migration applies correctly**
- [ ] **UI create form works**
- [ ] **UI edit form loads and saves correctly**

Run the test suite:

```bash
# Backend
cargo fmt && cargo clippy && cargo test

# Frontend
cd ui && npm run check && npm run build
```

## File Summary

| File | Changes |
|------|---------|
| `src/domain/filter.rs` | Add `FilterType` variant, metadata, `FilterConfig` variant |
| `src/domain/mod.rs` | Export new types (if any) |
| `src/xds/filters/http/my_new_filter.rs` | Create new module with config structs |
| `src/xds/filters/http/mod.rs` | Export module, add `HttpFilterKind` and `HttpScopedConfig` variants |
| `src/xds/filters/conversion.rs` | Add match arms for unified conversion |
| `src/api/handlers/filters/validation.rs` | Add filter type/config validation match arm |
| `src/services/filter_service.rs` | Add filter type/config validation match arm |
| `migrations/YYYYMMDD_*.sql` | Add filter type to CHECK constraint |
| `ui/src/lib/api/types.ts` | Add TypeScript types |
| `ui/src/lib/components/filters/MyNewFilterConfigForm.svelte` | Create config form component |
| `ui/src/routes/(authenticated)/filters/create/+page.svelte` | Add filter to create page |
| `ui/src/routes/(authenticated)/filters/[id]/edit/+page.svelte` | Add filter to edit page |

## Framework Benefits

By following this framework:

1. **Single source of truth**: All filter metadata is in `filter_registry()`
2. **Automatic validation**: Attachment points and per-route behavior are checked automatically
3. **Consistent injection**: The unified conversion trait handles both listener and route injection
4. **Type safety**: Compile-time checks for filter capabilities
5. **Minimal boilerplate**: Most behavior is derived from metadata
6. **Full stack coverage**: Backend, database, and frontend all stay in sync

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
- `src/api/handlers/filters/validation.rs` - API validation
- `src/services/filter_service.rs` - Service validation
- `ui/src/lib/components/filters/HeaderMutationConfigForm.svelte` - UI form
