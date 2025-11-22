<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import Navigation from '$lib/components/Navigation.svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam, initializeSelectedTeam, setSelectedTeam } from '$lib/stores/team';

	let isLoading = $state(true);
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let currentTeam = $state<string>('');
	let availableTeams = $state<string[]>([]);

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

			isLoading = false;
		} catch (error) {
			// Not authenticated, redirect to login
			goto('/login');
		}
	});

	function handleTeamChange(team: string) {
		setSelectedTeam(team);
	}
</script>

{#if isLoading}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center">
		<div class="text-gray-600">Loading...</div>
	</div>
{:else if sessionInfo}
	<div class="min-h-screen bg-gray-50">
		<Navigation
			{sessionInfo}
			selectedTeam={currentTeam}
			{availableTeams}
			onTeamChange={handleTeamChange}
		/>
		<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
			{@render children()}
		</main>
	</div>
{/if}
