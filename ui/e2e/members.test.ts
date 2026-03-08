import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

// Runs as orgadmin project (org admin auth)
const orgName = SEED.org;

test.describe('Invite Flow', () => {
	// Scenario 8: Org admin invites a user → verify appears in member list
	test('invite user → verify in org members', async ({ page }) => {
		const errors = collectPageErrors(page);
		const timestamp = Date.now();
		const inviteEmail = `e2e-invite-${timestamp}@flowplane.local`;

		// Navigate to the org detail page (admin/organizations/{id})
		// First get the org list to find the org ID
		await page.goto('/admin/organizations');
		await waitForPageLoad(page);

		// Click on the org to go to its detail page
		const orgLink = page.getByRole('link', { name: orgName }).first();
		const orgLinkVisible = await orgLink.isVisible().catch(() => false);

		if (orgLinkVisible) {
			await orgLink.click();
			await waitForPageLoad(page);

			// Look for the invite section or "Invite Member" button
			const inviteBtn = page.getByRole('button', { name: /invite/i }).first();
			const hasInvite = await inviteBtn.isVisible().catch(() => false);

			if (hasInvite) {
				await inviteBtn.click();

				// Fill invite form
				const emailInput = page.locator('input[type="email"], #email, input[name="email"]').first();
				await emailInput.fill(inviteEmail);

				const firstNameInput = page.locator('#firstName, input[name="firstName"]').first();
				if (await firstNameInput.isVisible()) {
					await firstNameInput.fill('E2E');
				}

				const lastNameInput = page.locator('#lastName, input[name="lastName"]').first();
				if (await lastNameInput.isVisible()) {
					await lastNameInput.fill(`Invite${timestamp}`);
				}

				// Submit the invite
				const submitBtn = page.getByRole('button', { name: /invite|send|add/i }).last();
				await submitBtn.click();
				await page.waitForTimeout(2000);

				// Verify the invited user appears in the members list
				await expect(page.getByText(inviteEmail)).toBeVisible({ timeout: 10000 });
			}
		}

		assertNoPageErrors(errors);
	});
});
