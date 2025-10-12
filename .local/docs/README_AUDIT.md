# README.md Accuracy Audit

**Date:** 2025-10-07
**Version:** v0.0.1
**Auditor:** Task 12 - Documentation Suite Creation

## Executive Summary

This audit identifies discrepancies between README.md documentation and the actual codebase implementation. Found **7 critical inaccuracies** and **8 missing features** that need immediate correction.

## 🔴 Critical Issues (Must Fix)

### 1. ❌ INCORRECT OpenAPI Import Endpoint (Line 105)

**README Claims:**
```bash
curl -sS \
  -X POST "http://127.0.0.1:8080/api/v1/gateways/openapi?name=example" \
  -H 'Content-Type: application/json' \
  --data-binary @openapi.json
```

**Actual Endpoint:** `/api/v1/api-definitions/from-openapi`

**Evidence:** `src/api/routes.rs:194`
```rust
.route("/api/v1/api-definitions/from-openapi", post(import_openapi_handler))
```

**Impact:** Users will get 404 errors following the README instructions.

**Fix Required:** Update all references from `/api/v1/gateways/openapi` to `/api/v1/api-definitions/from-openapi`

---

### 2. ⚠️ Environment Variable Name Discrepancy (Line 24)

**README Shows:** `FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db`

**Actual Variable:** `DATABASE_URL` (no FLOWPLANE_ prefix)

**Evidence:** `src/config/settings.rs:39`
```rust
let url = std::env::var("DATABASE_URL")
```

**Impact:** Control plane won't find database configuration.

**Fix Required:** Change to `DATABASE_URL=sqlite://./data/flowplane.db`

---

### 3. ❌ Missing Critical Environment Variable (Line 20-26)

**README Omits:** `FLOWPLANE_API_BIND_ADDRESS`

**Default Value:** `127.0.0.1` (only localhost access)

**Evidence:** `src/config/mod.rs:154`
```rust
let api_bind_address = std::env::var("FLOWPLANE_API_BIND_ADDRESS")
    .unwrap_or_else(|_| "127.0.0.1".to_string());
```

**Impact:** Docker deployments and remote access won't work without this setting.

**Fix Required:** Add to quick start example:
```bash
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
```

---

### 4. 📝 XDS Port Default Mismatch (Line 20)

**README Example:** `FLOWPLANE_XDS_PORT=18003`

**Code Default:** `18000`

**Evidence:** `src/config/mod.rs:95`
```rust
std::env::var("FLOWPLANE_XDS_PORT").unwrap_or_else(|_| "18000".to_string());
```

**Impact:** Confusing for users - example doesn't match documented default.

**Fix Required:** Use `18000` in examples for consistency, or explain why using non-default.

---

### 5. 🦀 Outdated Rust Version (Line 12)

**README Claims:** "Rust toolchain (1.75+ recommended)"

**Actual Requirement:** Rust 1.89+ (based on Docker images and local dev)

**Evidence:**
- `Dockerfile:2` - `FROM rust:1.89-slim`
- Local rustc: `1.89.0`
- Release v0.0.1 built with 1.89

**Impact:** Users may experience build issues with older Rust versions.

**Fix Required:** Update to "Rust toolchain (1.89+ required)"

---

### 6. ❌ Missing Bootstrap Token Display Information

**README States:** "On first launch a bootstrap admin token is emitted once in the logs" (Line 66)

**Actual Behavior:** Token displayed in **prominent ASCII art banner** (v0.0.1 feature)

**Evidence:** `src/openapi/defaults.rs:52-77`
```rust
eprintln!("\n{}", "=".repeat(80));
eprintln!("🔐 BOOTSTRAP ADMIN TOKEN GENERATED");
// ... full banner with security warnings
```

**Impact:** Undocumented feature; users don't know to expect prominent display.

**Fix Required:** Add note about prominent banner display and extraction commands.

---

### 7. 📍 API Endpoint Path Inconsistency (Line 28)

**README Shows:** `/swagger-ui` (correct) but also mentions `/api-docs/openapi.json`

**Actual Swagger URL:** `/swagger-ui` (trailing slash optional)

**Actual OpenAPI JSON:** `/api-docs/openapi.json` ✅

**Status:** Correct, but should clarify both work: `/swagger-ui` and `/swagger-ui/`

---

## ⚠️ Missing Documentation

### Environment Variables Not Documented

The following environment variables are used in code but **not mentioned in README:**

1. **`FLOWPLANE_ENABLE_METRICS`** - Enable Prometheus metrics (default: true)
   - Source: Release notes mention this

2. **`FLOWPLANE_ENABLE_TRACING`** - Enable OpenTelemetry tracing (default: false)
   - Source: Release notes mention this

3. **`FLOWPLANE_TOKEN`** - CLI authentication token
   - Evidence: Used by CLI for API access

4. **`FLOWPLANE_BASE_URL`** - CLI base URL configuration
   - Evidence: `grep` output shows usage

5. **`FLOWPLANE_LOG_LEVEL`** - Logging verbosity
   - Evidence: `grep` output shows usage

6. **`FLOWPLANE_SERVICE_NAME`** - Service identification for tracing
   - Evidence: `grep` output shows usage

7. **`FLOWPLANE_JAEGER_ENDPOINT`** - Jaeger tracing endpoint
   - Evidence: `grep` output shows usage

8. **`RUST_LOG`** - Standard Rust logging (should document alongside FLOWPLANE_LOG_LEVEL)
   - Used extensively in Docker and examples

---

## 📊 API Endpoints Accuracy Check

