/**
 * E2E seed data constants matching the output of `make seed` (scripts/seed-demo.sh).
 *
 * Auth: Uses Zitadel OIDC. Org admin is `demo@acme-corp.com` (seeded by seed-demo.sh).
 * Platform admin is `admin@flowplane.local` (seeded by CP startup).
 */

// ── Constants (must match scripts/seed-demo.sh output) ──────────

export const SEED = {
	org: 'acme-corp',
	orgDisplayName: 'Acme Corp',
	team: 'engineering',
	agent: 'flowplane-agent',
};

export const ORG_ADMIN = {
	email: 'demo@acme-corp.com',
	name: 'Demo User',
};
