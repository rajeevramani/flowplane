-- Create table for storing aggregated API schemas from multiple observations
-- Migration: 20251019000002_create_aggregated_api_schemas_table.sql

-- Stores consensus schemas aggregated from multiple inferred_schemas observations
-- This represents the final, production-ready API documentation per endpoint
CREATE TABLE IF NOT EXISTS aggregated_api_schemas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team TEXT NOT NULL,

    -- API endpoint identification
    path TEXT NOT NULL,           -- e.g., "/api/users/{id}"
    http_method TEXT NOT NULL,    -- GET, POST, PUT, DELETE, etc.

    -- Versioning for schema evolution tracking
    version INTEGER NOT NULL DEFAULT 1,
    previous_version_id INTEGER,  -- NULL for first version

    -- Aggregated schemas (JSON Schema Draft 2020-12 format)
    request_schema TEXT,          -- NULL if no request body
    response_schemas TEXT,        -- JSON object mapping status codes to schemas: {"200": {...}, "404": {...}}

    -- Aggregation metadata
    sample_count INTEGER NOT NULL DEFAULT 0,        -- Total number of observations aggregated
    confidence_score REAL NOT NULL DEFAULT 0.0,     -- Confidence score (0.0 to 1.0)
    breaking_changes TEXT,                          -- JSON array of breaking change objects

    -- Timestamps
    first_observed DATETIME NOT NULL,               -- Earliest observation timestamp
    last_observed DATETIME NOT NULL,                -- Latest observation timestamp
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key to track schema evolution
    FOREIGN KEY (previous_version_id) REFERENCES aggregated_api_schemas(id),

    -- Ensure unique combination of team, path, method, and version
    UNIQUE(team, path, http_method, version)
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_team
    ON aggregated_api_schemas(team);

CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_method_path
    ON aggregated_api_schemas(http_method, path);

CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_team_method_path
    ON aggregated_api_schemas(team, http_method, path);

-- Index for finding latest version of an endpoint schema
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_team_method_path_version
    ON aggregated_api_schemas(team, http_method, path, version DESC);

-- Index for version tracking
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_previous_version
    ON aggregated_api_schemas(previous_version_id);

-- Index for confidence-based queries
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_confidence
    ON aggregated_api_schemas(confidence_score DESC);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_aggregated_schemas_created_at
    ON aggregated_api_schemas(created_at DESC);
