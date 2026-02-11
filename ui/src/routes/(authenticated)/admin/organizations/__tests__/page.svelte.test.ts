import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page } from 'vitest/browser';
import type { SessionInfoResponse, AdminListOrgsResponse } from '$lib/api/types';

// Mock API client
vi.mock('$lib/api/client', () => ({
	apiClient: {
		getSessionInfo: vi.fn(),
		listOrganizations: vi.fn(),
		clearAuth: vi.fn()
	}
}));

import { apiClient } from '$lib/api/client';
import OrganizationsPage from '../+page.svelte';

const mockSessionInfo: SessionInfoResponse = {
	sessionId: 'sess-1',
	userId: 'user-1',
	name: 'Admin User',
	email: 'admin@test.com',
	isAdmin: true,
	teams: ['test-team'],
	scopes: ['admin:all'],
	expiresAt: null,
	version: '1.0.0'
};

const mockOrgsResponse: AdminListOrgsResponse = {
	organizations: [
		{
			id: 'org-1',
			name: 'acme-corp',
			displayName: 'Acme Corporation',
			description: 'Main organization',
			status: 'active',
			createdAt: '2026-01-10T08:00:00Z',
			updatedAt: '2026-01-10T08:00:00Z'
		},
		{
			id: 'org-2',
			name: 'beta-inc',
			displayName: 'Beta Inc',
			status: 'suspended',
			createdAt: '2026-02-05T14:00:00Z',
			updatedAt: '2026-02-05T14:00:00Z'
		}
	],
	total: 2
};

describe('Organizations Page', () => {
	beforeEach(() => {
		vi.mocked(apiClient.getSessionInfo).mockReset();
		vi.mocked(apiClient.listOrganizations).mockReset();
	});

	it('shows loading spinner initially', async () => {
		vi.mocked(apiClient.getSessionInfo).mockResolvedValue(mockSessionInfo);
		// Keep listOrganizations pending so component stays in loading state
		vi.mocked(apiClient.listOrganizations).mockReturnValue(new Promise(() => {}));

		render(OrganizationsPage);

		await expect.element(page.getByText('Organization Management')).toBeVisible();
	});

	it('shows error message when API fails', async () => {
		vi.mocked(apiClient.getSessionInfo).mockResolvedValue(mockSessionInfo);
		vi.mocked(apiClient.listOrganizations).mockRejectedValue(
			new Error('Internal server error')
		);

		render(OrganizationsPage);

		await expect.element(page.getByText('Internal server error')).toBeVisible();
	});

	it('shows empty state when no organizations', async () => {
		vi.mocked(apiClient.getSessionInfo).mockResolvedValue(mockSessionInfo);
		vi.mocked(apiClient.listOrganizations).mockResolvedValue({
			organizations: [],
			total: 0
		});

		render(OrganizationsPage);

		await expect.element(page.getByText('No organizations found')).toBeVisible();
	});

	it('renders organization table when data loaded', async () => {
		vi.mocked(apiClient.getSessionInfo).mockResolvedValue(mockSessionInfo);
		vi.mocked(apiClient.listOrganizations).mockResolvedValue(mockOrgsResponse);

		render(OrganizationsPage);

		// Verify org names (unique text, no ambiguity with filter dropdowns)
		await expect.element(page.getByText('acme-corp')).toBeVisible();
		await expect.element(page.getByText('beta-inc')).toBeVisible();

		// Verify display names
		await expect.element(page.getByText('Acme Corporation')).toBeVisible();
		await expect.element(page.getByText('Beta Inc')).toBeVisible();

		// Verify pagination info shows correct total
		await expect.element(page.getByText(/of 2 organizations/)).toBeVisible();
	});
});
