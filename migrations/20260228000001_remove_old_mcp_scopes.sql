-- Remove retired MCP scope definitions
--
-- mcp:read and mcp:execute were the old flat scopes from the pre-unified-auth model.
-- They have been replaced by the team-scoped pattern (team:{name}:mcp:read, etc.)
-- enforced via check_resource_access().
--
-- Scope definitions retained:
-- - cp:read, cp:write  — used in team:{name}:cp:{action} patterns
-- - mcp:write          — still used for tool management (enable/disable routes)
-- - api:read           — used in team:{name}:api:read for gateway tool listing
-- - api:execute        — used in team:{name}:api:execute for gateway tool execution

DELETE FROM scopes WHERE value IN ('mcp:read', 'mcp:execute');
