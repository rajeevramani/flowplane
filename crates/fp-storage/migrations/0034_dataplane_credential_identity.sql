-- 0034: establish canonical certificate serials and exact leaf identity metadata.
--
-- This is the safe intermediate for exact credential binding: the existing global
-- SPIFFE URI uniqueness remains in place until every xDS-family resolver is exact.
-- Legacy rows remain nullable because their leaf DER was never stored.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM proxy_certificates
        WHERE serial_number = ''
           OR serial_number !~ '^[0-9A-Fa-f]+$'
    ) THEN
        RAISE EXCEPTION 'FP_CERT_SERIAL_MALFORMED: proxy certificate serials must be unsigned hexadecimal';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM (
            SELECT
                team_id,
                CASE
                    WHEN ltrim(lower(serial_number), '0') = '' THEN '0'
                    ELSE ltrim(lower(serial_number), '0')
                END AS canonical_serial
            FROM proxy_certificates
        ) canonical
        GROUP BY team_id, canonical_serial
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'FP_CERT_SERIAL_CANONICAL_COLLISION: canonical serials collide within a team';
    END IF;
END
$$;

UPDATE proxy_certificates
SET serial_number = CASE
    WHEN ltrim(lower(serial_number), '0') = '' THEN '0'
    ELSE ltrim(lower(serial_number), '0')
END;

ALTER TABLE proxy_certificates
    ADD COLUMN fingerprint_sha256 TEXT,
    ADD CONSTRAINT proxy_certificates_serial_number_canonical
        CHECK (serial_number ~ '^(0|[1-9a-f][0-9a-f]*)$'),
    ADD CONSTRAINT proxy_certificates_fingerprint_sha256_format
        CHECK (
            fingerprint_sha256 IS NULL
            OR fingerprint_sha256 ~ '^[0-9a-f]{64}$'
        );

CREATE UNIQUE INDEX uq_proxy_certificates_fingerprint_sha256
    ON proxy_certificates(fingerprint_sha256)
    WHERE fingerprint_sha256 IS NOT NULL;

CREATE FUNCTION canonicalize_proxy_certificate_serial()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.serial_number = '' OR NEW.serial_number !~ '^[0-9A-Fa-f]+$' THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'FP_CERT_SERIAL_MALFORMED: proxy certificate serials must be unsigned hexadecimal';
    END IF;
    NEW.serial_number := CASE
        WHEN ltrim(lower(NEW.serial_number), '0') = '' THEN '0'
        ELSE ltrim(lower(NEW.serial_number), '0')
    END;
    RETURN NEW;
END
$$;

CREATE TRIGGER canonicalize_proxy_certificate_serial_before_write
    BEFORE INSERT OR UPDATE OF serial_number ON proxy_certificates
    FOR EACH ROW
    EXECUTE FUNCTION canonicalize_proxy_certificate_serial();
