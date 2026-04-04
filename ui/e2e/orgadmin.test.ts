import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

// Page type determines what UI elements we assert beyond "no JS errors"
type PageType = 'list' | 'create' | 'dashboard' | 'form' | 'cards';

interface PageDef {
	path: string;
	name: string;
	type: PageType;
}

// Pages accessible to org-admin (excludes admin-only pages)
const orgAdminPages: PageDef[] = [
	// Dashboard
	{ path: '/dashboard', name: 'Dashboard', type: 'dashboard' },

	// Resource list pages (render <table> or "No X yet/found" empty state)
	{ path: '/clusters', name: 'Clusters', type: 'list' },
	{ path: '/listeners', name: 'Listeners', type: 'list' },
	{ path: '/route-configs', name: 'Route Configs', type: 'list' },
	{ path: '/filters', name: 'Filters', type: 'list' },
	{ path: '/secrets', name: 'Secrets', type: 'list' },
	{ path: '/dataplanes', name: 'Dataplanes', type: 'list' },
	{ path: '/imports', name: 'Imports', type: 'list' },
	{ path: '/custom-filters', name: 'Custom Filters', type: 'cards' },
	{ path: '/learning', name: 'Learning Sessions', type: 'list' },
	{ path: '/learning/schemas', name: 'Discovered Schemas', type: 'list' },
	// Card-based pages (render grid of cards/stat cards, may include table below)
	{ path: '/mcp-tools', name: 'MCP Tools', type: 'cards' },
	{ path: '/mcp-connections', name: 'MCP Connections', type: 'cards' },

	// Resource creation pages
	{ path: '/clusters/create', name: 'Create Cluster', type: 'create' },
	{ path: '/listeners/create', name: 'Create Listener', type: 'create' },
	{ path: '/route-configs/create', name: 'Create Route Config', type: 'create' },
	{ path: '/filters/create', name: 'Create Filter', type: 'create' },
	{ path: '/secrets/create', name: 'Create Secret', type: 'create' },
	{ path: '/dataplanes/create', name: 'Create Dataplane', type: 'create' },
	{ path: '/imports/import', name: 'Import OpenAPI', type: 'create' },
	{ path: '/learning/create', name: 'Create Learning Session', type: 'create' },
	{ path: '/custom-filters/upload', name: 'Upload Custom Filter', type: 'create' },

	// Form pages
	{ path: '/profile/password', name: 'Change Password', type: 'form' }
	// Note: /generate-envoy-config has a 301 redirect to /dataplanes (already tested above)
];

test.describe('Org Admin - Smoke Tests', () => {
	for (const p of orgAdminPages) {
		test(`${p.name} (${p.path}) loads with expected content`, async ({ page }) => {
			const errors = collectPageErrors(page);

			await page.goto(p.path);
			await waitForPageLoad(page);

			await expect(page.locator('body')).toBeVisible();

			const bodyText = (await page.locator('body').textContent()) || '';
			expect(bodyText).not.toContain('Cannot read properties');
			expect(bodyText).not.toContain('undefined is not an object');

			// Assert expected UI elements based on page type
			switch (p.type) {
				case 'list': {
					// List pages must show a table, an empty state message, or an error state
					const hasTable = (await page.locator('table').count()) > 0;
					const hasEmptyState =
						(await page.getByText(/no .*(found|yet|available)/i).count()) > 0;
					const hasErrorState =
						(await page.locator('.bg-red-50, [role="alert"]').count()) > 0;
					expect(
						hasTable || hasEmptyState || hasErrorState,
						`${p.name}: expected table, empty state, or error state, got neither`
					).toBe(true);
					break;
				}
				case 'create': {
					const formElements = await page
						.locator('form, input, select, textarea')
						.count();
					expect(
						formElements,
						`${p.name}: expected form elements (form/input/select/textarea)`
					).toBeGreaterThan(0);
					const hasButton = (await page.locator('button').count()) > 0;
					expect(hasButton, `${p.name}: expected at least one button`).toBe(true);
					break;
				}
				case 'form': {
					const formElements = await page
						.locator('form, input, select, textarea')
						.count();
					expect(
						formElements,
						`${p.name}: expected form elements`
					).toBeGreaterThan(0);
					break;
				}
				case 'dashboard': {
					// Org-admin dashboard shows welcome message and resource overview
					const hasWelcome = (await page.getByText(/welcome back/i).count()) > 0;
					const hasOverview = (await page.getByText(/resource overview/i).count()) > 0;
					const hasCards = (await page.locator('[class*="grid"]').count()) > 0;
					expect(
						hasWelcome || hasOverview || hasCards,
						`Dashboard: expected welcome message, resource overview, or card grid`
					).toBe(true);
					// Must NOT show platform governance
					await expect(page.getByText('Platform Governance')).not.toBeVisible();
					break;
				}
				case 'cards': {
					// Card-based pages show grid/cards/stat-cards OR an empty/info message
					const hasCards =
						(await page.locator('[class*="grid"]').count()) > 0 ||
						(await page.locator('[class*="card"], [class*="Card"]').count()) > 0;
					const hasContent =
						(await page
							.getByText(
								/no .*(found|yet|available|data|active|connections)|select a team/i
							)
							.count()) > 0;
					const hasHeading = (await page.locator('h1, h2').count()) > 0;
					expect(
						hasCards || hasContent || hasHeading,
						`${p.name}: expected card grid, informational content, or page heading`
					).toBe(true);
					break;
				}
			}

			assertNoPageErrors(errors);
		});
	}
});

