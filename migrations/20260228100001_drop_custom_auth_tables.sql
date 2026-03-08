-- Drop custom auth tables replaced by Zitadel
--
-- personal_access_tokens + token_scopes: PAT-based auth replaced by Zitadel JWT
-- invitations: org invitation system replaced by Zitadel user management

DROP TABLE IF EXISTS token_scopes;
DROP TABLE IF EXISTS personal_access_tokens;
DROP TABLE IF EXISTS invitations;
