-- Create learning_sessions table for API schema learning lifecycle management
-- Migration: 20251018000001_create_learning_sessions_table.sql

CREATE TABLE IF NOT EXISTS learning_sessions (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,

    -- What to learn
    route_pattern TEXT NOT NULL,
    cluster_name TEXT,
    http_methods TEXT, -- JSON array: ["GET", "POST"] or NULL for all

    -- Lifecycle tracking
    status TEXT NOT NULL DEFAULT 'pending',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at DATETIME,
    ends_at DATETIME,
    completed_at DATETIME,

    -- Progress tracking
    target_sample_count INTEGER NOT NULL,
    current_sample_count INTEGER NOT NULL DEFAULT 0,

    -- Metadata
    triggered_by TEXT,
    deployment_version TEXT,
    configuration_snapshot TEXT, -- JSON snapshot of config
    error_message TEXT,

    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_learning_sessions_team
    ON learning_sessions(team);

CREATE INDEX IF NOT EXISTS idx_learning_sessions_status
    ON learning_sessions(status);

CREATE INDEX IF NOT EXISTS idx_learning_sessions_team_status
    ON learning_sessions(team, status);

CREATE INDEX IF NOT EXISTS idx_learning_sessions_route_pattern
    ON learning_sessions(route_pattern);

CREATE INDEX IF NOT EXISTS idx_learning_sessions_created_at
    ON learning_sessions(created_at);

-- Composite index for common query patterns (list active sessions by team)
CREATE INDEX IF NOT EXISTS idx_learning_sessions_team_status_created
    ON learning_sessions(team, status, created_at DESC);