test.describe('Org Admin - Sidebar', () => {
	test('shows Org Settings instead of full Admin section', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/dashboard');
		await waitForPageLoad(page);

		const sidebar = page.locator('aside');

		// Should NOT show system-admin links
		await expect(sidebar.locator('a[href="/admin/audit-log"]')).not.toBeVisible();
		await expect(sidebar.locator('a[href="/admin/organizations"]')).not.toBeVisible();

		// Should show Org Settings
		await expect(sidebar.getByText('Org Settings')).toBeVisible();

		assertNoPageErrors(errors);
	});

	test('sidebar renders without errors', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/dashboard');
		await waitForPageLoad(page);

		// Sidebar should be visible with navigation links
		const sidebar = page.locator('aside');
		await expect(sidebar).toBeVisible();

		assertNoPageErrors(errors);
	});
});

test.describe('Org Admin - Resource Pages', () => {
	test('clusters page renders list or empty state', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/clusters');
		await waitForPageLoad(page);

		// Page should render a table or empty state (seed-demo.sh does not create clusters)
		const hasTable = (await page.locator('table').count()) > 0;
		const hasEmptyState =
			(await page.getByText(/no .*(found|yet|available)/i).count()) > 0;
		expect(
			hasTable || hasEmptyState,
			'Clusters page should render table or empty state'
		).toBe(true);

		assertNoPageErrors(errors);
	});

	test('route-configs page renders list or empty state', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/route-configs');
		await waitForPageLoad(page);

		// Page should render a table or empty state (seed-demo.sh does not create route configs)
		const hasTable = (await page.locator('table').count()) > 0;
		const hasEmptyState =
			(await page.getByText(/no .*(found|yet|available)/i).count()) > 0;
		expect(
			hasTable || hasEmptyState,
			'Route configs page should render table or empty state'
		).toBe(true);

		assertNoPageErrors(errors);
	});
});

test.describe('Org Admin - Admin Page Restrictions', () => {
	const restrictedPages = [
		{ path: '/admin/teams', name: 'Teams' },
		{ path: '/admin/audit-log', name: 'Audit Log' }
	];

	for (const rp of restrictedPages) {
		test(`${rp.name} page (${rp.path}) is not accessible`, async ({ page }) => {
			const errors = collectPageErrors(page);
			await page.goto(rp.path);
			await waitForPageLoad(page);

			const url = page.url();
			const bodyText = (await page.locator('body').textContent()) || '';
			const isRestricted =
				url.includes('/dashboard') ||
				url.includes('/login') ||
				bodyText.toLowerCase().includes('forbidden') ||
				bodyText.toLowerCase().includes('denied') ||
				bodyText.toLowerCase().includes('unauthorized') ||
				bodyText.toLowerCase().includes('not authorized');

			expect(isRestricted).toBe(true);
		});
	}

	// Scenario 7: Team member (org admin) cannot see platform admin pages
	test('org admin cannot see admin sidebar links or access admin pages', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/dashboard');
		await waitForPageLoad(page);

		const sidebar = page.locator('aside');

		// Org admin should NOT see platform admin links
		await expect(sidebar.locator('a[href="/admin/organizations"]')).not.toBeVisible();

		// Try navigating directly to an admin page
		await page.goto('/admin/organizations');
		await waitForPageLoad(page);

		const url = page.url();
		const bodyText = (await page.locator('body').textContent()) ?? '';
		const isRestricted =
			url.includes('/dashboard') ||
			url.includes('/login') ||
			bodyText.toLowerCase().includes('forbidden') ||
			bodyText.toLowerCase().includes('denied') ||
			bodyText.toLowerCase().includes('not authorized');

		expect(isRestricted).toBe(true);

		assertNoPageErrors(errors);
	});
});
