-- Create listeners table for storing Envoy listener configurations
-- Migration: 20241201000003_create_listeners_table.sql

CREATE TABLE IF NOT EXISTS listeners (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    address TEXT NOT NULL,  -- IP address or socket path
    port INTEGER,           -- Port number (NULL for Unix domain sockets)
    protocol TEXT NOT NULL DEFAULT 'HTTP',  -- HTTP, HTTPS, TCP, etc.
    configuration TEXT NOT NULL,  -- JSON serialized listener config
    version INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Ensure unique listener names per version
    UNIQUE(name, version)
);

-- Index for version queries
CREATE INDEX IF NOT EXISTS idx_listeners_version ON listeners(version);

-- Index for address lookups
CREATE INDEX IF NOT EXISTS idx_listeners_address ON listeners(address);

-- Index for protocol filtering
CREATE INDEX IF NOT EXISTS idx_listeners_protocol ON listeners(protocol);

-- Index for efficient timestamp queries
CREATE INDEX IF NOT EXISTS idx_listeners_updated_at ON listeners(updated_at);

-- Partial unique index for address:port combinations (when port is not NULL)
CREATE UNIQUE INDEX IF NOT EXISTS idx_listeners_address_port
ON listeners(address, port) WHERE port IS NOT NULL;