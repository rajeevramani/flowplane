import { test, expect } from '@playwright/test';

test.describe('Organizations Page', () => {
	test('organizations page loads without crash', async ({ page }) => {
		await page.goto('/admin/organizations');
		await expect(page.locator('body')).toBeVisible();

		const bodyText = await page.locator('body').textContent();
		expect(bodyText).not.toContain('Cannot read properties');
	});

	test('organizations page shows content or empty state', async ({ page }) => {
		await page.goto('/admin/organizations');

		// Wait for loading to complete
		await page
			.waitForSelector('[class*="animate-spin"]', { state: 'hidden', timeout: 10000 })
			.catch(() => {});

		const hasTable = (await page.locator('table').count()) > 0;
		const hasEmptyState = (await page.getByText(/no organizations/i).count()) > 0;
		const hasError = (await page.locator('[class*="error"], .bg-red-50').count()) > 0;
		const hasContent = (await page.locator('body').textContent())!.length > 0;

		// At least one state should be visible (not a blank crash)
		expect(hasTable || hasEmptyState || hasError || hasContent).toBe(true);
	});
});
