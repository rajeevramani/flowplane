import { test, expect } from '@playwright/test';

test.describe('Navigation', () => {
	const pages = [
		{ path: '/dashboard', name: 'Dashboard' },
		{ path: '/listeners', name: 'Listeners' },
		{ path: '/route-configs', name: 'Route Configs' },
		{ path: '/clusters', name: 'Clusters' },
		{ path: '/filters', name: 'Filters' },
		{ path: '/secrets', name: 'Secrets' },
		{ path: '/dataplanes', name: 'Dataplanes' },
		{ path: '/imports', name: 'Imports' },
		{ path: '/tokens', name: 'Access Tokens' },
		{ path: '/custom-filters', name: 'Custom Filters' },
		{ path: '/learning', name: 'Learning Sessions' },
		{ path: '/learning/schemas', name: 'Discovered Schemas' },
		{ path: '/mcp-tools', name: 'MCP Tools' },
		{ path: '/mcp-connections', name: 'MCP Connections' }
	];

	for (const p of pages) {
		test(`${p.name} page loads without crash`, async ({ page }) => {
			await page.goto(p.path);
			await expect(page.locator('body')).toBeVisible();
			const bodyText = await page.locator('body').textContent();
			expect(bodyText).not.toContain('Cannot read properties');
			expect(bodyText).not.toContain('Internal error');
		});
	}

	// Admin pages (require admin role)
	const adminPages = [
		{ path: '/admin/users', name: 'Users' },
		{ path: '/admin/teams', name: 'Teams' },
		{ path: '/admin/audit-log', name: 'Audit Log' },
		{ path: '/admin/organizations', name: 'Organizations' }
	];

	for (const p of adminPages) {
		test(`Admin: ${p.name} page loads without crash`, async ({ page }) => {
			await page.goto(p.path);
			await expect(page.locator('body')).toBeVisible();
			const bodyText = await page.locator('body').textContent();
			expect(bodyText).not.toContain('Cannot read properties');
			expect(bodyText).not.toContain('Internal error');
		});
	}
});
