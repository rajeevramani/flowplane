<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import AppShell from '$lib/components/AppShell.svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam, initializeSelectedTeam, setSelectedTeam } from '$lib/stores/team';

	interface ResourceCounts {
		routeConfigs: number;
		clusters: number;
		listeners: number;
		filters: number;
		imports: number;
	}

	let isLoading = $state(true);
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let currentTeam = $state<string>('');
	let availableTeams = $state<string[]>([]);
	let resourceCounts = $state<ResourceCounts>({ routeConfigs: 0, clusters: 0, listeners: 0, filters: 0, imports: 0 });

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

			// Load resource counts
			await loadResourceCounts();

			isLoading = false;
		} catch (error) {
			// Not authenticated, redirect to login
			goto('/login');
		}
	});

	async function loadResourceCounts() {
		try {
			const [routes, clusters, listeners, filters, imports] = await Promise.all([
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners(),
				apiClient.listFilters(),
				sessionInfo?.isAdmin
					? apiClient.listAllImports()
					: currentTeam
						? apiClient.listImports(currentTeam)
						: Promise.resolve([])
			]);

			resourceCounts = {
				routeConfigs: routes.length,
				clusters: clusters.length,
				listeners: listeners.length,
				filters: filters.length,
				imports: imports.length
			};
		} catch (error) {
			console.error('Failed to load resource counts:', error);
		}
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
	>
		{@render children()}
	</AppShell>
{/if}
