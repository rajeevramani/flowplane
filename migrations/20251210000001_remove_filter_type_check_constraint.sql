-- Remove filter_type CHECK constraint to enable dynamic filter types
-- The filter_type is now validated at the application level via FilterSchemaRegistry
-- PostgreSQL: Use ALTER TABLE to drop CHECK constraint directly

ALTER TABLE filters DROP CONSTRAINT IF EXISTS filters_filter_type_check;
