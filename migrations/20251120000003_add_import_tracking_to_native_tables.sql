-- Add import tracking columns to native resource tables
-- Migration: 20251120000003_add_import_tracking_to_native_tables.sql

-- Add import_id to routes table
ALTER TABLE routes ADD COLUMN import_id TEXT
    REFERENCES import_metadata(id) ON DELETE CASCADE;

-- Add route_order for deterministic Envoy route ordering
ALTER TABLE routes ADD COLUMN route_order INTEGER DEFAULT 0;

-- Add headers column for header-based routing (JSON)
ALTER TABLE routes ADD COLUMN headers TEXT;

-- Add import_id to clusters table
ALTER TABLE clusters ADD COLUMN import_id TEXT
    REFERENCES import_metadata(id) ON DELETE SET NULL;

-- Add import_id to listeners table
ALTER TABLE listeners ADD COLUMN import_id TEXT
    REFERENCES import_metadata(id) ON DELETE CASCADE;

-- Create indexes for import_id columns
CREATE INDEX IF NOT EXISTS idx_routes_import_id ON routes(import_id);
CREATE INDEX IF NOT EXISTS idx_clusters_import_id ON clusters(import_id);
CREATE INDEX IF NOT EXISTS idx_listeners_import_id ON listeners(import_id);

-- Create index for route_order to optimize sorting
CREATE INDEX IF NOT EXISTS idx_routes_order ON routes(route_order);
