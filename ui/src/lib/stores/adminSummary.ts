import { writable } from 'svelte/store';
import { apiClient } from '$lib/api/client';
import type { AdminResourceSummary } from '$lib/api/types';

const CACHE_TTL = 60_000; // 60 seconds

let lastFetch = 0;
let cachedData: AdminResourceSummary | null = null;
let fetchPromise: Promise<AdminResourceSummary> | null = null;

export const adminSummary = writable<AdminResourceSummary | null>(null);
export const adminSummaryLoading = writable(false);
export const adminSummaryError = writable<string | null>(null);

export async function getAdminSummary(): Promise<AdminResourceSummary> {
	// Return cached if fresh
	if (cachedData && Date.now() - lastFetch < CACHE_TTL) {
		return cachedData;
	}

	// Deduplicate concurrent calls
	if (fetchPromise) return fetchPromise;

	adminSummaryLoading.set(true);
	adminSummaryError.set(null);

	fetchPromise = apiClient
		.getAdminResourceSummary()
		.then((data) => {
			cachedData = data;
			lastFetch = Date.now();
			adminSummary.set(data);
			return data;
		})
		.catch((err: unknown) => {
			const message = err instanceof Error ? err.message : 'Failed to load summary';
			adminSummaryError.set(message);
			throw err;
		})
		.finally(() => {
			adminSummaryLoading.set(false);
			fetchPromise = null;
		});

	return fetchPromise;
}

export function invalidateAdminSummary(): void {
	cachedData = null;
	lastFetch = 0;
}
