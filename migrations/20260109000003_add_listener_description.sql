-- Add description and tags columns to listeners table
-- Migration: 20260109000003_add_listener_description.sql
-- Purpose: Enable better MCP control plane tool responses

ALTER TABLE listeners ADD COLUMN description TEXT;
ALTER TABLE listeners ADD COLUMN tags TEXT;  -- JSON array
