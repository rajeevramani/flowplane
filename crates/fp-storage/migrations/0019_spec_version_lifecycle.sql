-- 0019: S8.7 learned SpecVersion review/publish lifecycle.
-- Spec content remains immutable; lifecycle state lives next to it.

ALTER TABLE api_definitions
ADD COLUMN published_spec_version_id UUID;

ALTER TABLE spec_versions
ADD CONSTRAINT spec_versions_id_api_team_unique UNIQUE (id, api_definition_id, team_id);

ALTER TABLE api_definitions
ADD CONSTRAINT api_definitions_published_spec_version_fk
FOREIGN KEY (published_spec_version_id, id, team_id)
REFERENCES spec_versions(id, api_definition_id, team_id)
ON DELETE SET NULL (published_spec_version_id);

CREATE TABLE spec_version_review_events (
    id                UUID PRIMARY KEY,
    team_id           UUID NOT NULL,
    org_id            UUID NOT NULL,
    api_definition_id UUID NOT NULL,
    spec_version_id   UUID NOT NULL,
    decision          TEXT NOT NULL CHECK (decision IN ('submitted', 'reviewed', 'rejected', 'published', 'unpublished')),
    actor_type        TEXT NOT NULL,
    actor_id          UUID,
    reason            TEXT NOT NULL DEFAULT '',
    metadata          JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, team_id),
    FOREIGN KEY (team_id, org_id) REFERENCES teams(id, org_id) ON DELETE CASCADE,
    FOREIGN KEY (api_definition_id, team_id) REFERENCES api_definitions(id, team_id) ON DELETE CASCADE,
    FOREIGN KEY (spec_version_id, api_definition_id, team_id)
        REFERENCES spec_versions(id, api_definition_id, team_id) ON DELETE CASCADE
);
CREATE INDEX idx_spec_version_review_events_spec ON spec_version_review_events(spec_version_id, created_at);
CREATE INDEX idx_spec_version_review_events_api ON spec_version_review_events(api_definition_id, created_at);
