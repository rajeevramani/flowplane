-- Add certificate lifecycle tracking columns to dataplanes table
-- Migration: 20260204000001_add_dataplane_certificate_tracking.sql
-- Purpose: Track certificate serial number and expiry for mTLS renewal and revocation

-- Add certificate tracking columns
ALTER TABLE dataplanes ADD COLUMN certificate_serial TEXT;
ALTER TABLE dataplanes ADD COLUMN certificate_expires_at TEXT;

-- Index for finding dataplanes with expiring certificates
CREATE INDEX IF NOT EXISTS idx_dataplanes_certificate_expires_at
    ON dataplanes(certificate_expires_at)
    WHERE certificate_expires_at IS NOT NULL;
