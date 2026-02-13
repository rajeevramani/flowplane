import { test as setup, expect } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { seedTestData } from './seed-data';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const authFile = join(__dirname, '..', 'test-results', '.auth', 'admin.json');

const E2E_ADMIN = {
	email: 'admin@e2e.flowplane.local',
	password: 'E2E_SecurePassword!23',
	name: 'E2E Admin',
};

setup('authenticate', async ({ request, page }) => {
	// Bootstrap the system if needed (idempotent — works on fresh or existing DB)
	const statusResp = await request.get('/api/v1/bootstrap/status');
	const status = await statusResp.json();

	if (status.needsInitialization) {
		const bootstrapResp = await request.post('/api/v1/bootstrap/initialize', {
			data: {
				email: E2E_ADMIN.email,
				password: E2E_ADMIN.password,
				name: E2E_ADMIN.name,
			},
		});
		if (bootstrapResp.status() !== 201) {
			throw new Error(`Bootstrap failed: ${bootstrapResp.status()} ${await bootstrapResp.text()}`);
		}
	}

	// Seed test data via API (idempotent — safe to run multiple times)
	await seedTestData(request, { email: E2E_ADMIN.email, password: E2E_ADMIN.password });

	await page.goto('/login');
	await page.fill('input[name="email"]', E2E_ADMIN.email);
	await page.fill('input[name="password"]', E2E_ADMIN.password);

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
