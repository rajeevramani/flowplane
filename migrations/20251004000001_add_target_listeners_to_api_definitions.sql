-- Add target_listeners column to api_definitions table for listener isolation false mode
-- Migration: 20251004000001_add_target_listeners_to_api_definitions.sql

-- Add target_listeners column to store JSON array of listener names
-- This field is only used when listenerIsolation is false
-- NULL means use default behavior (default-gateway-listener)
-- Must be NULL when listenerIsolation is true (validated at application layer)
ALTER TABLE api_definitions ADD COLUMN target_listeners TEXT;

-- Add index for querying definitions by target listeners
-- Only index non-NULL values to improve query performance
CREATE INDEX IF NOT EXISTS idx_api_definitions_target_listeners
    ON api_definitions(target_listeners)
    WHERE target_listeners IS NOT NULL;
