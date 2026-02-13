import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';

// Page type determines what UI elements we assert beyond "no JS errors"
type PageType = 'list' | 'create' | 'dashboard' | 'form' | 'cards' | 'content';

interface PageDef {
	path: string;
	name: string;
	type: PageType;
}

// All authenticated pages categorized by expected UI structure
const allPages: PageDef[] = [
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
	{ path: '/tokens', name: 'Access Tokens', type: 'list' },
	{ path: '/custom-filters', name: 'Custom Filters', type: 'list' },
	{ path: '/learning', name: 'Learning Sessions', type: 'list' },
	{ path: '/learning/schemas', name: 'Discovered Schemas', type: 'list' },

	// Card-based pages (render grid of cards/stat cards, may include table below)
	{ path: '/mcp-tools', name: 'MCP Tools', type: 'cards' },
	{ path: '/mcp-connections', name: 'MCP Connections', type: 'cards' },
	{ path: '/stats', name: 'Stats Dashboard', type: 'cards' },

	// Resource creation pages (render <form>, <input>, <select>, or <textarea>)
	{ path: '/clusters/create', name: 'Create Cluster', type: 'create' },
	{ path: '/listeners/create', name: 'Create Listener', type: 'create' },
	{ path: '/route-configs/create', name: 'Create Route Config', type: 'create' },
	{ path: '/filters/create', name: 'Create Filter', type: 'create' },
	{ path: '/secrets/create', name: 'Create Secret', type: 'create' },
	{ path: '/dataplanes/create', name: 'Create Dataplane', type: 'create' },
	{ path: '/imports/import', name: 'Import OpenAPI', type: 'create' },
	{ path: '/learning/create', name: 'Create Learning Session', type: 'create' },
	{ path: '/custom-filters/upload', name: 'Upload Custom Filter', type: 'create' },

	// Admin list pages
	{ path: '/admin/organizations', name: 'Admin: Organizations', type: 'list' },
	{ path: '/admin/users', name: 'Admin: Users', type: 'list' },
	{ path: '/admin/teams', name: 'Admin: Teams', type: 'list' },
	{ path: '/admin/audit-log', name: 'Admin: Audit Log', type: 'list' },

	// Admin creation pages
	{ path: '/admin/organizations/create', name: 'Admin: Create Organization', type: 'create' },
	{ path: '/admin/users/create', name: 'Admin: Create User', type: 'create' },
	{ path: '/admin/teams/create', name: 'Admin: Create Team', type: 'create' },

	// Form pages (non-resource-creation forms)
	{ path: '/profile/password', name: 'Change Password', type: 'form' },
	{ path: '/generate-envoy-config', name: 'Generate Envoy Config', type: 'form' }
];

test.describe('Smoke Tests - All Pages', () => {
	for (const p of allPages) {
		test(`${p.name} (${p.path}) loads with expected content`, async ({ page }) => {
			const errors = collectPageErrors(page);

			await page.goto(p.path);
			await waitForPageLoad(page);

			// Page should have visible content
			await expect(page.locator('body')).toBeVisible();

			// Page should not show raw error text
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
					// Create pages must show form elements
					const formElements = await page
						.locator('form, input, select, textarea')
						.count();
					expect(
						formElements,
						`${p.name}: expected form elements (form/input/select/textarea)`
					).toBeGreaterThan(0);
					// Must have a submit/create button
					const hasButton = (await page.locator('button').count()) > 0;
					expect(hasButton, `${p.name}: expected at least one button`).toBe(true);
					break;
				}
				case 'form': {
					// Generic form pages must have inputs and a submit button
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
					// Platform admin dashboard shows AdminResourceSummary or governance content
					const hasSummary =
						(await page.getByText(/platform governance/i).count()) > 0 ||
						(await page.locator('table').count()) > 0 ||
						(await page.getByText(/welcome back/i).count()) > 0;
					expect(
						hasSummary,
						`Dashboard: expected governance heading, summary table, or welcome message`
					).toBe(true);
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

			// Critical: No JS errors or schema validation failures
			assertNoPageErrors(errors);
		});
	}
});
