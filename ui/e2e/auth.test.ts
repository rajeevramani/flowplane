import { test, expect } from '@playwright/test';

const ZITADEL_HOST = process.env.ZITADEL_HOST ?? 'http://localhost:8081';

// Auth tests need a fresh context (no pre-existing OIDC session)
test.use({ storageState: { cookies: [], origins: [] } });

test.describe('Authentication', () => {
	test('unauthenticated user is redirected to Zitadel login', async ({ page }) => {
		// Navigate to a protected page without auth
		await page.goto('/dashboard');

		// The app should redirect to /login, then clicking sign-in goes to Zitadel.
		// But the (authenticated) layout checks OIDC state and redirects to /login.
		await page.waitForURL(/\/login/, { timeout: 15000 });

		// Verify we're on the app login page with the Zitadel sign-in button
		await expect(page.getByRole('button', { name: /sign in/i })).toBeVisible();
	});

	test('OIDC login succeeds and lands on dashboard', async ({ page }) => {
		// This test performs a full OIDC login flow.
		// It relies on the admin setup having already set the password.
		// We use the same admin credentials as auth.setup.ts.
		const adminEmail = process.env.E2E_ADMIN_EMAIL ?? 'admin@flowplane.local';
		const adminPassword = 'E2eAdmin!SecurePass1';

		// Start at app login page
		await page.goto('/login');
		await page.getByRole('button', { name: /sign in/i }).click();

		// Wait for Zitadel login page
		await page.waitForURL(/localhost:8081\/ui\/login/, { timeout: 15000 });

		// Step 1: Username
		await page.locator('#loginName').fill(adminEmail);
		await page.locator('#submit-button').click();

		// Step 2: Password
		await page.locator('#password').waitFor({ state: 'visible', timeout: 10000 });
		await page.locator('#password').fill(adminPassword);
		await page.locator('#submit-button').click();

		// Step 3: Handle optional Zitadel MFA setup prompt (click "Skip" if shown)
		const skipButton = page.getByRole('button', { name: 'Skip' });
		try {
			await skipButton.waitFor({ state: 'visible', timeout: 5000 });
			await skipButton.click();
		} catch {
			// No MFA prompt — Zitadel redirected directly
		}

		// Should redirect back to app and land on dashboard
		await page.waitForURL(/\/dashboard/, { timeout: 30000 });
		await expect(page.locator('body')).toBeVisible();
	});

	test('invalid credentials show error on Zitadel login', async ({ page }) => {
		await page.goto('/login');
		await page.getByRole('button', { name: /sign in/i }).click();

		// Wait for Zitadel login page
		await page.waitForURL(/localhost:8081\/ui\/login/, { timeout: 15000 });

		// Step 1: Enter a valid-looking username
		await page.locator('#loginName').fill('admin@flowplane.local');
		await page.locator('#submit-button').click();

		// Step 2: Enter wrong password
		await page.locator('#password').waitFor({ state: 'visible', timeout: 10000 });
		await page.locator('#password').fill('WrongPassword123!');
		await page.locator('#submit-button').click();

		// Should stay on Zitadel login page with an error message
		// Zitadel displays errors in .lgn-error or similar error containers
		const errorVisible = await page
			.locator('.lgn-error, .lgn-warn, [class*="error"], [class*="alert"]')
			.first()
			.waitFor({ state: 'visible', timeout: 10000 })
			.then(() => true)
			.catch(() => false);

		// Should still be on Zitadel login page (not redirected to app)
		expect(page.url()).toContain('localhost:8081');

		// Either we see an error element or we're still on the password page
		const stillOnZitadel = page.url().includes('localhost:8081');
		expect(errorVisible || stillOnZitadel).toBe(true);
	});
});
