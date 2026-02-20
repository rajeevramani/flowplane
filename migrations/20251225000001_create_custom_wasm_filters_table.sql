-- Create custom_wasm_filters table for storing user-uploaded WASM filter binaries
-- Each team can upload their own custom WASM filters with configuration schemas

CREATE TABLE IF NOT EXISTS custom_wasm_filters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    display_name TEXT NOT NULL,
    description TEXT,
    -- WASM binary stored as BLOB
    wasm_binary BYTEA NOT NULL,
    -- SHA256 hash for integrity verification
    wasm_sha256 TEXT NOT NULL,
    -- Size in bytes for display and validation
    wasm_size_bytes BIGINT NOT NULL,
    -- JSON Schema for filter configuration validation
    config_schema TEXT NOT NULL,
    -- Optional per-route config schema
    per_route_config_schema TEXT,
    -- UI hints for form generation (optional JSON)
    ui_hints TEXT,
    -- Attachment points as JSON array: ["listener", "route"]
    attachment_points TEXT NOT NULL DEFAULT '["listener", "route"]',
    -- WASM runtime (e.g., envoy.wasm.runtime.v8)
    runtime TEXT NOT NULL DEFAULT 'envoy.wasm.runtime.v8',
    -- Failure policy: FAIL_CLOSED or FAIL_OPEN
    failure_policy TEXT NOT NULL DEFAULT 'FAIL_CLOSED',
    -- Version for optimistic locking
    version BIGINT NOT NULL DEFAULT 1,
    -- Team ownership for multi-tenancy
    team TEXT NOT NULL,
    -- User who created this filter
    created_by TEXT,
    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (team) REFERENCES teams(name) ON DELETE RESTRICT,
    UNIQUE(team, name)
);

-- Index for team-scoped queries (most common access pattern)
CREATE INDEX idx_custom_wasm_filters_team ON custom_wasm_filters(team);

-- Index for team + name lookups
CREATE INDEX idx_custom_wasm_filters_team_name ON custom_wasm_filters(team, name);

-- Index for change detection
CREATE INDEX idx_custom_wasm_filters_updated_at ON custom_wasm_filters(updated_at);
