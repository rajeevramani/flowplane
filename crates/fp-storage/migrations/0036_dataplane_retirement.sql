-- 0036: tombstone dataplanes and release names only after retirement.

ALTER TABLE dataplanes
    ADD COLUMN retired_at TIMESTAMPTZ,
    ADD COLUMN retired_reason TEXT;

ALTER TABLE dataplanes
    DROP CONSTRAINT dataplanes_team_id_name_key;

CREATE UNIQUE INDEX uq_dataplanes_active_team_name
    ON dataplanes(team_id, name)
    WHERE retired_at IS NULL;
