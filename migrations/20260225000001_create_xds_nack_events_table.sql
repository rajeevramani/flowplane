-- Create table for xDS NACK events
-- Migration: 20260225000001_create_xds_nack_events_table.sql
-- Purpose: Persist NACK events from Envoy for observability and troubleshooting

CREATE TABLE IF NOT EXISTS xds_nack_events (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,                              -- Team name for isolation
    dataplane_name TEXT NOT NULL,                    -- Name of the dataplane that NACKed
    type_url TEXT NOT NULL,                          -- xDS resource type (e.g. type.googleapis.com/envoy.config.cluster.v3.Cluster)
    version_rejected TEXT NOT NULL,                  -- Config version that was rejected
    nonce TEXT NOT NULL,                             -- Response nonce from the rejected config
    error_code BIGINT NOT NULL,                      -- gRPC error code
    error_message TEXT NOT NULL,                     -- Error details from Envoy
    node_id TEXT,                                    -- Envoy node identifier
    resource_names TEXT,                             -- JSON array of rejected resource names
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for query performance
CREATE INDEX IF NOT EXISTS idx_xds_nack_events_team_created_at ON xds_nack_events(team, created_at);
CREATE INDEX IF NOT EXISTS idx_xds_nack_events_team_dataplane ON xds_nack_events(team, dataplane_name);
CREATE INDEX IF NOT EXISTS idx_xds_nack_events_team_type_url ON xds_nack_events(team, type_url);
