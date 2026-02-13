import { APIRequestContext } from '@playwright/test';

// All seeded resource names use this prefix for easy identification and cleanup
const PREFIX = 'e2e';

export const SEED = {
	org: `${PREFIX}-test-org`, // tenant org for all resource creation
	team: `${PREFIX}-test-org-default`, // auto-created default team when org is created
	cluster: `${PREFIX}-httpbin-cluster`,
	routeConfig: `${PREFIX}-api-routes`,
	listener: `${PREFIX}-http-listener`,
	filter: `${PREFIX}-header-filter`,
	dataplane: `${PREFIX}-dataplane`,
	secret: `${PREFIX}-tls-secret`
};

export const ORG_ADMIN = {
	email: 'orgadmin@e2e.flowplane.local',
	password: 'E2E_OrgAdmin!23',
	name: 'E2E Org Admin',
};

export const SEED_ORG = {
	name: 'e2e-test-org',
	team: 'e2e-org-team',
	cluster: 'e2e-org-cluster',
	routeConfig: 'e2e-org-routes',
};

interface SeedContext {
	request: APIRequestContext;
	csrfToken: string;
	dataplaneId?: string;
	orgId?: string;
}

/**
 * Seed test data via API calls. Called from auth.setup.ts after bootstrap.
 *
 * Uses the Playwright APIRequestContext which automatically stores
 * the fp_session cookie from the login response.
 *
 * Idempotent: handles 409 Conflict and 500 duplicate-key gracefully.
 *
 * Strategy:
 * - Create tenant org "e2e-test-org" (auto-creates "e2e-test-org-default" team)
 * - Create all Envoy resources under the tenant org's default team
 * - "platform" org is governance-only — no teams, no resources
 */
export async function seedTestData(
	request: APIRequestContext,
	credentials: { email: string; password: string }
): Promise<void> {
	// 1. Login via API to get session cookie + CSRF token
	const loginResp = await request.post('/api/v1/auth/login', {
		data: { email: credentials.email, password: credentials.password }
	});
	if (loginResp.status() !== 200) {
		console.warn(`[seed] API login failed: ${loginResp.status()}`);
		return;
	}
	const loginData = await loginResp.json();
	const csrfToken = loginData.csrfToken;

	const ctx: SeedContext = { request, csrfToken };

	// 2. Create tenant org (auto-creates "e2e-test-org-default" team)
	await createOrg(ctx);

	// 3. Create resources in dependency order under the tenant org's default team
	await createDataplane(ctx);
	await createCluster(ctx);
	await createFilter(ctx);
	await createRouteConfig(ctx);
	await createListener(ctx);
	await createSecret(ctx);

	// 4. Set up org-admin user and org-scoped resources
	await fetchOrgId(ctx);
	await createOrgTeam(ctx);
	await createOrgAdminUser(ctx);
	await createOrgCluster(ctx);
	await createOrgRouteConfig(ctx);

	console.log('[seed] Test data seeding complete');
}

// --- Resource creation helpers ---

async function post(
	ctx: SeedContext,
	url: string,
	data: unknown
): Promise<{ ok: boolean; status: number; body?: any }> {
	const resp = await ctx.request.post(url, {
		headers: {
			'Content-Type': 'application/json',
			'X-CSRF-Token': ctx.csrfToken
		},
		data
	});
	const status = resp.status();
	if (status === 201) {
		const body = await resp.json();
		return { ok: true, status, body };
	}
	if (status === 409) {
		// Already exists — idempotent
		console.log(`[seed] ${url} → 409 (already exists, skipping)`);
		return { ok: true, status };
	}
	// Backend wraps duplicate-key violations in "Failed to create" (returns 500 not 409)
	const text = await resp.text();
	if (status === 500 && (text.includes('duplicate key') || text.includes('Failed to create'))) {
		console.log(`[seed] ${url} → 500 (likely already exists, skipping): ${text}`);
		return { ok: true, status: 409 };
	}
	console.warn(`[seed] ${url} → ${status}: ${text}`);
	return { ok: false, status };
}

async function createOrg(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/admin/organizations', {
		name: SEED.org,
		displayName: 'E2E Test Organization',
		description: 'Auto-created by e2e test seeder'
	});
	if (result.ok && result.body) {
		ctx.orgId = result.body.id;
		console.log(`[seed] org "${SEED.org}" ready (id: ${ctx.orgId})`);
	} else if (result.ok) {
		console.log(`[seed] org "${SEED.org}" ready (id will be fetched later)`);
	}
}

