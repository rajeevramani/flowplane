-- Create tables for storing inferred API schemas from learning sessions
-- Migration: 20251019000001_create_inferred_schemas_table.sql

-- Stores aggregated schema information for API endpoints discovered during learning sessions
CREATE TABLE IF NOT EXISTS inferred_schemas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team TEXT NOT NULL,

    -- Learning session reference
    session_id TEXT NOT NULL,

    -- API endpoint identification
    http_method TEXT NOT NULL,  -- GET, POST, PUT, DELETE, etc.
    path_pattern TEXT NOT NULL, -- e.g., "/api/users/{id}"

    -- Schema storage (JSON)
    request_schema TEXT,  -- JSON Schema Draft 2020-12 format (NULL if no request body)
    response_schema TEXT, -- JSON Schema Draft 2020-12 format (NULL if no response body)
    response_status_code INTEGER, -- HTTP status code this schema applies to

    -- Statistics
    sample_count INTEGER NOT NULL DEFAULT 1, -- Number of samples this schema was inferred from
    confidence REAL NOT NULL DEFAULT 1.0,    -- Confidence score (0.0 to 1.0)

    -- Timestamps
    first_seen_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key constraint
    FOREIGN KEY (session_id) REFERENCES learning_sessions(id) ON DELETE CASCADE
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_inferred_schemas_team
    ON inferred_schemas(team);

CREATE INDEX IF NOT EXISTS idx_inferred_schemas_session_id
    ON inferred_schemas(session_id);

CREATE INDEX IF NOT EXISTS idx_inferred_schemas_method_path
    ON inferred_schemas(http_method, path_pattern);

CREATE INDEX IF NOT EXISTS idx_inferred_schemas_team_method_path
    ON inferred_schemas(team, http_method, path_pattern);

-- Composite index for common query (list schemas for a session)
CREATE INDEX IF NOT EXISTS idx_inferred_schemas_session_method_path
    ON inferred_schemas(session_id, http_method, path_pattern);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_inferred_schemas_created_at
    ON inferred_schemas(created_at DESC);
