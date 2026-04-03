import { test, expect } from '@playwright/test';
import { collectPageErrors, assertNoPageErrors, waitForPageLoad } from './helpers';
import { SEED } from './seed-data';

// Runs as orgadmin project (org admin auth)
const orgName = SEED.org;

test.describe('Agent CRUD + Grant Management', () => {
	// Scenario 3: Create agent → verify credentials shown
	test('create agent → verify credential modal', async ({ page }) => {
		const errors = collectPageErrors(page);
		const agentName = `e2e-test-agent-${Date.now()}`;

		await page.goto(`/organizations/${orgName}/agents/create`);
		await waitForPageLoad(page);

		// Fill agent name
		await page.locator('#name').fill(agentName);

		// Select at least one team
		const teamCheckbox = page.locator('input[type="checkbox"]').first();
		await teamCheckbox.check();

		// Submit
		await page.getByRole('button', { name: /create agent/i }).click();

		// Credential modal should appear with Client ID and Client Secret
		const modal = page.locator('[role="dialog"]');
		await expect(modal).toBeVisible({ timeout: 15000 });
		await expect(modal.getByText('Client ID')).toBeVisible();
		await expect(modal.getByText('Client Secret')).toBeVisible();

		// Acknowledge and close
		await modal.locator('input[type="checkbox"]').check();
		await modal.getByRole('button', { name: /close/i }).click();

		// Should redirect to agents list
		await page.waitForURL(new RegExp(`/organizations/${orgName}/agents`), { timeout: 10000 });

		// Verify agent appears in the Name column of the table.
		// Use table row locator to avoid strict mode violation — the agent name also
		// appears as a substring in the Username column (e.g. acme-corp--agentName).
		await expect(page.locator('tr', { hasText: agentName }).first()).toBeVisible();

		assertNoPageErrors(errors);
	});

	// Scenario 4: Agent detail → add grant → verify in grants table
	test('add grant to agent via PermissionMatrix', async ({ page }) => {
		const errors = collectPageErrors(page);

		// Navigate to seeded agent (or the first agent in the list)
		await page.goto(`/organizations/${orgName}/agents`);
		await waitForPageLoad(page);

		// Click "Manage" on the first agent
		const manageLink = page.getByRole('link', { name: /manage/i }).first();
		await manageLink.click();
		await waitForPageLoad(page);

		// If PermissionMatrix is visible (cp-tool agents), toggle a grant
		const matrixTable = page.locator('table').filter({ hasText: 'Resource' });
		const matrixVisible = await matrixTable.isVisible().catch(() => false);

		if (matrixVisible) {
			// Find an unchecked checkbox in the matrix and get its title for a stable locator
			const uncheckedCell = matrixTable
				.locator('input[type="checkbox"]:not(:checked)')
				.first();
			if (await uncheckedCell.isVisible()) {
				const title = await uncheckedCell.getAttribute('title');
				await uncheckedCell.check();
				// Wait for the grant to be created (network call)
				await page.waitForTimeout(1000);
				// Use a stable locator based on title to verify the specific checkbox
				const stableCheckbox = matrixTable.locator(`input[type="checkbox"][title="${title}"]`);
				await expect(stableCheckbox).toBeChecked();
			}
		}

		assertNoPageErrors(errors);
	});

	// Scenario 5: Agent detail → revoke grant → verify removed
	test('revoke grant from agent', async ({ page }) => {
		const errors = collectPageErrors(page);

		await page.goto(`/organizations/${orgName}/agents`);
		await waitForPageLoad(page);

		const manageLink = page.getByRole('link', { name: /manage/i }).first();
		await manageLink.click();
		await waitForPageLoad(page);

		// Try PermissionMatrix path first (cp-tool agents)
		const matrixTable = page.locator('table').filter({ hasText: 'Resource' });
		const matrixVisible = await matrixTable.isVisible().catch(() => false);
		let revokedViaMatrix = false;

		if (matrixVisible) {
			const checkedCell = matrixTable.locator('input[type="checkbox"]:checked').first();
			if (await checkedCell.isVisible()) {
				// Capture the title for a stable locator — the :checked selector shifts
				// to the next checked checkbox after uncheck, causing assertion timeouts
				const title = await checkedCell.getAttribute('title');
				await checkedCell.uncheck();
				await page.waitForTimeout(1000);
				const stableCheckbox = matrixTable.locator(`input[type="checkbox"][title="${title}"]`);
				await expect(stableCheckbox).not.toBeChecked();
				revokedViaMatrix = true;
			}
		}

		// Fall back to grants table Delete button (gateway agents) — only if matrix path didn't work
		if (!revokedViaMatrix) {
			// Scope to the grants table to avoid matching the "Delete Agent" header button
			const grantsTable = page.locator('table').filter({ hasText: 'Actions' });
			const deleteBtn = grantsTable.getByRole('button', { name: /delete/i }).first();
			const hasDeleteBtn = await deleteBtn.isVisible().catch(() => false);
			if (hasDeleteBtn) {
				await deleteBtn.click();
				// Confirm in the delete modal
				const confirmBtn = page
					.locator('[role="dialog"]')
					.getByRole('button', { name: /delete|confirm|remove/i });
				if (await confirmBtn.isVisible()) {
					await confirmBtn.click();
					await page.waitForTimeout(1000);
				}
			}
		}

		assertNoPageErrors(errors);
	});

	// Scenario 14: Delete agent → verify removed from list
	test('delete agent → verify removed', async ({ page }) => {
		const errors = collectPageErrors(page);
		const agentName = `e2e-test-del-${Date.now()}`;

		// First create an agent to delete
		await page.goto(`/organizations/${orgName}/agents/create`);
		await waitForPageLoad(page);
		await page.locator('#name').fill(agentName);
		await page.locator('input[type="checkbox"]').first().check();
		await page.getByRole('button', { name: /create agent/i }).click();

		// Handle credential modal
		const modal = page.locator('[role="dialog"]');
		await expect(modal).toBeVisible({ timeout: 15000 });
		await modal.locator('input[type="checkbox"]').check();
		await modal.getByRole('button', { name: /close/i }).click();
		await page.waitForURL(new RegExp(`/organizations/${orgName}/agents`));
		await waitForPageLoad(page);

		// Now delete it from the agents list
		const row = page.locator('tr', { hasText: agentName });
		await expect(row).toBeVisible();
		await row.getByRole('button', { name: /delete/i }).click();

		// Confirm delete in modal
		const deleteModal = page.locator('[role="dialog"]');
		await expect(deleteModal).toBeVisible();
		await deleteModal.getByRole('button', { name: /delete/i }).click();

		// Wait for removal and verify the row no longer appears in the table.
		// Check the table row rather than getByText to avoid matching the Username column
		// which contains the agent name as a suffix (e.g. acme-corp--agentName).
		await expect(page.locator('tr', { hasText: agentName })).not.toBeVisible({ timeout: 10000 });

		assertNoPageErrors(errors);
	});

	// Scenario 16: Permission matrix reflects correct state after grant changes
	test('permission matrix reflects correct state after grant cycle', async ({ page }) => {
		const errors = collectPageErrors(page);

		await page.goto(`/organizations/${orgName}/agents`);
		await waitForPageLoad(page);

		const manageLink = page.getByRole('link', { name: /manage/i }).first();
		await manageLink.click();
		await waitForPageLoad(page);

		const matrixTable = page.locator('table').filter({ hasText: 'Resource' });
		const matrixVisible = await matrixTable.isVisible().catch(() => false);

		if (matrixVisible) {
			// Use title attribute for a stable locator across re-renders
			const firstCheckbox = matrixTable.locator('input[type="checkbox"]').first();
			const title = await firstCheckbox.getAttribute('title');
			const checkbox = matrixTable.locator(`input[type="checkbox"][title="${title}"]`);
			const wasChecked = await checkbox.isChecked();

			// Toggle on
			if (!wasChecked) {
				await checkbox.check();
				await page.waitForTimeout(1000);
				await expect(checkbox).toBeChecked();
			}

			// Toggle off
			await checkbox.uncheck();
			await page.waitForTimeout(1000);
			await expect(checkbox).not.toBeChecked();

			// Toggle back to original state
			if (wasChecked) {
				await checkbox.check();
				await page.waitForTimeout(1000);
				await expect(checkbox).toBeChecked();
			}
		}

		assertNoPageErrors(errors);
	});
});
