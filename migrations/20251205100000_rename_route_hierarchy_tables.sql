-- Migration: Rename route hierarchy tables for Envoy terminology alignment
--
-- This migration aligns our database schema with Envoy's terminology:
-- - RouteConfiguration: Top-level route config (was "routes" table)
-- - Route: Individual route within a VirtualHost (was "route_rules" table)
--
-- Renames:
-- 1. routes -> route_configs
-- 2. route_filters -> route_config_filters (with route_id -> route_config_id)
-- 3. route_rules -> routes
-- 4. route_rule_filters -> route_filters (with route_rule_id -> route_id)
-- 5. listener_routes -> listener_route_configs (with route_id -> route_config_id)

-- Step 1: Rename the routes table to route_configs
ALTER TABLE routes RENAME TO route_configs;

-- Step 2: Rename indexes for route_configs
DROP INDEX IF EXISTS idx_routes_version;
DROP INDEX IF EXISTS idx_routes_cluster_name;
DROP INDEX IF EXISTS idx_routes_path_prefix;
DROP INDEX IF EXISTS idx_routes_updated_at;
DROP INDEX IF EXISTS idx_routes_team;
DROP INDEX IF EXISTS idx_routes_import_id;

CREATE INDEX idx_route_configs_version ON route_configs(version);
CREATE INDEX idx_route_configs_cluster_name ON route_configs(cluster_name);
CREATE INDEX idx_route_configs_path_prefix ON route_configs(path_prefix);
CREATE INDEX idx_route_configs_updated_at ON route_configs(updated_at);
CREATE INDEX IF NOT EXISTS idx_route_configs_team ON route_configs(team);
CREATE INDEX IF NOT EXISTS idx_route_configs_import_id ON route_configs(import_id);

-- Step 3: Rename route_filters to route_config_filters and update column
-- SQLite requires recreating the table to rename columns with FK constraints
CREATE TABLE route_config_filters (
    route_config_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_config_id) REFERENCES route_configs(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (route_config_id, filter_id),
    UNIQUE(route_config_id, filter_order)
);

INSERT INTO route_config_filters (route_config_id, filter_id, filter_order, created_at)
SELECT route_id, filter_id, filter_order, created_at FROM route_filters;

DROP TABLE route_filters;

CREATE INDEX idx_route_config_filters_route ON route_config_filters(route_config_id);
CREATE INDEX idx_route_config_filters_filter ON route_config_filters(filter_id);

-- Step 4: Rename route_rules to routes
ALTER TABLE route_rules RENAME TO routes;

-- Step 5: Rename indexes for routes (was route_rules)
DROP INDEX IF EXISTS idx_route_rules_virtual_host;

CREATE INDEX idx_routes_virtual_host ON routes(virtual_host_id);

-- Step 6: Rename route_rule_filters to route_filters and update column
CREATE TABLE route_filters (
    route_id TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    filter_order BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
    FOREIGN KEY (filter_id) REFERENCES filters(id) ON DELETE RESTRICT,

    PRIMARY KEY (route_id, filter_id),
    UNIQUE(route_id, filter_order)
);

INSERT INTO route_filters (route_id, filter_id, filter_order, created_at)
SELECT route_rule_id, filter_id, filter_order, created_at FROM route_rule_filters;

DROP TABLE route_rule_filters;

CREATE INDEX idx_route_filters_route ON route_filters(route_id);
CREATE INDEX idx_route_filters_filter ON route_filters(filter_id);

-- Step 7: Rename listener_routes to listener_route_configs and update column
CREATE TABLE listener_route_configs (
    listener_id TEXT NOT NULL,
    route_config_id TEXT NOT NULL,
    route_order BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    FOREIGN KEY (listener_id) REFERENCES listeners(id) ON DELETE CASCADE,
    FOREIGN KEY (route_config_id) REFERENCES route_configs(id) ON DELETE CASCADE,

    PRIMARY KEY (listener_id, route_config_id),
    UNIQUE(listener_id, route_order)
);

INSERT INTO listener_route_configs (listener_id, route_config_id, route_order, created_at)
SELECT listener_id, route_id, route_order, created_at FROM listener_routes;

DROP TABLE listener_routes;

CREATE INDEX idx_listener_route_configs_listener ON listener_route_configs(listener_id);
CREATE INDEX idx_listener_route_configs_route ON listener_route_configs(route_config_id);
