-- Fix type mismatches between DDL and Rust types:
-- 1. route_metadata.learning_schema_id: INTEGER (INT4) -> BIGINT (INT8) for Rust i64
-- 2. mcp_tools.learning_schema_id: INTEGER (INT4) -> BIGINT (INT8) for Rust i64
-- 3. mcp_tools.enabled: INTEGER (0/1) -> BOOLEAN for Rust bool

ALTER TABLE route_metadata ALTER COLUMN learning_schema_id TYPE BIGINT;

ALTER TABLE mcp_tools ALTER COLUMN learned_schema_id TYPE BIGINT;

-- Convert enabled from INTEGER (0/1) to BOOLEAN
ALTER TABLE mcp_tools ALTER COLUMN enabled DROP DEFAULT;
ALTER TABLE mcp_tools DROP CONSTRAINT IF EXISTS mcp_tools_enabled_check;
ALTER TABLE mcp_tools ALTER COLUMN enabled TYPE BOOLEAN USING (enabled::int != 0);
ALTER TABLE mcp_tools ALTER COLUMN enabled SET DEFAULT TRUE;
