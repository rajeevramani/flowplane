-- 0032: optional RFC 7235 auth scheme on AI providers. When set, the ExtProc capture
-- host injects '<auth_scheme> <decoded-secret>'; NULL keeps today's verbatim injection.
-- Vault feature: releases/3.0.0/features/2026-07-ai-credential-auth-scheme.

ALTER TABLE ai_providers ADD COLUMN auth_scheme TEXT;
