-- Add request/response header columns to schema tables for learning pipeline
-- Migration: 20260225000002_add_header_columns_to_schema_tables.sql
--
-- Headers are stored as JSON arrays of objects:
--   [{"name": "Authorization", "example": "Bearer ***"}, {"name": "Content-Type", "example": "application/json"}]
-- Sensitive header values (Authorization, Cookie, X-Api-Key) are redacted at capture time.

-- Add header columns to inferred_schemas (per-sample headers)
ALTER TABLE inferred_schemas ADD COLUMN IF NOT EXISTS request_headers TEXT;
ALTER TABLE inferred_schemas ADD COLUMN IF NOT EXISTS response_headers TEXT;

-- Add header columns to aggregated_api_schemas (merged across samples)
ALTER TABLE aggregated_api_schemas ADD COLUMN IF NOT EXISTS request_headers TEXT;
ALTER TABLE aggregated_api_schemas ADD COLUMN IF NOT EXISTS response_headers TEXT;