async function createDataplane(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, `/api/v1/teams/${SEED.team}/dataplanes`, {
		team: SEED.team,
		name: SEED.dataplane,
		gatewayHost: `envoy-${PREFIX}`,
		description: 'E2E test dataplane'
	});
	if (result.ok && result.body) {
		ctx.dataplaneId = result.body.id;
		console.log(`[seed] dataplane "${SEED.dataplane}" ready (id: ${ctx.dataplaneId})`);
	} else if (result.status === 409) {
		// Need the ID for listener creation — fetch it
		const listResp = await ctx.request.get(`/api/v1/teams/${SEED.team}/dataplanes`);
		if (listResp.ok()) {
			const dataplanes = await listResp.json();
			const items = Array.isArray(dataplanes) ? dataplanes : dataplanes.items ?? [];
			const dp = items.find((d: any) => d.name === SEED.dataplane);
			if (dp) {
				ctx.dataplaneId = dp.id;
				console.log(`[seed] dataplane "${SEED.dataplane}" already exists (id: ${ctx.dataplaneId})`);
			}
		}
	}
}

async function createCluster(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/clusters', {
		team: SEED.team,
		name: SEED.cluster,
		serviceName: 'httpbin-service',
		endpoints: [{ host: 'httpbin.org', port: 443 }],
		connectTimeoutSeconds: 5,
		useTls: true,
		lbPolicy: 'ROUND_ROBIN',
		healthChecks: []
	});
	if (result.ok) {
		console.log(`[seed] cluster "${SEED.cluster}" ready`);
	}
}

async function createFilter(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/filters', {
		name: SEED.filter,
		filterType: 'header_mutation',
		description: 'E2E test header mutation filter',
		config: {
			type: 'header_mutation',
			config: {
				request_headers_to_add: [
					{ key: 'X-E2E-Test', value: 'true', append: false }
				],
				request_headers_to_remove: [],
				response_headers_to_add: [
					{ key: 'X-Content-Type-Options', value: 'nosniff', append: false }
				],
				response_headers_to_remove: []
			}
		},
		team: SEED.team
	});
	if (result.ok) {
		console.log(`[seed] filter "${SEED.filter}" ready`);
	}
}

async function createRouteConfig(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/route-configs', {
		name: SEED.routeConfig,
		team: SEED.team,
		virtualHosts: [
			{
				name: 'default',
				domains: ['*'],
				routes: [
					{
						name: 'get-route',
						match: { path: { type: 'prefix', value: '/get' } },
						action: {
							type: 'forward',
							cluster: SEED.cluster,
							timeoutSeconds: 30
						}
					},
					{
						name: 'anything-route',
						match: { path: { type: 'prefix', value: '/anything' } },
						action: {
							type: 'forward',
							cluster: SEED.cluster,
							timeoutSeconds: 30
						}
					}
				]
			}
		]
	});
	if (result.ok) {
		console.log(`[seed] route-config "${SEED.routeConfig}" ready`);
	}
}

async function createListener(ctx: SeedContext): Promise<void> {
	if (!ctx.dataplaneId) {
		console.warn('[seed] skipping listener — no dataplaneId available');
		return;
	}
	const result = await post(ctx, '/api/v1/listeners', {
		name: SEED.listener,
		address: '0.0.0.0',
		port: 10099,
		team: SEED.team,
		protocol: 'HTTP',
		dataplaneId: ctx.dataplaneId,
		filterChains: [
			{
				name: 'default',
				filters: [
					{
						name: 'envoy.filters.network.http_connection_manager',
						type: 'httpConnectionManager',
						routeConfigName: SEED.routeConfig,
						httpFilters: [{ filter: { type: 'router' } }]
					}
				]
			}
		]
	});
	if (result.ok) {
		console.log(`[seed] listener "${SEED.listener}" ready`);
	}
}

async function createSecret(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, `/api/v1/teams/${SEED.team}/secrets`, {
		name: SEED.secret,
		secretType: 'generic_secret',
		description: 'E2E test secret',
		configuration: {
			type: 'generic_secret',
			secret: 'ZTJlLXRlc3Qtc2VjcmV0LXZhbHVl'
		}
	});
	if (result.ok) {
		console.log(`[seed] secret "${SEED.secret}" ready`);
	} else if (result.status === 503) {
		// Secret repository requires SECRET_ENCRYPTION_KEY env var — skip gracefully
		console.log('[seed] secret repository not available (encryption key not configured), skipping');
	}
}

