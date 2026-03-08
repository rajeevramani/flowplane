-- Agent context on machine users (NULL for humans, non-null for agents)
ALTER TABLE users
    ADD COLUMN agent_context TEXT DEFAULT NULL
        CHECK (agent_context IN ('cp-tool', 'gateway-tool', 'api-consumer'));

-- Partial index: only index non-null values (machine users)
CREATE INDEX idx_users_agent_context ON users(agent_context)
    WHERE agent_context IS NOT NULL;

-- Route exposure flag: controls whether a route can be granted to agents
ALTER TABLE routes
    ADD COLUMN exposure TEXT NOT NULL DEFAULT 'internal'
        CHECK (exposure IN ('internal', 'external'));

CREATE INDEX idx_routes_exposure ON routes(exposure);
