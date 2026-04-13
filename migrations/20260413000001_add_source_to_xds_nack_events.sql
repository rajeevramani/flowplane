-- Add source discriminator and warming-report fields to xds_nack_events
-- Migration: 20260413000001_add_source_to_xds_nack_events.sql
-- Purpose: Allow xds_nack_events to store both stream NACKs (source='stream')
--          and listener/cluster warming failure reports from the dataplane agent
--          (source='warming_report') without forcing fake values into stream-only
--          fields. Warming reports have no nonce or version_rejected, so those
--          columns become nullable. A nullable dedup_hash with a unique partial
--          index allows the ingestion path to deduplicate repeated reports for
--          the same underlying error without affecting stream NACK inserts.
--
-- Backfill: existing rows are stream NACKs. The NOT NULL DEFAULT 'stream' on
-- source handles the backfill automatically.

ALTER TABLE xds_nack_events
    ADD COLUMN source TEXT NOT NULL DEFAULT 'stream'
        CHECK (source IN ('stream', 'warming_report'));

ALTER TABLE xds_nack_events
    ADD COLUMN dedup_hash TEXT;

ALTER TABLE xds_nack_events
    ALTER COLUMN nonce DROP NOT NULL;

ALTER TABLE xds_nack_events
    ALTER COLUMN version_rejected DROP NOT NULL;

-- Unique partial index: deduplicate warming reports (or any future source)
-- that opt in to a dedup_hash, while leaving rows with NULL dedup_hash
-- (existing stream NACKs) unconstrained.
CREATE UNIQUE INDEX IF NOT EXISTS xds_nack_events_dedup_hash_idx
    ON xds_nack_events (dedup_hash)
    WHERE dedup_hash IS NOT NULL;
