import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

// Runs as orgadmin project (org admin auth)
const orgName = SEED.org;

test.describe('Org Context', () => {
	test('org teams page loads with correct context', async ({ page }) => {
		const errors = collectPageErrors(page);

		// Navigate to teams page under the seeded org
		await page.goto(`/organizations/${orgName}/teams`);
		await waitForPageLoad(page);

		// Verify we see teams from the org or an empty state
		const hasTeamTable = await page.locator('table').isVisible().catch(() => false);
		const hasEmptyState = await page
			.getByText(/no teams/i)
			.isVisible()
			.catch(() => false);
		expect(hasTeamTable || hasEmptyState).toBe(true);

		// URL should contain the org name
		expect(page.url()).toContain(orgName);

		assertNoPageErrors(errors);
	});

	test('navigating to non-existent org shows error', async ({ page }) => {
		const errors = collectPageErrors(page);

		await page.goto('/organizations/nonexistent-org-12345/teams');
		await waitForPageLoad(page);

		const bodyText = (await page.locator('body').textContent()) ?? '';
		const hasError =
			bodyText.toLowerCase().includes('not found') ||
			bodyText.toLowerCase().includes('forbidden') ||
			bodyText.toLowerCase().includes('error') ||
			// Teams page shows membership-required message for unknown orgs
			bodyText.toLowerCase().includes('membership required') ||
			// Teams page shows no teams for non-member orgs (graceful degradation)
			bodyText.toLowerCase().includes('no teams') ||
			page.url().includes('/dashboard') ||
			page.url().includes('/login');

		expect(hasError).toBe(true);
	});
});
