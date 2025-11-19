<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import Navigation from '$lib/components/Navigation.svelte';
	import type { SessionInfoResponse } from '$lib/api/types';

	let isLoading = $state(true);
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let selectedTeam = $state<string>('');
	let availableTeams = $state<string[]>([]);

	let { children } = $props();

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();

			// Load available teams
			const teamsResponse = await apiClient.listTeams();
			availableTeams = teamsResponse.teams;

			// Set selected team from session storage or first team
			const storedTeam = sessionStorage.getItem('selected_team');
			if (storedTeam && availableTeams.includes(storedTeam)) {
				selectedTeam = storedTeam;
			} else if (availableTeams.length > 0) {
				selectedTeam = availableTeams[0];
				sessionStorage.setItem('selected_team', selectedTeam);
			}

			isLoading = false;
		} catch (error) {
			// Not authenticated, redirect to login
			goto('/login');
		}
	});

	function handleTeamChange(team: string) {
		selectedTeam = team;
		sessionStorage.setItem('selected_team', selectedTeam);
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
			{selectedTeam}
			{availableTeams}
			onTeamChange={handleTeamChange}
		/>
		<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
			{@render children()}
		</main>
	</div>
{/if}
