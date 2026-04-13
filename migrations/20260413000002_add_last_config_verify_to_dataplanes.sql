-- Add last_config_verify column to dataplanes for diagnostics liveness tracking
-- Migration: 20260413000002_add_last_config_verify_to_dataplanes.sql
-- Purpose: Record the wall-clock time of the most recent diagnostics report received
-- from the flowplane-agent for a given dataplane. Used by `flowplane xds status` and
-- related views to determine whether the agent is live and producing reports. The
-- column is nullable because existing dataplanes have no report history, and every
-- successful or failed ReportDiagnostics call updates this column best-effort.

ALTER TABLE dataplanes
    ADD COLUMN IF NOT EXISTS last_config_verify TIMESTAMPTZ;
