-- Add headers column to api_routes table for HTTP method matching
-- Migration: 20251006000001_add_headers_to_api_routes.sql

ALTER TABLE api_routes ADD COLUMN headers TEXT;
