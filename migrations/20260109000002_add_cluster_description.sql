-- Add description and tags columns to clusters table
-- Migration: 20260109000002_add_cluster_description.sql
-- Purpose: Enable better MCP control plane tool responses

ALTER TABLE clusters ADD COLUMN description TEXT;
ALTER TABLE clusters ADD COLUMN tags TEXT;  -- JSON array
