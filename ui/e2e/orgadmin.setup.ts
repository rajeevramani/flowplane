import { test as setup, expect } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { ORG_ADMIN } from './seed-data';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const authFile = join(__dirname, '..', 'test-results', '.auth', 'orgadmin.json');

setup('authenticate as org-admin', async ({ page }) => {
	await page.goto('/login');
	await page.fill('input[name="email"]', ORG_ADMIN.email);
	await page.fill('input[name="password"]', ORG_ADMIN.password);

	const [response] = await Promise.all([
		page.waitForResponse(
			(resp) => resp.url().includes('/api/v1/auth/login') && resp.status() === 200
		),
		page.click('button[type="submit"]')
	]);

	await page.waitForURL(/\/dashboard/, { timeout: 15000 });
	await expect(page.locator('body')).toBeVisible();

	await page.context().storageState({ path: authFile });
});
