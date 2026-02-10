-- Add risk_level column to audit_log for security observability
-- Stores the risk classification (SAFE, LOW, MEDIUM, HIGH, CRITICAL) of the tool operation
ALTER TABLE audit_log ADD COLUMN risk_level VARCHAR(20);
