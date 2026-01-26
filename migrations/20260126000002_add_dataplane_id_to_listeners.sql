-- Add dataplane_id column to listeners table
-- Migration: 20260126000002_add_dataplane_id_to_listeners.sql
-- Purpose: Link listeners to dataplanes for gateway_host resolution in MCP tool execution

-- Add dataplane_id column (nullable for backwards compatibility)
ALTER TABLE listeners ADD COLUMN dataplane_id TEXT REFERENCES dataplanes(id) ON DELETE SET NULL;

-- Index for efficient dataplane lookups
CREATE INDEX IF NOT EXISTS idx_listeners_dataplane_id ON listeners(dataplane_id);
