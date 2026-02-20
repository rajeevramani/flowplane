-- Create table for storing route metadata extracted from OpenAPI specs
-- Migration: 20260109000001_create_route_metadata_table.sql
-- Purpose: Store OpenAPI metadata for routes to enable MCP tool generation

CREATE TABLE IF NOT EXISTS route_metadata (
    id TEXT PRIMARY KEY,
    route_id TEXT NOT NULL,

    -- OpenAPI metadata
    operation_id TEXT,
    summary TEXT,
    description TEXT,
    tags TEXT,
    http_method TEXT,

    -- Request/Response schemas (from OpenAPI)
    request_body_schema TEXT,
    response_schemas TEXT,

    -- Learning enrichment (optional)
    learning_schema_id INTEGER,
    enriched_from_learning BOOLEAN DEFAULT FALSE,

    -- Source tracking
    source_type TEXT NOT NULL CHECK (source_type IN ('openapi', 'manual', 'learned')),
    confidence DOUBLE PRECISION,

    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    FOREIGN KEY (learning_schema_id) REFERENCES aggregated_api_schemas(id) ON DELETE SET NULL,
    UNIQUE(route_id)
);

CREATE INDEX IF NOT EXISTS idx_route_metadata_route_id ON route_metadata(route_id);
CREATE INDEX IF NOT EXISTS idx_route_metadata_operation_id ON route_metadata(operation_id);
CREATE INDEX IF NOT EXISTS idx_route_metadata_source_type ON route_metadata(source_type);
