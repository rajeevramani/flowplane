-- Create audit_log table for tracking configuration changes
-- Migration: 20241201000005_create_audit_log_table.sql

CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_type TEXT NOT NULL,  -- 'cluster', 'route', 'listener'
    resource_id TEXT NOT NULL,    -- ID of the affected resource
    resource_name TEXT NOT NULL,  -- Human-readable name
    action TEXT NOT NULL,         -- 'CREATE', 'UPDATE', 'DELETE'
    old_configuration TEXT,       -- Previous configuration (NULL for CREATE)
    new_configuration TEXT,       -- New configuration (NULL for DELETE)
    user_id TEXT,                 -- User who made the change (from JWT)
    client_ip TEXT,               -- IP address of the client
    user_agent TEXT,              -- User agent string
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Constraints
    CHECK (action IN ('CREATE', 'UPDATE', 'DELETE')),
    CHECK (resource_type IN ('cluster', 'route', 'listener', 'endpoint'))
);

-- Index for resource lookups
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);

-- Index for user activity tracking
CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_log(user_id, created_at);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(created_at);

-- Index for action filtering
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);