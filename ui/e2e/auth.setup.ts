import { test as setup, expect } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const authFile = join(__dirname, '..', 'test-results', '.auth', 'user.json');

setup('authenticate', async ({ page }) => {
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

	// Wait for redirect to dashboard
	await page.waitForURL(/\/dashboard/, { timeout: 15000 });
	await expect(page.locator('body')).toBeVisible();

	// Save storage state (cookies + localStorage + sessionStorage)
	await page.context().storageState({ path: authFile });
});