### ✅ Correct in README

- `/api/v1/clusters` (GET, POST, GET /{name}, PUT /{name}, DELETE /{name})
- `/api/v1/routes` (GET, POST, GET /{name}, PUT /{name}, DELETE /{name})
- `/api/v1/listeners` (GET, POST, GET /{name}, PUT /{name}, DELETE /{name})
- `/api/v1/tokens` (POST, GET, GET /{id}, PATCH /{id}, DELETE /{id})

### ❌ Missing from README

The following endpoints exist but are **not documented:**

1. **`POST /api/v1/tokens/{id}/rotate`** - Rotate token
2. **`POST /api/v1/api-definitions`** - Create BFF API definition
3. **`GET /api/v1/api-definitions`** - List API definitions
4. **`GET /api/v1/api-definitions/{id}`** - Get API definition
5. **`GET /api/v1/api-definitions/{id}/bootstrap`** - Get bootstrap config
6. **`POST /api/v1/api-definitions/{id}/routes`** - Append route to API

---

## 🎯 Quick Start Example Issues

### Current Example (Lines 19-26)

```bash
FLOWPLANE_XDS_PORT=18003 \
FLOWPLANE_CLUSTER_NAME=my_cluster \
FLOWPLANE_BACKEND_PORT=9090 \
FLOWPLANE_LISTENER_PORT=8080 \
FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db \
cargo run --bin flowplane
```

### Problems

1. ❌ Uses `FLOWPLANE_DATABASE_URL` instead of `DATABASE_URL`
2. ⚠️ XDS port 18003 doesn't match default 18000
3. ❌ Missing `FLOWPLANE_API_BIND_ADDRESS` for remote access
4. ❌ These legacy vars (`CLUSTER_NAME`, `BACKEND_PORT`, `LISTENER_PORT`) are for **simple development mode** only - not needed for production

### Recommended Quick Start

```bash
# Minimal production start
DATABASE_URL=sqlite://./data/flowplane.db \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
cargo run --bin flowplane

# With custom ports
DATABASE_URL=sqlite://./data/flowplane.db \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
FLOWPLANE_API_PORT=8080 \
FLOWPLANE_XDS_PORT=50051 \
cargo run --bin flowplane
```

---

## 📝 Configuration Defaults Verification

| Variable | README | Code Default | Status |
|----------|--------|--------------|--------|
| `FLOWPLANE_API_PORT` | 8080 | 8080 | ✅ Correct |
| `FLOWPLANE_API_BIND_ADDRESS` | Not documented | `127.0.0.1` | ❌ Missing |
| `FLOWPLANE_XDS_PORT` | 18003 (example) | 18000 | ⚠️ Mismatch |
| `FLOWPLANE_XDS_BIND_ADDRESS` | Not documented | `0.0.0.0` | ❌ Missing |
| `DEFAULT_GATEWAY_PORT` | 10000 | 10000 | ✅ Correct |
| Database URL prefix | `FLOWPLANE_DATABASE_URL` | `DATABASE_URL` | ❌ Wrong |

---

## 🔧 Recommended Actions

### Priority 1 (Breaking Issues - Fix Immediately)

1. **Fix OpenAPI endpoint path** (Line 105)
   - Change `/api/v1/gateways/openapi` → `/api/v1/api-definitions/from-openapi`

2. **Fix DATABASE_URL environment variable** (Line 24)
   - Remove `FLOWPLANE_` prefix

3. **Add FLOWPLANE_API_BIND_ADDRESS** to quick start
   - Required for Docker and production deployments

### Priority 2 (Accuracy Issues)

4. Update Rust version requirement to 1.89+
5. Fix XDS port default mismatch (use 18000 or explain)
6. Document bootstrap token banner display
7. Add missing API endpoints section

### Priority 3 (Completeness)

8. Add comprehensive environment variables reference
9. Clarify which variables are dev-only vs. production
10. Add troubleshooting section for common config issues

---

## 📚 Additional Documentation Gaps

1. **Docker Quick Start** - ✅ Exists in README-DOCKER.md (added in v0.0.1)
2. **CLI Usage** - ✅ Mentioned but could link to docs/token-management.md
3. **Filter Configuration** - ✅ Links to docs/filters.md
4. **API Definitions (BFF)** - ❌ Not explained (new in Platform API work)
5. **Listener Isolation** - ❌ Not explained (query param `listener=`)
6. **Rate Limiting** - ✅ Documented (Line 114-120)
7. **Token Scopes** - ✅ Documented (Line 86-87)

---

## ✅ What's Correct in README

- Overview and architecture description
- TLS/mTLS configuration
- Authentication flow
- Rate limiting explanation
- Documentation map (lines 122-132)
- Default gateway resources explanation
- Swagger UI location
- Bruno workspace mention

---

## 🎓 Lessons Learned

1. **README drift is real** - Quick start examples should be tested in CI
2. **Environment variables need centralized docs** - Consider a dedicated ENV_VARS.md
3. **API changes need README updates** - `/gateways/openapi` → `/api-definitions/from-openapi`
4. **Version requirements matter** - Keep Rust version current with Docker images
5. **Hidden features** - Bootstrap token banner should be celebrated, not buried

---

## 📋 Next Steps for Task 12

1. Apply Priority 1 fixes to README.md
2. Create comprehensive environment variables reference
3. Update quick start examples with tested commands
4. Add API endpoints reference section
5. Consider splitting README into:
   - Quick start (README.md)
   - Configuration reference (docs/configuration.md)
   - API reference (point to Swagger UI + docs/api.md)
