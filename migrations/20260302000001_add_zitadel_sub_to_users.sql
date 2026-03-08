-- Add zitadel_sub column to users table for JIT user provisioning.
-- This column stores the Zitadel subject identifier (the `sub` claim from JWT)
-- and is the stable bridge between Zitadel identity and Flowplane permissions.

ALTER TABLE users ADD COLUMN zitadel_sub TEXT UNIQUE;
CREATE INDEX idx_users_zitadel_sub ON users(zitadel_sub);
