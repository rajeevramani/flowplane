-- Add local_rate_limit to the filter_type CHECK constraint
-- PostgreSQL: Use ALTER TABLE to modify CHECK constraint directly

ALTER TABLE filters DROP CONSTRAINT IF EXISTS filters_filter_type_check;
ALTER TABLE filters ADD CONSTRAINT filters_filter_type_check
    CHECK (filter_type IN ('header_mutation', 'jwt_auth', 'cors', 'local_rate_limit', 'rate_limit', 'ext_authz'));
