<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { TeamResponse } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';

	let orgName = $derived($page.params.orgName ?? '');

	let teams = $state<TeamResponse[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let userIsOrgAdmin = $state(false);

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			userIsOrgAdmin = isOrgAdmin(sessionInfo.orgScopes);
			await loadTeams();
		} catch {
			goto('/login');
		}
	});

	async function loadTeams() {
		isLoading = true;
		error = null;
		try {
			const response = await apiClient.listOrgTeams(orgName);
			teams = response.teams;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load teams';
		} finally {
			isLoading = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/dashboard"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to dashboard"
					>
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Teams — {orgName}</h1>
				</div>
				{#if userIsOrgAdmin}
					<a
						href="/organizations/{orgName}/teams/create"
						class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
					>
						Create Team
					</a>
				{/if}
			</div>
		</div>
	</nav>

	<main class="w-full px-4 sm:px-6 lg:px-8 py-8">
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<div class="bg-white rounded-lg shadow-md overflow-hidden">
			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if teams.length === 0}
				<div class="text-center py-12">
					<p class="text-gray-500">No teams found</p>
					{#if userIsOrgAdmin}
						<a
							href="/organizations/{orgName}/teams/create"
							class="mt-4 inline-block px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
						>
							Create your first team
						</a>
					{/if}
				</div>
			{:else}
				<div class="overflow-x-auto">
					<table class="min-w-full divide-y divide-gray-200">
						<thead class="bg-gray-50">
							<tr>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Name
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Display Name
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Description
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Created
								</th>
								<th
									class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Actions
								</th>
							</tr>
						</thead>
						<tbody class="bg-white divide-y divide-gray-200">
							{#each teams as team (team.id)}
								<tr class="hover:bg-gray-50 cursor-pointer" onclick={() => goto(`/organizations/${orgName}/teams/${team.name}`)}>
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm font-medium text-gray-900 font-mono">{team.name}</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm text-gray-900">{team.displayName}</div>
									</td>
									<td class="px-6 py-4">
										<div class="text-sm text-gray-600 max-w-xs truncate">
											{team.description || '-'}
										</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
										{formatDate(team.createdAt)}
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
										<a
											href="/organizations/{orgName}/teams/{team.name}"
											class="text-blue-600 hover:text-blue-900"
											onclick={(e) => e.stopPropagation()}
										>
											View
										</a>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
	</main>
</div>
