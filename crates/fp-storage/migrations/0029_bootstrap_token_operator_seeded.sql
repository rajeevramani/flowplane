-- #113: distinguish operator-supplied (seeded) bootstrap tokens from legacy generate-and-log
-- tokens. A successful operator seed invalidates prior unused *legacy* tokens, while a different
-- live *operator-seeded* token across replicas must fail closed rather than be clobbered. The
-- row data alone (hash/expiry/used) cannot tell the two apart, so we persist the provenance.
ALTER TABLE bootstrap_tokens
    ADD COLUMN operator_seeded BOOLEAN NOT NULL DEFAULT false;
