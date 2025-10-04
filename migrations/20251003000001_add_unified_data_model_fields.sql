-- Add source tracking and foreign key relationships for unified data model
-- Migration: 20251003000001_add_unified_data_model_fields.sql

-- Add source column to listeners table
ALTER TABLE listeners ADD COLUMN source TEXT NOT NULL DEFAULT 'native_api'
    CHECK (source IN ('native_api', 'platform_api'));

-- Add source column to routes table
ALTER TABLE routes ADD COLUMN source TEXT NOT NULL DEFAULT 'native_api'
    CHECK (source IN ('native_api', 'platform_api'));

-- Add source column to clusters table
ALTER TABLE clusters ADD COLUMN source TEXT NOT NULL DEFAULT 'native_api'
    CHECK (source IN ('native_api', 'platform_api'));

-- Add foreign key columns to api_definitions for generated native resources
ALTER TABLE api_definitions ADD COLUMN generated_listener_id TEXT
    REFERENCES listeners(id) ON DELETE SET NULL;

-- Add foreign key columns to api_routes for generated native resources
ALTER TABLE api_routes ADD COLUMN generated_route_id TEXT
    REFERENCES routes(id) ON DELETE SET NULL;

ALTER TABLE api_routes ADD COLUMN generated_cluster_id TEXT
    REFERENCES clusters(id) ON DELETE SET NULL;

-- Add filter_config JSON column to api_routes for HTTP filter configurations
ALTER TABLE api_routes ADD COLUMN filter_config TEXT;

-- Create indexes for the new foreign key relationships
CREATE INDEX IF NOT EXISTS idx_api_definitions_listener ON api_definitions(generated_listener_id);
CREATE INDEX IF NOT EXISTS idx_api_routes_route ON api_routes(generated_route_id);
CREATE INDEX IF NOT EXISTS idx_api_routes_cluster ON api_routes(generated_cluster_id);

-- Create indexes for source column filtering
CREATE INDEX IF NOT EXISTS idx_listeners_source ON listeners(source);
CREATE INDEX IF NOT EXISTS idx_routes_source ON routes(source);
CREATE INDEX IF NOT EXISTS idx_clusters_source ON clusters(source);