// --- Org-admin setup helpers ---

async function fetchOrgId(ctx: SeedContext): Promise<void> {
	if (ctx.orgId) return;
	const resp = await ctx.request.get('/api/v1/admin/organizations');
	if (!resp.ok()) {
		console.warn(`[seed] failed to fetch orgs: ${resp.status()}`);
		return;
	}
	const data = await resp.json();
	const items = Array.isArray(data) ? data : data.items ?? [];
	const org = items.find((o: { name: string }) => o.name === SEED_ORG.name);
	if (org) {
		ctx.orgId = org.id;
		console.log(`[seed] org "${SEED_ORG.name}" found (id: ${ctx.orgId})`);
	} else {
		console.warn(`[seed] org "${SEED_ORG.name}" not found in org list`);
	}
}

async function createOrgTeam(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, `/api/v1/orgs/${SEED_ORG.name}/teams`, {
		name: SEED_ORG.team,
		displayName: 'E2E Org Team',
		description: 'Team under e2e-test-org for org-admin resource testing'
	});
	if (result.ok) {
		console.log(`[seed] team "${SEED_ORG.team}" ready (under ${SEED_ORG.name} org)`);
	}
}

async function createOrgAdminUser(ctx: SeedContext): Promise<void> {
	if (!ctx.orgId) {
		console.warn('[seed] skipping org-admin user — no orgId available');
		return;
	}

	// Create user with org assignment
	const userResult = await post(ctx, '/api/v1/users', {
		email: ORG_ADMIN.email,
		password: ORG_ADMIN.password,
		name: ORG_ADMIN.name,
		isAdmin: false,
		orgId: ctx.orgId
	});

	let userId: string | undefined;
	if (userResult.ok && userResult.body) {
		userId = userResult.body.id;
		console.log(`[seed] org-admin user "${ORG_ADMIN.email}" created (id: ${userId})`);
	} else if (userResult.status === 409) {
		// User already exists — fetch ID from users list
		const listResp = await ctx.request.get('/api/v1/users');
		if (listResp.ok()) {
			const data = await listResp.json();
			const items = Array.isArray(data) ? data : data.items ?? [];
			const user = items.find((u: { email: string }) => u.email === ORG_ADMIN.email);
			if (user) {
				userId = user.id;
				console.log(`[seed] org-admin user "${ORG_ADMIN.email}" already exists (id: ${userId})`);
			}
		}
	}

	if (!userId) {
		console.warn('[seed] could not determine org-admin user ID, skipping membership');
		return;
	}

	// Add org membership with admin role
	const memberResult = await post(ctx, `/api/v1/admin/organizations/${ctx.orgId}/members`, {
		userId,
		role: 'admin'
	});
	if (memberResult.ok) {
		console.log(`[seed] org membership (admin) added for "${ORG_ADMIN.email}"`);
	}
}

async function createOrgCluster(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/clusters', {
		team: SEED_ORG.team,
		name: SEED_ORG.cluster,
		serviceName: 'org-httpbin-service',
		endpoints: [{ host: 'httpbin.org', port: 443 }],
		connectTimeoutSeconds: 5,
		useTls: true,
		lbPolicy: 'ROUND_ROBIN',
		healthChecks: []
	});
	if (result.ok) {
		console.log(`[seed] cluster "${SEED_ORG.cluster}" ready (under ${SEED_ORG.team})`);
	}
}

async function createOrgRouteConfig(ctx: SeedContext): Promise<void> {
	const result = await post(ctx, '/api/v1/route-configs', {
		name: SEED_ORG.routeConfig,
		team: SEED_ORG.team,
		virtualHosts: [
			{
				name: 'default',
				domains: ['*'],
				routes: [
					{
						name: 'org-route',
						match: { path: { type: 'prefix', value: '/' } },
						action: {
							type: 'forward',
							cluster: SEED_ORG.cluster,
							timeoutSeconds: 30
						}
					}
				]
			}
		]
	});
	if (result.ok) {
		console.log(`[seed] route-config "${SEED_ORG.routeConfig}" ready (under ${SEED_ORG.team})`);
	}
}
