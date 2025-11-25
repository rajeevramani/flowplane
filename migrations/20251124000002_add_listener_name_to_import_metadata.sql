-- Add listener_name column to import_metadata table
-- This stores the listener associated with an import (created or existing)
-- Migration: 20251124000002_add_listener_name_to_import_metadata.sql

ALTER TABLE import_metadata ADD COLUMN listener_name TEXT;

-- Create index for listener-based queries
CREATE INDEX IF NOT EXISTS idx_import_metadata_listener_name ON import_metadata(listener_name);
