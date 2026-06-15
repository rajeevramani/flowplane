-- 0025: S10 AI-owned gateway resources. User APIs still hide managed rows.

ALTER TABLE clusters DROP CONSTRAINT clusters_owner_id_required;
ALTER TABLE route_configs DROP CONSTRAINT route_configs_owner_id_required;
ALTER TABLE listeners DROP CONSTRAINT listeners_owner_id_required;

ALTER TABLE clusters DROP CONSTRAINT clusters_owner_kind_check;
ALTER TABLE route_configs DROP CONSTRAINT route_configs_owner_kind_check;
ALTER TABLE listeners DROP CONSTRAINT listeners_owner_kind_check;

ALTER TABLE clusters
    ADD CONSTRAINT clusters_owner_kind_check CHECK (owner_kind IN ('user', 'discovery', 'ai')),
    ADD CONSTRAINT clusters_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind IN ('discovery', 'ai') AND owner_id IS NOT NULL)
    );

ALTER TABLE route_configs
    ADD CONSTRAINT route_configs_owner_kind_check CHECK (owner_kind IN ('user', 'discovery', 'ai')),
    ADD CONSTRAINT route_configs_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind IN ('discovery', 'ai') AND owner_id IS NOT NULL)
    );

ALTER TABLE listeners
    ADD CONSTRAINT listeners_owner_kind_check CHECK (owner_kind IN ('user', 'discovery', 'ai')),
    ADD CONSTRAINT listeners_owner_id_required CHECK (
        (owner_kind = 'user' AND owner_id IS NULL)
        OR (owner_kind IN ('discovery', 'ai') AND owner_id IS NOT NULL)
    );
