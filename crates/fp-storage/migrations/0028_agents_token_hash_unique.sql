-- S11/#79: agent bearer tokens are stored only as hashes and resolved by hash at auth time.
-- Enforce one credential hash per agent row so lookup cannot become ambiguous.
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_token_hash_unique ON agents(token_hash);
