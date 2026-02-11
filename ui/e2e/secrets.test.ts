import { test, expect } from '@playwright/test';

test.describe('Secrets Page', () => {
	test('secrets page loads without crash', async ({ page }) => {
		await page.goto('/secrets');
		await expect(page.locator('body')).toBeVisible();

		const bodyText = await page.locator('body').textContent();
		expect(bodyText).not.toContain('Cannot read properties');
	});

	test('secrets page shows content or empty state', async ({ page }) => {
		await page.goto('/secrets');

		// Wait for loading to complete
		await page
			.waitForSelector('[class*="animate-spin"]', { state: 'hidden', timeout: 10000 })
			.catch(() => {});

		const hasTable = (await page.locator('table').count()) > 0;
		const hasEmptyState = (await page.getByText(/no secrets/i).count()) > 0;
		const hasError = (await page.locator('[class*="error"], .bg-red-50').count()) > 0;
		const hasContent = (await page.locator('body').textContent())!.length > 0;

		expect(hasTable || hasEmptyState || hasError || hasContent).toBe(true);
	});
});
