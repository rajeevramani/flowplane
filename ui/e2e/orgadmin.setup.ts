import { test as setup, expect } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { readFileSync } from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const authFile = join(__dirname, '..', 'playwright', '.auth', 'orgadmin.json');

// Org admin credentials (demo user created by seed-demo.sh)
const ORG_ADMIN_EMAIL = process.env.E2E_ORG_ADMIN_EMAIL ?? 'demo@acme-corp.com';
const ORG_ADMIN_PASSWORD = 'E2eOrgAdmin!SecurePass1';

// Zitadel API configuration
const ZITADEL_HOST = process.env.ZITADEL_HOST ?? 'http://localhost:8081';

/**
 * Load the Zitadel admin PAT from .env.zitadel or environment.
 */
function loadZitadelPat(): string {
	if (process.env.ZITADEL_ADMIN_PAT) {
		return process.env.ZITADEL_ADMIN_PAT;
	}
	const envPath = join(__dirname, '..', '..', '.env.zitadel');
	try {
		const content = readFileSync(envPath, 'utf-8');
		for (const line of content.split('\n')) {
			const match = line.match(/^ZITADEL_ADMIN_PAT=(.+)$/);
			if (match) {
				return match[1].trim();
			}
		}
	} catch {
		// File not found
	}
	throw new Error(
		'ZITADEL_ADMIN_PAT not found. Run `make up` first to generate .env.zitadel, ' +
			'or set ZITADEL_ADMIN_PAT environment variable.'
	);
}

async function findZitadelUserId(pat: string, email: string): Promise<string> {
	const resp = await fetch(`${ZITADEL_HOST}/v2/users`, {
		method: 'POST',
		headers: {
			Authorization: `Bearer ${pat}`,
			'Content-Type': 'application/json'
		},
		body: JSON.stringify({
			queries: [{ emailQuery: { emailAddress: email } }]
		})
	});
	if (!resp.ok) {
		throw new Error(`Zitadel user search failed (${resp.status}): ${await resp.text()}`);
	}
	const data = (await resp.json()) as {
		result?: Array<{ userId?: string; id?: string }>;
	};
	const userId = data.result?.[0]?.userId ?? data.result?.[0]?.id;
	if (!userId) {
		throw new Error(`Zitadel user not found: ${email}`);
	}
	return userId;
}

async function setZitadelPassword(pat: string, userId: string, password: string): Promise<void> {
	const resp = await fetch(`${ZITADEL_HOST}/v2/users/${userId}/password`, {
		method: 'POST',
		headers: {
			Authorization: `Bearer ${pat}`,
			'Content-Type': 'application/json'
		},
		body: JSON.stringify({
			newPassword: { password, changeRequired: false }
		})
	});
	if (!resp.ok) {
		throw new Error(`Failed to set Zitadel password (${resp.status}): ${await resp.text()}`);
	}
}

async function performOidcLogin(
	page: import('@playwright/test').Page,
	email: string,
	password: string
): Promise<void> {
	await page.goto('/login');
	await page.getByRole('button', { name: /sign in/i }).click();
	await page.waitForURL(/localhost:8081\/ui\/login/, { timeout: 15000 });

	// Step 1: Username
	await page.locator('#loginName').fill(email);
	await page.locator('#submit-button').click();

	// Step 2: Password
	await page.locator('#password').waitFor({ state: 'visible', timeout: 10000 });
	await page.locator('#password').fill(password);
	await page.locator('#submit-button').click();

	// Step 3: Handle optional Zitadel MFA setup prompt (click "Skip" if shown)
	const skipButton = page.getByRole('button', { name: 'Skip' });
	try {
		await skipButton.waitFor({ state: 'visible', timeout: 5000 });
		await skipButton.click();
	} catch {
		// No MFA prompt — Zitadel redirected directly
	}

	// Wait for redirect back to app
	await page.waitForURL(/\/dashboard/, { timeout: 30000 });
	await expect(page.locator('body')).toBeVisible();
}

setup('authenticate as org admin', async ({ page }) => {
	const pat = loadZitadelPat();

	console.log('[orgadmin-setup] Setting org admin password via Zitadel API...');
	const userId = await findZitadelUserId(pat, ORG_ADMIN_EMAIL);
	await setZitadelPassword(pat, userId, ORG_ADMIN_PASSWORD);
	console.log(`[orgadmin-setup] Password set for ${ORG_ADMIN_EMAIL} (userId: ${userId})`);

	console.log('[orgadmin-setup] Starting OIDC login flow...');
	await performOidcLogin(page, ORG_ADMIN_EMAIL, ORG_ADMIN_PASSWORD);
	console.log('[orgadmin-setup] OIDC login complete, saving browser state');

	await page.context().storageState({ path: authFile });
});
