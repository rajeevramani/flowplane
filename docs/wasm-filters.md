# Custom WASM Filters Guide

This guide covers creating, uploading, and using custom WebAssembly (WASM) filters in Flowplane.

## Overview

Custom WASM filters allow you to extend Envoy's request processing with your own logic. Flowplane supports:

- Uploading WASM binaries with configuration schemas
- Team-scoped filter management
- Automatic injection into Envoy listener configurations
- Per-route configuration overrides

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    WASM Filter Lifecycle                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. Develop        2. Upload           3. Create Instance       │
│  ┌──────────┐     ┌──────────────┐    ┌───────────────────┐     │
│  │  Rust    │ ──▶ │  POST        │ ──▶│  Create filter    │     │
│  │  WASM    │     │  /custom-    │    │  with type:       │     │
│  │  Module  │     │  filters     │    │  custom_wasm_xxx  │     │
│  └──────────┘     └──────────────┘    └───────────────────┘     │
│                                                │                │
│                                                ▼                │
│  4. xDS Generation                    5. Envoy Execution        │
│  ┌───────────────────────────┐       ┌───────────────────────┐  │
│  │  Resolve WASM binary      │       │  Load inline bytes    │  │
│  │  Inject into HCM chain    │  ──▶  │  Execute with config  │  │
│  │  Base64 encode binary     │       │  Process requests     │  │
│  └───────────────────────────┘       └───────────────────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Developing a WASM Filter

### Prerequisites

- Rust toolchain with `wasm32-unknown-unknown` target
- `proxy-wasm` SDK

### Project Setup

```bash
# Create new Rust library project
cargo new --lib my-filter
cd my-filter

# Add wasm32 target
rustup target add wasm32-unknown-unknown
```

### Cargo.toml

```toml
[package]
name = "my-filter"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
proxy-wasm = "0.2"
log = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

[profile.release]
lto = true
opt-level = "s"
strip = true
```

### Example Filter: Add Header

```rust
// src/lib.rs
use proxy_wasm::traits::*;
use proxy_wasm::types::*;
use serde::Deserialize;

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_root_context(|_| -> Box<dyn RootContext> {
        Box::new(AddHeaderRoot::default())
    });
}}

#[derive(Default)]
struct AddHeaderRoot {
    config: FilterConfig,
}

#[derive(Default, Deserialize)]
struct FilterConfig {
    header_name: String,
    header_value: String,
}

impl Context for AddHeaderRoot {}

impl RootContext for AddHeaderRoot {
    fn on_configure(&mut self, _plugin_configuration_size: usize) -> bool {
        if let Some(config_bytes) = self.get_plugin_configuration() {
            match serde_json::from_slice::<FilterConfig>(&config_bytes) {
                Ok(config) => {
                    self.config = config;
                    return true;
                }
                Err(e) => {
                    log::error!("Failed to parse config: {:?}", e);
                    return false;
                }
            }
        }
        true
    }

    fn create_http_context(&self, _context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(AddHeaderFilter {
            header_name: self.config.header_name.clone(),
            header_value: self.config.header_value.clone(),
        }))
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

struct AddHeaderFilter {
    header_name: String,
    header_value: String,
}

impl Context for AddHeaderFilter {}

impl HttpContext for AddHeaderFilter {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.add_http_request_header(&self.header_name, &self.header_value);
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.add_http_response_header("x-processed-by", "wasm-filter");
        Action::Continue
    }
}
```

### Build

```bash
# Build for release
cargo build --target wasm32-unknown-unknown --release

# Output: target/wasm32-unknown-unknown/release/my_filter.wasm

# Optional: Optimize with wasm-opt (requires binaryen)
wasm-opt -O3 target/wasm32-unknown-unknown/release/my_filter.wasm \
  -o my_filter_optimized.wasm
```

## Uploading to Flowplane

### API Endpoint

```
POST /api/v1/teams/{team}/custom-filters
```

### Request Format

```json
{
  "name": "add-header-filter",
  "display_name": "Add Header Filter",
  "description": "Adds custom headers to HTTP requests and responses",
  "wasm_binary_base64": "<base64-encoded-wasm-binary>",
  "config_schema": {
    "type": "object",
    "properties": {
      "header_name": {
        "type": "string",
        "title": "Header Name",
        "description": "Name of the header to add"
      },
      "header_value": {
        "type": "string",
        "title": "Header Value",
        "description": "Value of the header to add"
      }
    },
    "required": ["header_name", "header_value"]
  },
  "per_route_config_schema": {
    "type": "object",
    "properties": {
      "header_value": {
        "type": "string",
        "title": "Override Header Value",
        "description": "Per-route header value override"
      }
    }
  },
  "attachment_points": ["listener", "route"],
  "runtime": "envoy.wasm.runtime.v8",
  "failure_policy": "FAIL_CLOSED"
}
```

