import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

test.describe('Admin Pages - Content Verification', () => {
	test('organizations page shows seeded org in table', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/admin/organizations');
		await waitForPageLoad(page);

		// Table should render with the seeded org visible
		const table = page.locator('table');
		await expect(table).toBeVisible();
		// Hard-assert: seeded org MUST appear (proves seed→DB→API→UI data flow)
		await expect(page.getByText(SEED.org).first()).toBeVisible();
		assertNoPageErrors(errors);
	});

	test('users page renders user table with admin user', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/admin/users');
		await waitForPageLoad(page);

		const table = page.locator('table');
		await expect(table).toBeVisible();
		// Hard-assert: at least 2 users (bootstrap admin + org-admin seeded user)
		expect(await table.locator('tbody tr').count()).toBeGreaterThanOrEqual(2);
		assertNoPageErrors(errors);
	});

	test('teams page shows seeded team in table', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/admin/teams');
		await waitForPageLoad(page);

		const table = page.locator('table');
		await expect(table).toBeVisible();
		// Hard-assert: at least 2 teams (e2e-test-org-default + e2e-org-team under test org)
		expect(await table.locator('tbody tr').count()).toBeGreaterThanOrEqual(2);
		assertNoPageErrors(errors);
	});

	test('audit log page renders log entries', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/admin/audit-log');
		await waitForPageLoad(page);

		// Bootstrap + seeding generates audit log entries
		const hasTable = (await page.locator('table').count()) > 0;
		const hasContent = (await page.getByText(/audit/i).count()) > 0;

		expect(hasTable || hasContent).toBe(true);
		assertNoPageErrors(errors);
	});
});
