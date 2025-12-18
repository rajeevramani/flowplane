-- Migration: Add settings column to filter junction tables
--
-- This enables per-scope (route config, virtual host, route) filter settings,
-- allowing users to disable filters or override configuration at specific scopes.
--
-- Settings JSON schema:
-- {
--   "behavior": "use_base" | "disable" | "override",
--   "config": { /* filter-type specific override */ },
--   "requirementName": "string"  // JWT only
-- }

-- Add settings column to route_config_filters
ALTER TABLE route_config_filters ADD COLUMN settings TEXT;

-- Add settings column to virtual_host_filters
ALTER TABLE virtual_host_filters ADD COLUMN settings TEXT;

-- Add settings column to route_filters
ALTER TABLE route_filters ADD COLUMN settings TEXT;
