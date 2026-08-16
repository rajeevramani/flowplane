-- 0035: permit bounded overlap between exact dataplane credentials.
--
-- Runtime issue/register mutations enforce at most two unrevoked rows while holding the
-- dataplane row lock. This preflight prevents an existing over-cap population from crossing
-- into the new schema before that service invariant can protect subsequent writes.

DO $$
BEGIN
    IF EXISTS (
        SELECT dataplane_id
        FROM proxy_certificates
        WHERE revoked_at IS NULL
        GROUP BY dataplane_id
        HAVING count(*) > 2
    ) THEN
        RAISE EXCEPTION 'FP_CERT_UNREVOKED_CAP_EXCEEDED: a dataplane has more than two unrevoked credentials';
    END IF;
END
$$;

ALTER TABLE proxy_certificates
    DROP CONSTRAINT proxy_certificates_spiffe_uri_key;

CREATE INDEX idx_proxy_certificates_spiffe_uri
    ON proxy_certificates(spiffe_uri);