### Upload Script

```bash
#!/bin/bash

# Base64 encode the WASM binary
WASM_BASE64=$(base64 -i my_filter.wasm)

# Upload to Flowplane
curl -X POST "http://localhost:8080/api/v1/teams/my-team/custom-filters" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"name\": \"add-header-filter\",
    \"display_name\": \"Add Header Filter\",
    \"description\": \"Adds custom headers to requests\",
    \"wasm_binary_base64\": \"$WASM_BASE64\",
    \"config_schema\": {
      \"type\": \"object\",
      \"properties\": {
        \"header_name\": {\"type\": \"string\"},
        \"header_value\": {\"type\": \"string\"}
      },
      \"required\": [\"header_name\", \"header_value\"]
    },
    \"runtime\": \"envoy.wasm.runtime.v8\",
    \"failure_policy\": \"FAIL_CLOSED\"
  }"
```

### Response

```json
{
  "id": "cf_abc123",
  "name": "add-header-filter",
  "display_name": "Add Header Filter",
  "description": "Adds custom headers to requests",
  "wasm_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "wasm_size_bytes": 45678,
  "config_schema": {...},
  "attachment_points": ["listener", "route"],
  "runtime": "envoy.wasm.runtime.v8",
  "failure_policy": "FAIL_CLOSED",
  "version": 1,
  "team": "my-team",
  "filter_type": "custom_wasm_cf_abc123",
  "created_at": "2025-01-31T10:00:00Z",
  "updated_at": "2025-01-31T10:00:00Z"
}
```

Note the `filter_type` field - use this when creating filter instances.

## Creating Filter Instances

After uploading, create filter instances using the `filter_type` from the response.

### Via API

```bash
curl -X POST "http://localhost:8080/api/v1/filters" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "my-team",
    "name": "my-add-header-instance",
    "filter_type": "custom_wasm_cf_abc123",
    "config": {
      "header_name": "X-Custom-Header",
      "header_value": "Hello from WASM"
    }
  }'
```

### Attach to Listener

```bash
curl -X POST "http://localhost:8080/api/v1/listeners/{listener_id}/filters/{filter_id}" \
  -H "Authorization: Bearer $TOKEN"
```

## Configuration Schema

### JSON Schema Format

The `config_schema` defines what configuration your filter accepts:

```json
{
  "type": "object",
  "title": "Filter Configuration",
  "description": "Configuration for the custom WASM filter",
  "properties": {
    "header_name": {
      "type": "string",
      "title": "Header Name",
      "description": "Name of the header to add",
      "minLength": 1,
      "maxLength": 100
    },
    "header_value": {
      "type": "string",
      "title": "Header Value",
      "description": "Value of the header"
    },
    "enabled": {
      "type": "boolean",
      "title": "Enabled",
      "default": true
    },
    "priority": {
      "type": "integer",
      "title": "Priority",
      "minimum": 0,
      "maximum": 100,
      "default": 50
    }
  },
  "required": ["header_name", "header_value"]
}
```

### Per-Route Configuration

For filters that support per-route overrides:

```json
{
  "per_route_config_schema": {
    "type": "object",
    "properties": {
      "header_value": {
        "type": "string",
        "title": "Override Value",
        "description": "Override the header value for this route"
      },
      "disabled": {
        "type": "boolean",
        "title": "Disable",
        "description": "Disable this filter for this route"
      }
    }
  }
}
```

### UI Hints

Optional UI hints for form generation:

```json
{
  "ui_hints": {
    "order": ["header_name", "header_value", "enabled"],
    "header_name": {
      "placeholder": "X-Custom-Header"
    },
    "header_value": {
      "multiline": true
    }
  }
}
```

## Attachment Points

| Point | Description |
|-------|-------------|
| `listener` | Injected into HTTP Connection Manager filter chain |
| `route` | Per-route configuration override |
| `cluster` | Cluster-level configuration (future support) |

## Runtime Options

