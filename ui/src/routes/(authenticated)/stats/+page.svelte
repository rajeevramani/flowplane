<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { selectedTeam } from '$lib/stores/team';
	import {
		statsEnabled,
		statsOverview,
		clusterStats,
		statsLoading,
		statsError,
		lastRefresh,
		checkStatsEnabled,
		loadAllStats,
		startPolling,
		stopPolling,
		clearStats,
		healthStatusColor,
		clusterHealthSummary
	} from '$lib/stores/stats';
	import type { Unsubscriber } from 'svelte/store';

	let currentTeam = $state<string>('');
	let isEnabled = $state<boolean | null>(null);
	let overview = $state($statsOverview);
	let clusters = $state($clusterStats);
	let loading = $state($statsLoading);
	let error = $state($statsError);
	let refresh = $state($lastRefresh);
	let healthColor = $state($healthStatusColor);
	let healthSummary = $state($clusterHealthSummary);
	let pollingEnabled = $state(false);
	let unsubscribers: Unsubscriber[] = [];

	// Computed values
	const errorRatePercent = $derived(overview ? (overview.errorRate * 100).toFixed(1) : '0.0');
	const refreshTime = $derived(
		refresh ? refresh.toLocaleTimeString() : 'Never'
	);

	function togglePolling() {
		if (pollingEnabled) {
			stopPolling();
			pollingEnabled = false;
		} else if (currentTeam) {
			startPolling(currentTeam, 10000);
			pollingEnabled = true;
		}
	}

	function handleRefresh() {
		if (currentTeam) {
			loadAllStats(currentTeam);
		}
	}

	function getHealthBadgeClasses(status: string): string {
		switch (status) {
			case 'healthy':
				return 'bg-green-100 text-green-800';
			case 'degraded':
				return 'bg-yellow-100 text-yellow-800';
			case 'unhealthy':
				return 'bg-red-100 text-red-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	function getHealthDotClasses(status: string): string {
		switch (status) {
			case 'healthy':
				return 'bg-green-500';
			case 'degraded':
				return 'bg-yellow-500';
			case 'unhealthy':
				return 'bg-red-500';
			default:
				return 'bg-gray-400';
		}
	}

	onMount(async () => {
		// Check if stats dashboard is enabled
		const enabled = await checkStatsEnabled();
		isEnabled = enabled;

		if (!enabled) {
			return;
		}

		// Subscribe to stores
		unsubscribers.push(
			statsOverview.subscribe((v) => (overview = v)),
			clusterStats.subscribe((v) => (clusters = v)),
			statsLoading.subscribe((v) => (loading = v)),
			statsError.subscribe((v) => (error = v)),
			lastRefresh.subscribe((v) => (refresh = v)),
			healthStatusColor.subscribe((v) => (healthColor = v)),
			clusterHealthSummary.subscribe((v) => (healthSummary = v)),
			selectedTeam.subscribe(async (team) => {
				if (team && team !== currentTeam) {
					currentTeam = team;
					// Clear old data and load new
					clearStats();
					await loadAllStats(team);
				}
			})
		);
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
		stopPolling();
	});
</script>

<svelte:head>
	<title>Envoy Stats Dashboard - Flowplane</title>
</svelte:head>

{#if isEnabled === null}
	<!-- Loading state -->
	<div class="flex items-center justify-center py-24">
		<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
	</div>
{:else if isEnabled === false}
	<!-- Stats dashboard disabled -->
	<div class="flex flex-col items-center justify-center py-24">
		<svg class="h-16 w-16 text-gray-400 mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
			<path
				stroke-linecap="round"
				stroke-linejoin="round"
				stroke-width="2"
				d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z"
			/>
		</svg>
		<h2 class="text-xl font-semibold text-gray-900 mb-2">Stats Dashboard Not Enabled</h2>
		<p class="text-gray-600">
			The Envoy Stats Dashboard has not been enabled by an administrator.
		</p>
	</div>
{:else}
	<!-- Header -->
	<div class="mb-8 flex items-center justify-between">
		<div>
			<h1 class="text-2xl font-bold text-gray-900">Envoy Stats Dashboard</h1>
			<p class="mt-1 text-sm text-gray-500">
				Real-time Envoy proxy metrics for team: <span class="font-medium">{currentTeam || 'None selected'}</span>
			</p>
		</div>
		<div class="flex items-center gap-3">
			<!-- Last refresh time -->
			<span class="text-sm text-gray-500">
				Last updated: {refreshTime}
			</span>

			<!-- Polling toggle -->
			<button
				onclick={togglePolling}
				class="inline-flex items-center px-3 py-2 border rounded-md text-sm font-medium transition-colors {pollingEnabled
					? 'border-green-500 bg-green-50 text-green-700 hover:bg-green-100'
					: 'border-gray-300 bg-white text-gray-700 hover:bg-gray-50'}"
			>
				{#if pollingEnabled}
					<span class="relative flex h-2 w-2 mr-2">
						<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-75"></span>
						<span class="relative inline-flex rounded-full h-2 w-2 bg-green-500"></span>
					</span>
					Live
				{:else}
					<svg class="h-4 w-4 mr-1" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z" />
					</svg>
					Paused
				{/if}
			</button>

			<!-- Manual refresh -->
			<button
				onclick={handleRefresh}
				disabled={loading}
				class="inline-flex items-center px-3 py-2 border border-gray-300 rounded-md text-sm font-medium text-gray-700 bg-white hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
			>
				<svg class="h-4 w-4 mr-1 {loading ? 'animate-spin' : ''}" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
				</svg>
				Refresh
			</button>
		</div>
	</div>

	<!-- Error message -->
	{#if error}
		<div class="mb-6 bg-red-50 border-l-4 border-red-500 p-4 rounded-md">
			<div class="flex">
				<svg class="h-5 w-5 text-red-500" viewBox="0 0 20 20" fill="currentColor">
					<path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z" clip-rule="evenodd" />
				</svg>
				<p class="ml-3 text-sm text-red-700">{error}</p>
			</div>
		</div>
	{/if}

	<!-- Loading state -->
	{#if loading && !overview}
		<div class="flex items-center justify-center py-24">
			<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
		</div>
	{:else if overview}
		<!-- Overview Stats Cards -->
		<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
			<!-- RPS Card -->
			<div class="bg-white rounded-lg border border-gray-200 p-6">
				<div class="flex items-start justify-between">
					<div>
						<h3 class="text-sm font-medium text-gray-600 mb-1">Requests/sec</h3>
						<p class="text-3xl font-bold text-gray-900">{overview.totalRps.toFixed(1)}</p>
					</div>
					<div class="p-3 rounded-lg bg-blue-100 text-blue-600">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 7h8m0 0v8m0-8l-8 8-4-4-6 6" />
						</svg>
					</div>
				</div>
			</div>

			<!-- Connections Card -->
			<div class="bg-white rounded-lg border border-gray-200 p-6">
				<div class="flex items-start justify-between">
					<div>
						<h3 class="text-sm font-medium text-gray-600 mb-1">Active Connections</h3>
						<p class="text-3xl font-bold text-gray-900">{overview.totalConnections.toLocaleString()}</p>
					</div>
					<div class="p-3 rounded-lg bg-purple-100 text-purple-600">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8.111 16.404a5.5 5.5 0 017.778 0M12 20h.01m-7.08-7.071c3.904-3.905 10.236-3.905 14.141 0M1.394 9.393c5.857-5.857 15.355-5.857 21.213 0" />
						</svg>
					</div>
				</div>
			</div>

			<!-- Error Rate Card -->
			<div class="bg-white rounded-lg border border-gray-200 p-6">
				<div class="flex items-start justify-between">
					<div>
						<h3 class="text-sm font-medium text-gray-600 mb-1">Error Rate</h3>
						<p class="text-3xl font-bold {overview.errorRate > 0.05 ? 'text-red-600' : overview.errorRate > 0.01 ? 'text-yellow-600' : 'text-gray-900'}">
							{errorRatePercent}%
						</p>
					</div>
					<div class="p-3 rounded-lg {overview.errorRate > 0.05 ? 'bg-red-100 text-red-600' : overview.errorRate > 0.01 ? 'bg-yellow-100 text-yellow-600' : 'bg-green-100 text-green-600'}">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
						</svg>
					</div>
				</div>
			</div>

			<!-- P99 Latency Card -->
			<div class="bg-white rounded-lg border border-gray-200 p-6">
				<div class="flex items-start justify-between">
					<div>
						<h3 class="text-sm font-medium text-gray-600 mb-1">P99 Latency</h3>
						<p class="text-3xl font-bold text-gray-900">{overview.p99LatencyMs.toFixed(0)} ms</p>
					</div>
					<div class="p-3 rounded-lg bg-orange-100 text-orange-600">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
						</svg>
					</div>
				</div>
			</div>
		</div>

		<!-- Cluster Health Summary -->
		<div class="mb-8">
			<div class="flex items-center justify-between mb-4">
				<h2 class="text-lg font-semibold text-gray-900">Cluster Health</h2>
				<span class="inline-flex items-center px-3 py-1 rounded-full text-sm font-medium {getHealthBadgeClasses(overview.healthStatus)}">
					<span class="h-2 w-2 rounded-full {getHealthDotClasses(overview.healthStatus)} mr-2"></span>
					{overview.healthStatus.charAt(0).toUpperCase() + overview.healthStatus.slice(1)}
				</span>
			</div>

			<!-- Health Summary Bar -->
			<div class="bg-white rounded-lg border border-gray-200 p-6">
				<div class="flex items-center gap-8 mb-4">
					<div class="flex items-center">
						<span class="h-3 w-3 rounded-full bg-green-500 mr-2"></span>
						<span class="text-sm text-gray-600">Healthy: <span class="font-semibold">{healthSummary.healthy}</span></span>
					</div>
					<div class="flex items-center">
						<span class="h-3 w-3 rounded-full bg-yellow-500 mr-2"></span>
						<span class="text-sm text-gray-600">Degraded: <span class="font-semibold">{healthSummary.degraded}</span></span>
					</div>
					<div class="flex items-center">
						<span class="h-3 w-3 rounded-full bg-red-500 mr-2"></span>
						<span class="text-sm text-gray-600">Unhealthy: <span class="font-semibold">{healthSummary.unhealthy}</span></span>
					</div>
					<div class="ml-auto text-sm text-gray-500">
						Total: {healthSummary.total} clusters
					</div>
				</div>

				<!-- Progress bar -->
				<div class="w-full bg-gray-200 rounded-full h-3 overflow-hidden">
					{#if healthSummary.total > 0}
						<div class="h-full flex">
							<div
								class="bg-green-500 h-full"
								style="width: {(healthSummary.healthy / healthSummary.total) * 100}%"
							></div>
							<div
								class="bg-yellow-500 h-full"
								style="width: {(healthSummary.degraded / healthSummary.total) * 100}%"
							></div>
							<div
								class="bg-red-500 h-full"
								style="width: {(healthSummary.unhealthy / healthSummary.total) * 100}%"
							></div>
						</div>
					{:else}
						<div class="bg-gray-300 h-full w-full"></div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Cluster Details Grid -->
		{#if clusters && clusters.clusters.length > 0}
			<div>
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Cluster Details</h2>
				<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
					{#each clusters.clusters as cluster}
						<div class="bg-white rounded-lg border border-gray-200 p-4 hover:shadow-md transition-shadow">
							<!-- Header -->
							<div class="flex items-start justify-between mb-3">
								<div class="flex-1 min-w-0">
									<h3 class="text-sm font-semibold text-gray-900 truncate" title={cluster.clusterName}>
										{cluster.clusterName}
									</h3>
								</div>
								<span class="flex-shrink-0 ml-2 inline-flex items-center px-2 py-0.5 rounded text-xs font-medium {getHealthBadgeClasses(cluster.healthStatus)}">
									{cluster.healthStatus}
								</span>
							</div>

							<!-- Stats -->
							<div class="grid grid-cols-2 gap-3 text-sm">
								<div>
									<span class="text-gray-500">Hosts</span>
									<p class="font-medium">{cluster.healthyHosts}/{cluster.totalHosts}</p>
								</div>
								<div>
									<span class="text-gray-500">Connections</span>
									<p class="font-medium">{cluster.activeConnections}</p>
								</div>
								<div>
									<span class="text-gray-500">Active Requests</span>
									<p class="font-medium">{cluster.activeRequests}</p>
								</div>
								<div>
									<span class="text-gray-500">Pending</span>
									<p class="font-medium">{cluster.pendingRequests}</p>
								</div>
							</div>

							<!-- Indicators -->
							<div class="mt-3 pt-3 border-t border-gray-100 flex items-center gap-3">
								{#if cluster.circuitBreakerOpen}
									<span class="inline-flex items-center text-xs text-red-600">
										<svg class="h-4 w-4 mr-1" fill="none" viewBox="0 0 24 24" stroke="currentColor">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M18.364 18.364A9 9 0 005.636 5.636m12.728 12.728A9 9 0 015.636 5.636m12.728 12.728L5.636 5.636" />
										</svg>
										Circuit Open
									</span>
								{/if}
								{#if cluster.outlierEjections > 0}
									<span class="inline-flex items-center text-xs text-yellow-600">
										<svg class="h-4 w-4 mr-1" fill="none" viewBox="0 0 24 24" stroke="currentColor">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
										</svg>
										{cluster.outlierEjections} ejected
									</span>
								{/if}
								{#if cluster.successRate !== null}
									<span class="inline-flex items-center text-xs {cluster.successRate >= 0.99 ? 'text-green-600' : cluster.successRate >= 0.95 ? 'text-yellow-600' : 'text-red-600'}">
										Success: {(cluster.successRate * 100).toFixed(1)}%
									</span>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			</div>
		{/if}
	{:else}
		<!-- No data yet -->
		<div class="flex flex-col items-center justify-center py-24">
			<svg class="h-16 w-16 text-gray-400 mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
			</svg>
			<h2 class="text-xl font-semibold text-gray-900 mb-2">No Stats Data</h2>
			<p class="text-gray-600 mb-4">
				{currentTeam ? `No stats available for team "${currentTeam}" yet.` : 'Select a team to view stats.'}
			</p>
			{#if currentTeam}
				<button
					onclick={handleRefresh}
					class="inline-flex items-center px-4 py-2 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700"
				>
					<svg class="h-4 w-4 mr-2" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
					</svg>
					Load Stats
				</button>
			{/if}
		</div>
	{/if}
{/if}
