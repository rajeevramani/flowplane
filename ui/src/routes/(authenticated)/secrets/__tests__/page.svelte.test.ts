import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page } from 'vitest/browser';
import { writable } from 'svelte/store';
import type { SecretResponse } from '$lib/api/types';

// Mock stores and API client
vi.mock('$lib/stores/team', () => ({
	selectedTeam: writable('test-team')
}));

vi.mock('$lib/api/client', () => ({
	apiClient: {
		listSecrets: vi.fn(),
		deleteSecret: vi.fn(),
		clearAuth: vi.fn()
	}
}));

import { apiClient } from '$lib/api/client';
import SecretsPage from '../+page.svelte';

const mockSecrets: SecretResponse[] = [
	{
		id: 'secret-1',
		name: 'my-tls-cert',
		secret_type: 'tls_certificate',
		description: 'Production TLS certificate',
		version: 1,
		source: 'ui',
		team: 'test-team',
		created_at: '2026-01-15T10:00:00Z',
		updated_at: '2026-01-15T10:00:00Z',
		expires_at: null
	},
	{
		id: 'secret-2',
		name: 'api-key',
		secret_type: 'generic_secret',
		description: null,
		version: 2,
		source: 'api',
		team: 'test-team',
		created_at: '2026-02-01T12:00:00Z',
		updated_at: '2026-02-01T12:00:00Z',
		expires_at: '2027-02-01T12:00:00Z',
		backend: 'vault',
		reference: 'secret/data/api-key'
	}
];

describe('Secrets Page', () => {
	beforeEach(() => {
		vi.mocked(apiClient.listSecrets).mockReset();
	});

	it('shows loading spinner initially', async () => {
		// Keep the promise pending so the component stays in loading state
		vi.mocked(apiClient.listSecrets).mockReturnValue(new Promise(() => {}));

		render(SecretsPage);

		await expect.element(page.getByText('Loading secrets...')).toBeVisible();
	});

	it('shows error message when API fails', async () => {
		vi.mocked(apiClient.listSecrets).mockRejectedValue(
			new Error('Network connection failed')
		);

		render(SecretsPage);

		await expect.element(page.getByText('Network connection failed')).toBeVisible();
	});

	it('shows empty state when no secrets', async () => {
		vi.mocked(apiClient.listSecrets).mockResolvedValue([]);

		render(SecretsPage);

		await expect.element(page.getByText('No secrets yet')).toBeVisible();
	});

	it('renders secrets table when data loaded', async () => {
		vi.mocked(apiClient.listSecrets).mockResolvedValue(mockSecrets);

		render(SecretsPage);

		// Verify secret names appear in the table
		await expect.element(page.getByText('my-tls-cert')).toBeVisible();
		await expect.element(page.getByText('api-key', { exact: true })).toBeVisible();

		// Verify IDs rendered (unique per row, no ambiguity with filters)
		await expect.element(page.getByText('secret-1')).toBeVisible();
		await expect.element(page.getByText('secret-2')).toBeVisible();

		// Verify description text
		await expect.element(page.getByText('Production TLS certificate')).toBeVisible();
	});
});