| Runtime | Description |
|---------|-------------|
| `envoy.wasm.runtime.v8` | V8 JavaScript engine (default, recommended) |
| `envoy.wasm.runtime.wamr` | WebAssembly Micro Runtime |
| `envoy.wasm.runtime.wasmtime` | Wasmtime runtime |

## Failure Policies

| Policy | Behavior |
|--------|----------|
| `FAIL_CLOSED` | Reject requests if WASM fails (default, safer) |
| `FAIL_OPEN` | Allow requests to continue if WASM fails |

## API Reference

### List Custom Filters

```bash
GET /api/v1/teams/{team}/custom-filters
```

### Get Custom Filter

```bash
GET /api/v1/teams/{team}/custom-filters/{id}
```

### Update Custom Filter

```bash
PUT /api/v1/teams/{team}/custom-filters/{id}
```

Note: Cannot update WASM binary after upload. Create a new version instead.

### Delete Custom Filter

```bash
DELETE /api/v1/teams/{team}/custom-filters/{id}
```

Note: Cannot delete if filter instances exist.

### Download WASM Binary

```bash
GET /api/v1/teams/{team}/custom-filters/{id}/download
```

## Troubleshooting

### WASM Validation Failed

**Error**: "Invalid WASM binary: missing magic bytes"

**Cause**: The uploaded file is not a valid WASM module.

**Solution**: Ensure you're building with `--target wasm32-unknown-unknown` and the file has `.wasm` extension.

### Configuration Parsing Failed

**Error**: "Failed to parse config"

**Cause**: Configuration doesn't match the schema.

**Solution**:
1. Check the `config_schema` matches your filter's expected configuration
2. Ensure all required fields are provided
3. Check types match (string vs number)

### Filter Not Executing

**Symptoms**: Requests pass through without filter effects.

**Troubleshooting**:
1. Check Envoy admin (`/config_dump`) for filter in HCM chain
2. Verify filter is attached to the correct listener
3. Check Envoy logs for WASM errors
4. Verify `failure_policy` setting

### WASM Runtime Error

**Error**: Envoy logs show WASM execution errors.

**Common causes**:
- Memory allocation issues
- Panic in Rust code
- Invalid host function calls

**Solutions**:
1. Add error handling to your filter
2. Use `log::error!` for debugging
3. Check memory usage in filter

### Binary Too Large

**Symptom**: Slow xDS updates or memory issues.

**Solutions**:
1. Use `wasm-opt -O3` to optimize
2. Enable LTO in Cargo.toml
3. Use `opt-level = "s"` for size
4. Strip debug info: `strip = true`

## Best Practices

### Development

1. **Test locally** with Envoy before uploading
2. **Use logging** for debugging (`proxy_wasm::set_log_level`)
3. **Handle errors** gracefully - don't panic
4. **Validate configuration** in `on_configure`

### Configuration Schema

1. **Provide defaults** where appropriate
2. **Add descriptions** for all properties
3. **Use validation** (minLength, minimum, etc.)
4. **Keep it simple** - complex schemas are hard to use

### Deployment

1. **Version your filters** - use versioned names
2. **Test in staging** before production
3. **Use FAIL_CLOSED** for security-critical filters
4. **Monitor filter performance** via Envoy stats

## Example Filters

### Request Logger

Logs request details to Envoy logs:

```rust
impl HttpContext for LoggerFilter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        let path = self.get_http_request_header(":path").unwrap_or_default();
        let method = self.get_http_request_header(":method").unwrap_or_default();
        log::info!("Request: {} {}", method, path);
        Action::Continue
    }
}
```

### Rate Limiter

Simple in-memory rate limiting:

```rust
impl HttpContext for RateLimiter {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        let client_ip = self.get_http_request_header("x-forwarded-for")
            .unwrap_or_default();

        if self.is_rate_limited(&client_ip) {
            self.send_http_response(429, vec![], Some(b"Rate limited"));
            return Action::Pause;
        }

        self.increment_counter(&client_ip);
        Action::Continue
    }
}
```

### Request Transformer

Modify request headers based on path:

```rust
impl HttpContext for Transformer {
    fn on_http_request_headers(&mut self, _: usize, _: bool) -> Action {
        let path = self.get_http_request_header(":path").unwrap_or_default();

        if path.starts_with("/api/v2") {
            self.set_http_request_header("x-api-version", Some("2"));
        }

        Action::Continue
    }
}
```

## Next Steps

- Review [filters documentation](filters.md)
- Learn about [filter attachment](adding-new-filter.md)
- Explore [routing cookbook](routing-cookbook.md)
