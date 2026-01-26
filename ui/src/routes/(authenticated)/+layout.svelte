<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam, initializeSelectedTeam, setSelectedTeam } from '$lib/stores/team';
	import { checkStatsEnabled } from '$lib/stores/stats';

	interface ResourceCounts {
		routeConfigs: number;
		clusters: number;
		listeners: number;
		filters: number;
		imports: number;
		secrets: number;
		dataplanes: number;
	}

	let isLoading = $state(true);
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let currentTeam = $state<string>('');
	let availableTeams = $state<string[]>([]);
	let resourceCounts = $state<ResourceCounts>({ routeConfigs: 0, clusters: 0, listeners: 0, filters: 0, imports: 0, secrets: 0, dataplanes: 0 });
	let statsEnabled = $state(false);

	let { children } = $props();

	// Subscribe to store changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();

			// Load available teams
			const teamsResponse = await apiClient.listTeams();
			availableTeams = teamsResponse.teams;

			// Initialize selected team from store/sessionStorage or first team
			initializeSelectedTeam(availableTeams);

			// Load resource counts and stats enabled status in parallel
			const [_, isEnabled] = await Promise.all([
				loadResourceCounts(),
				checkStatsEnabled()
			]);
			statsEnabled = isEnabled;

			isLoading = false;
		} catch (error) {
			// Not authenticated, redirect to login
			goto('/login');
		}
	});

	async function loadResourceCounts() {
		// Helper to safely call an API and return empty array on failure
		async function safeCall<T>(call: () => Promise<T[]>): Promise<T[]> {
			try {
				return await call();
			} catch {
				return [];
			}
		}

		const [routes, clusters, listeners, filters, imports, secrets, dataplanes] = await Promise.all([
			safeCall(() => apiClient.listRouteConfigs()),
			safeCall(() => apiClient.listClusters()),
			safeCall(() => apiClient.listListeners()),
			safeCall(() => apiClient.listFilters()),
			safeCall(() =>
				sessionInfo?.isAdmin
					? apiClient.listAllImports()
					: currentTeam
						? apiClient.listImports(currentTeam)
						: Promise.resolve([])
			),
			safeCall(() =>
				currentTeam
					? apiClient.listSecrets(currentTeam)
					: Promise.resolve([])
			),
			safeCall(() =>
				currentTeam
					? apiClient.listDataplanes(currentTeam)
					: Promise.resolve([])
			)
		]);

		resourceCounts = {
			routeConfigs: routes.length,
			clusters: clusters.length,
			listeners: listeners.length,
			filters: filters.length,
			imports: imports.length,
			secrets: secrets.length,
			dataplanes: dataplanes.length
		};
	}

	function handleTeamChange(team: string) {
		setSelectedTeam(team);
		// Reload counts when team changes
		loadResourceCounts();
	}
</script>

{#if isLoading}
	<div class="min-h-screen bg-gray-100 flex items-center justify-center">
		<div class="flex flex-col items-center gap-3">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			<span class="text-sm text-gray-600">Loading...</span>
		</div>
	</div>
{:else if sessionInfo}
	<AppShell
		{sessionInfo}
		selectedTeam={currentTeam}
		{availableTeams}
		onTeamChange={handleTeamChange}
		{resourceCounts}
		{statsEnabled}
	>
		{@render children()}
	</AppShell>
{/if}
