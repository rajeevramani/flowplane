import { test, expect } from '@playwright/test';

// Auth tests need their own fresh context (no pre-existing session)
test.use({ storageState: { cookies: [], origins: [] } });

test.describe('Authentication', () => {
	test('login page loads', async ({ page }) => {
		await page.goto('/login');
		await expect(page.locator('body')).toBeVisible();
		await expect(page.getByRole('button', { name: /sign in/i })).toBeVisible();
	});

	test('login page shows form fields', async ({ page }) => {
		await page.goto('/login');
		await expect(page.locator('input[name="email"]')).toBeVisible();
		await expect(page.locator('input[name="password"]')).toBeVisible();
	});

	test('login with valid credentials redirects to dashboard', async ({ page }) => {
		await page.goto('/login');
		await page.fill('input[name="email"]', 'admin@example.com');
		await page.fill('input[name="password"]', 'SecurePassword!23');

		// Wait for the login API response before checking navigation
		const [response] = await Promise.all([
			page.waitForResponse(
				(resp) => resp.url().includes('/api/v1/auth/login') && resp.status() === 200
			),
			page.click('button[type="submit"]')
		]);

		await page.waitForURL(/\/dashboard/, { timeout: 15000 });
		await expect(page.locator('body')).toBeVisible();
	});

	test('login with invalid credentials shows error', async ({ page }) => {
		await page.goto('/login');
		await page.fill('input[name="email"]', 'bad@example.com');
		await page.fill('input[name="password"]', 'wrongpassword');
		await page.click('button[type="submit"]');
		// Should stay on login page and show an error
		await expect(page.locator('.bg-red-50')).toBeVisible({ timeout: 10000 });
	});
});
