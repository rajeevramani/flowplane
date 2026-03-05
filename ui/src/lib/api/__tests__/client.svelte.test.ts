import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock SvelteKit modules (hoisted before imports)
vi.mock('$app/navigation', () => ({
	goto: vi.fn()
}));

vi.mock('$env/dynamic/public', () => ({
	env: { PUBLIC_API_BASE: 'http://test-api:8080' }
}));

vi.mock('$lib/stores/org', () => ({
	currentOrg: { set: vi.fn() }
}));

import { apiClient } from '../client';
import { goto } from '$app/navigation';

function mockResponse(
	body: unknown,
	options: { status?: number; statusText?: string; headers?: Record<string, string> } = {}
): Response {
	const { status = 200, statusText = 'OK', headers = {} } = options;
	if (!headers['Content-Type']) {
		headers['Content-Type'] = 'application/json';
	}
	return new Response(JSON.stringify(body), { status, statusText, headers });
}

describe('ApiClient', () => {
	beforeEach(() => {
		vi.restoreAllMocks();
		apiClient.clearAuth();
	});

	it('throws on 4xx/5xx with error message from body', async () => {
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse({ message: 'Validation failed' }, { status: 400, statusText: 'Bad Request' })
		);

		await expect(apiClient.get('/test')).rejects.toThrow('Validation failed');
	});

	it('redirects to /login on 401', async () => {
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse(
				{ message: 'Not authenticated' },
				{ status: 401, statusText: 'Unauthorized' }
			)
		);

		await expect(apiClient.get('/test')).rejects.toThrow(
			'Unauthorized - redirecting to login'
		);
		expect(goto).toHaveBeenCalledWith('/login');
	});

	it('returns parsed JSON on success', async () => {
		const data = { id: '123', name: 'test-resource' };
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(mockResponse(data));

		const result = await apiClient.get('/test');
		expect(result).toEqual(data);
	});

	it('calls fetch with correct URL and headers for GET', async () => {
		const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse({ ok: true })
		);

		await apiClient.get('/api/v1/test');

		expect(fetchSpy).toHaveBeenCalledWith(
			'http://test-api:8080/api/v1/test',
			expect.objectContaining({
				method: 'GET',
				credentials: 'include'
			})
		);
	});

	it('listAgentGrants calls correct endpoint', async () => {
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse({ grants: [{ id: 'g1', grantType: 'cp-tool', createdBy: 'admin', createdAt: '2026-03-01T00:00:00Z' }] })
		);

		const result = await apiClient.listAgentGrants('acme-corp', 'my-agent');
		expect(result.grants).toHaveLength(1);
		expect(result.grants[0].grantType).toBe('cp-tool');
	});

	it('createAgentGrant sends correct payload', async () => {
		const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse({ id: 'g2', grantType: 'cp-tool', resourceType: 'clusters', action: 'read', team: 'engineering', createdBy: 'admin', createdAt: '2026-03-01T00:00:00Z' })
		);

		await apiClient.createAgentGrant('acme-corp', 'my-agent', {
			grantType: 'cp-tool',
			resourceType: 'clusters',
			action: 'read',
			team: 'engineering'
		});

		const call = fetchSpy.mock.calls[0];
		expect(call[0]).toContain('/api/v1/orgs/acme-corp/agents/my-agent/grants');
		expect(call[1]).toEqual(
			expect.objectContaining({ method: 'POST' })
		);
		const body = JSON.parse(call[1]?.body as string);
		expect(body.team).toBe('engineering');
		expect(body.grantType).toBe('cp-tool');
	});

	it('deleteAgentGrant calls correct endpoint', async () => {
		const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse(null, { status: 204 })
		);

		await apiClient.deleteAgentGrant('acme-corp', 'my-agent', 'grant-123');

		const call = fetchSpy.mock.calls[0];
		expect(call[0]).toContain('/api/v1/orgs/acme-corp/agents/my-agent/grants/grant-123');
		expect(call[1]).toEqual(
			expect.objectContaining({ method: 'DELETE' })
		);
	});

	it('post includes CSRF token and body', async () => {
		// First, set CSRF token via a response header
		const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			mockResponse({ ok: true }, { headers: { 'X-CSRF-Token': 'csrf-abc-123' } })
		);
		// GET request triggers handleResponse which stores the CSRF token
		await apiClient.get('/setup');

		// Now POST
		fetchSpy.mockResolvedValue(mockResponse({ created: true }));
		await apiClient.post('/api/v1/resources', { name: 'new-item' });

		const postCall = fetchSpy.mock.calls[1];
		expect(postCall[0]).toBe('http://test-api:8080/api/v1/resources');
		expect(postCall[1]).toEqual(
			expect.objectContaining({
				method: 'POST',
				body: JSON.stringify({ name: 'new-item' }),
				credentials: 'include'
			})
		);
		const headers = postCall[1]?.headers as Record<string, string>;
		expect(headers['X-CSRF-Token']).toBe('csrf-abc-123');
	});
});
