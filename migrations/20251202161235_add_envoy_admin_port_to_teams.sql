-- Add envoy_admin_port column for per-team Envoy admin interface ports
-- Migration: 20251202161235_add_envoy_admin_port_to_teams.sql
--
-- Each team gets an auto-allocated port for its Envoy admin interface.
-- Ports are allocated sequentially starting from the base port (default 9901).
-- Existing teams will have NULL and fall back to the global config port.

ALTER TABLE teams ADD COLUMN envoy_admin_port INTEGER;

-- Create unique index to prevent port conflicts
CREATE UNIQUE INDEX IF NOT EXISTS idx_teams_envoy_admin_port ON teams(envoy_admin_port);
