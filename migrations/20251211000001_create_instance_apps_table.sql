-- Create instance_apps table for feature enablement
-- Migration: 20251211000001_create_instance_apps_table.sql
--
-- This table tracks optional "app" features within the control plane.
-- Apps are instance-wide features that can be enabled/disabled by admins.
-- Each app can have its own configuration stored as JSON.
--
-- Examples of apps:
-- - stats_dashboard: Envoy stats dashboard feature
-- - alerting: Future alerting integration
-- - slo_tracking: Future SLO tracking feature

CREATE TABLE IF NOT EXISTS instance_apps (
    app_id TEXT PRIMARY KEY,                       -- App identifier (e.g., "stats_dashboard")
    enabled INTEGER NOT NULL DEFAULT 0,            -- 0=disabled, 1=enabled
    config TEXT,                                   -- JSON configuration for the app
    enabled_by TEXT,                               -- User/entity who enabled/disabled (audit only)
    enabled_at TIMESTAMPTZ,                           -- When the app was enabled
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Index for quick enabled checks
CREATE INDEX IF NOT EXISTS idx_instance_apps_enabled ON instance_apps(enabled);
