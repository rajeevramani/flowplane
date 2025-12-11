import { writable, derived, get } from 'svelte/store';
import { apiClient } from '$lib/api/client';
import type {
	StatsOverviewResponse,
	ClustersStatsResponse,
	ClusterStatsResponse
} from '$lib/api/types';

// Store for whether the stats dashboard is enabled (instance-level)
export const statsEnabled = writable<boolean | null>(null);

// Loading state for stats enabled check
export const statsEnabledLoading = writable<boolean>(false);

// Store for current team's stats overview
export const statsOverview = writable<StatsOverviewResponse | null>(null);

// Store for current team's cluster stats
export const clusterStats = writable<ClustersStatsResponse | null>(null);

// Loading state for stats data
export const statsLoading = writable<boolean>(false);

// Error state
export const statsError = writable<string | null>(null);

// Last refresh timestamp
export const lastRefresh = writable<Date | null>(null);

// Polling interval ID for cleanup
let pollingInterval: ReturnType<typeof setInterval> | null = null;

/**
 * Check if the stats dashboard is enabled at the instance level.
 * Caches the result in the store.
 */
export async function checkStatsEnabled(): Promise<boolean> {
	// Return cached value if available
	const current = get(statsEnabled);
	if (current !== null) {
		return current;
	}

	statsEnabledLoading.set(true);
	try {
		const response = await apiClient.isStatsEnabled();
		statsEnabled.set(response.enabled);
		return response.enabled;
	} catch (error) {
		console.error('Failed to check stats enabled:', error);
		statsEnabled.set(false);
		return false;
	} finally {
		statsEnabledLoading.set(false);
	}
}

/**
 * Refresh the stats enabled status from the server.
 * Forces a fresh check, ignoring cached value.
 */
export async function refreshStatsEnabled(): Promise<boolean> {
	statsEnabled.set(null); // Clear cache
	return checkStatsEnabled();
}

/**
 * Load stats overview for a specific team.
 */
export async function loadStatsOverview(team: string): Promise<void> {
	if (!team) return;

	statsLoading.set(true);
	statsError.set(null);

	try {
		const overview = await apiClient.getStatsOverview(team);
		statsOverview.set(overview);
		lastRefresh.set(new Date());
	} catch (error) {
		const message = error instanceof Error ? error.message : 'Failed to load stats overview';
		statsError.set(message);
		console.error('Failed to load stats overview:', error);
	} finally {
		statsLoading.set(false);
	}
}

/**
 * Load cluster stats for a specific team.
 */
export async function loadClusterStats(team: string): Promise<void> {
	if (!team) return;

	statsLoading.set(true);
	statsError.set(null);

	try {
		const clusters = await apiClient.getClusterStats(team);
		clusterStats.set(clusters);
		lastRefresh.set(new Date());
	} catch (error) {
		const message = error instanceof Error ? error.message : 'Failed to load cluster stats';
		statsError.set(message);
		console.error('Failed to load cluster stats:', error);
	} finally {
		statsLoading.set(false);
	}
}

/**
 * Load all stats (overview + clusters) for a team.
 */
export async function loadAllStats(team: string): Promise<void> {
	if (!team) return;

	statsLoading.set(true);
	statsError.set(null);

	try {
		const [overview, clusters] = await Promise.all([
			apiClient.getStatsOverview(team),
			apiClient.getClusterStats(team)
		]);
		statsOverview.set(overview);
		clusterStats.set(clusters);
		lastRefresh.set(new Date());
	} catch (error) {
		const message = error instanceof Error ? error.message : 'Failed to load stats';
		statsError.set(message);
		console.error('Failed to load stats:', error);
	} finally {
		statsLoading.set(false);
	}
}

/**
 * Start polling for stats updates.
 * @param team - Team to poll stats for
 * @param intervalMs - Polling interval in milliseconds (default: 10000)
 */
export function startPolling(team: string, intervalMs: number = 10000): void {
	// Stop any existing polling
	stopPolling();

	// Load immediately
	loadAllStats(team);

	// Start polling
	pollingInterval = setInterval(() => {
		loadAllStats(team);
	}, intervalMs);
}

/**
 * Stop polling for stats updates.
 */
export function stopPolling(): void {
	if (pollingInterval) {
		clearInterval(pollingInterval);
		pollingInterval = null;
	}
}

/**
 * Clear all stats data from stores.
 * Call this when changing teams or unmounting.
 */
export function clearStats(): void {
	stopPolling();
	statsOverview.set(null);
	clusterStats.set(null);
	statsError.set(null);
	lastRefresh.set(null);
}

// Derived store for health status color
export const healthStatusColor = derived(statsOverview, ($overview) => {
	if (!$overview) return 'gray';
	switch ($overview.healthStatus) {
		case 'healthy':
			return 'green';
		case 'degraded':
			return 'yellow';
		case 'unhealthy':
			return 'red';
		default:
			return 'gray';
	}
});

// Derived store for cluster health summary
export const clusterHealthSummary = derived(clusterStats, ($clusters) => {
	if (!$clusters) {
		return { healthy: 0, degraded: 0, unhealthy: 0, total: 0 };
	}

	const counts = { healthy: 0, degraded: 0, unhealthy: 0 };
	for (const cluster of $clusters.clusters) {
		if (cluster.healthStatus === 'healthy') counts.healthy++;
		else if (cluster.healthStatus === 'degraded') counts.degraded++;
		else counts.unhealthy++;
	}

	return {
		...counts,
		total: $clusters.count
	};
});
