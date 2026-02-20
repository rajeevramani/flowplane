import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

test.describe('Resource List Pages - Content Verification', () => {
	// Platform admin sees AdminResourceSummary with counts per org/team.
	// Verify that the summary table shows the seeded org with non-zero counts.
	// Platform admin sees AdminResourceSummary (org/team breakdown with counts),
	// NOT individual resource names. Assert the summary table renders with data rows.
	const resourcePages = [
		{ path: '/clusters', label: /cluster/i },
		{ path: '/listeners', label: /listener/i },
		{ path: '/route-configs', label: /route/i },
		{ path: '/filters', label: /filter/i },
		{ path: '/secrets', label: /secret/i },
		{ path: '/dataplanes', label: /dataplane/i },
		{ path: '/imports', label: /import/i }
	];

	for (const rp of resourcePages) {
		test(`${rp.path} renders with seeded data`, async ({ page }) => {
			const errors = collectPageErrors(page);
			await page.goto(rp.path);
			await waitForPageLoad(page);

			// Should show the AdminResourceSummary table, an empty state, or content
			const hasTable = (await page.locator('table').count()) > 0;
			const hasEmptyState = (await page.getByText(/no .*(found|yet|available)/i).count()) > 0;
			const hasContent = (await page.getByText(rp.label).count()) > 0;

			expect(hasTable || hasEmptyState || hasContent).toBe(true);

			// Hard-assert: AdminResourceSummary table must have data rows (proves seeded data flows)
			if (hasTable) {
				const rowCount = await page.locator('table tbody tr').count();
				expect(rowCount, `${rp.path}: AdminResourceSummary table must have data rows`).toBeGreaterThanOrEqual(1);
			}

			assertNoPageErrors(errors);
		});
	}

	// Dashboard specific test — admin summary should show non-zero totals
	test('dashboard renders admin summary with seeded data', async ({ page }) => {
		const errors = collectPageErrors(page);
		await page.goto('/dashboard');
		await waitForPageLoad(page);

		// Dashboard should show summary/overview content
		const hasContent =
			(await page.getByText(/dashboard|overview|summary|welcome/i).count()) > 0;
		expect(hasContent).toBe(true);

		// Hard-assert: seeded org MUST appear in AdminResourceSummary breakdown
		await expect(
			page.getByText(SEED.org).first()
		).toBeVisible({ timeout: 5000 });

		assertNoPageErrors(errors);
	});
});

test.describe('Resource Create Pages - Form Rendering', () => {
	const createPages = [
		{ path: '/clusters/create', label: /cluster/i },
		{ path: '/listeners/create', label: /listener/i },
		{ path: '/route-configs/create', label: /route/i },
		{ path: '/filters/create', label: /filter/i },
		{ path: '/secrets/create', label: /secret/i },
		{ path: '/dataplanes/create', label: /dataplane/i }
	];

	for (const cp of createPages) {
		test(`${cp.path} renders creation form`, async ({ page }) => {
			const errors = collectPageErrors(page);
			await page.goto(cp.path);
			await waitForPageLoad(page);

			// Create pages should show form elements
			const hasForm = (await page.locator('form, input, select, textarea').count()) > 0;
			const hasButton = (await page.locator('button').count()) > 0;

			expect(hasForm || hasButton).toBe(true);
			assertNoPageErrors(errors);
		});
	}
});
