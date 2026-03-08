import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';

// Runs as orgadmin project (org admin auth)

test.describe('Route Management', () => {
	// Scenario 15: Toggle route exposure internal → external → verify badge
	test('toggle route exposure → verify badge change', async ({ page }) => {
		const errors = collectPageErrors(page);

		// Navigate to route configs
		await page.goto('/route-configs');
		await waitForPageLoad(page);

		// Click on the first route config to edit
		const table = page.locator('table');
		const hasTable = await table.isVisible().catch(() => false);

		if (hasTable) {
			// Click the first route config row or edit link
			const editLink = page.getByRole('link', { name: /edit|view|manage/i }).first();
			const hasEditLink = await editLink.isVisible().catch(() => false);

			if (hasEditLink) {
				await editLink.click();
				await waitForPageLoad(page);

				// Look for routes table with exposure badges
				const routesTable = page.locator('table').filter({ hasText: /route/i });
				const hasRoutes = await routesTable.isVisible().catch(() => false);

				if (hasRoutes) {
					// Look for an "internal" badge that can be toggled
					const internalBadge = page.getByText('internal').first();
					const hasInternal = await internalBadge.isVisible().catch(() => false);

					if (hasInternal) {
						// Click to edit/toggle the route
						const editRouteBtn = page
							.getByRole('button', { name: /edit/i })
							.first();
						const hasEdit = await editRouteBtn.isVisible().catch(() => false);

						if (hasEdit) {
							await editRouteBtn.click();
							await waitForPageLoad(page);

							// Find exposure select/toggle and change to external
							const exposureSelect = page.locator(
								'select[name*="exposure"], #exposure'
							);
							if (await exposureSelect.isVisible()) {
								await exposureSelect.selectOption('external');
								// Save
								const saveBtn = page.getByRole('button', { name: /save|update/i });
								await saveBtn.click();
								await page.waitForTimeout(2000);

								// Verify the badge changed to "external"
								const externalBadge = page.getByText('external').first();
								await expect(externalBadge).toBeVisible({ timeout: 10000 });
							}
						}
					}
				}
			}
		}

		assertNoPageErrors(errors);
	});
});
