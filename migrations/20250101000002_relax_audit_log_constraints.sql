-- Adjust audit_log constraints to support auth events
-- Migration: 20250101000002_relax_audit_log_constraints.sql

PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS audit_log_tmp (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    resource_name TEXT,
    action TEXT NOT NULL,
    old_configuration TEXT,
    new_configuration TEXT,
    user_id TEXT,
    client_ip TEXT,
    user_agent TEXT,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK (length(trim(resource_type)) > 0),
    CHECK (length(trim(action)) > 0)
);

INSERT INTO audit_log_tmp (
    id,
    resource_type,
    resource_id,
    resource_name,
    action,
    old_configuration,
    new_configuration,
    user_id,
    client_ip,
    user_agent,
    created_at
)
SELECT
    id,
    resource_type,
    resource_id,
    resource_name,
    action,
    old_configuration,
    new_configuration,
    user_id,
    client_ip,
    user_agent,
    created_at
FROM audit_log;

DROP TABLE audit_log;
ALTER TABLE audit_log_tmp RENAME TO audit_log;

CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);
CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_log(user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(created_at);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);

PRAGMA foreign_keys = ON;
