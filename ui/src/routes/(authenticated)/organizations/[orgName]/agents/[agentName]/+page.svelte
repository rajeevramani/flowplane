<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { AgentInfo } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';

	let orgName = $derived($page.params.orgName ?? '');
	let agentName = $derived($page.params.agentName ?? '');

	let agent = $state<AgentInfo | null>(null);
	let isLoading = $state(true);
	let loadError = $state<string | null>(null);

	let selectedScopes = $state<string[]>([]);
	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);
	let successMessage = $state<string | null>(null);

	const ALL_SCOPES = [
		'clusters:read',
		'clusters:write',
		'routes:read',
		'routes:write',
		'listeners:read',
		'listeners:write',
		'filters:read',
		'filters:write',
		'learning:read',
		'learning:write',
		'secrets:read',
		'secrets:write',
		'stats:read',
		'stats:write'
	];

	function parseBaseScopes(scopes: string[]): string[] {
		const base = new Set<string>();
		for (const scope of scopes) {
			const parts = scope.split(':');
			if (parts.length === 4 && parts[0] === 'team') {
				base.add(`${parts[2]}:${parts[3]}`);
			} else if (parts.length === 2) {
				base.add(scope);
			}
		}
		return Array.from(base);
	}

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isOrgAdmin(sessionInfo.scopes)) {
				goto(`/organizations/${orgName}/agents`);
				return;
			}
			// Load agent info from list
			const listResp = await apiClient.listOrgAgents(orgName);
			const found = listResp.agents.find((a) => a.name === agentName);
			if (!found) {
				loadError = `Agent '${agentName}' not found`;
				isLoading = false;
				return;
			}
			agent = found;
			selectedScopes = [];
		} catch {
			goto('/login');
		} finally {
			isLoading = false;
		}
	});

	function toggleScope(scope: string) {
		if (selectedScopes.includes(scope)) {
			selectedScopes = selectedScopes.filter((s) => s !== scope);
		} else {
			selectedScopes = [...selectedScopes, scope];
		}
	}

	async function handleSubmit() {
		isSubmitting = true;
		submitError = null;
		successMessage = null;

		try {
			// Scopes management removed — grants UI coming in F.2
			submitError = 'Scopes management has been replaced by grants. This page will be updated.';
			setTimeout(() => {
				goto(`/organizations/${orgName}/agents`);
			}, 1000);
		} catch (err: unknown) {
			submitError = err instanceof Error ? err.message : 'Failed to update scopes';
		} finally {
			isSubmitting = false;
		}
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/organizations/{orgName}/agents"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to agents"
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
					<h1 class="text-xl font-bold text-gray-900">Edit Agent: {agentName}</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		{#if isLoading}
			<div class="flex justify-center items-center py-12">
				<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
			</div>
		{:else if loadError}
			<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{loadError}</p>
			</div>
		{:else if agent}
			{#if submitError}
				<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
					<p class="text-red-800 text-sm">{submitError}</p>
				</div>
			{/if}

			{#if successMessage}
				<div class="mb-6 bg-green-50 border-l-4 border-green-500 rounded-md p-4">
					<p class="text-green-800 text-sm">{successMessage}</p>
				</div>
			{/if}

			<div class="bg-white rounded-lg shadow-md p-6">
				<!-- Agent info (read-only) -->
				<div class="mb-6 pb-6 border-b border-gray-200">
					<h2 class="text-sm font-medium text-gray-500 uppercase tracking-wider mb-3">Agent Details</h2>
					<dl class="grid grid-cols-2 gap-4">
						<div>
							<dt class="text-xs text-gray-500">Name</dt>
							<dd class="text-sm font-mono text-gray-900 mt-0.5">{agent.name}</dd>
						</div>
						<div>
							<dt class="text-xs text-gray-500">Username</dt>
							<dd class="text-sm font-mono text-gray-900 mt-0.5">{agent.username}</dd>
						</div>
						<div class="col-span-2">
							<dt class="text-xs text-gray-500">Teams</dt>
							<dd class="flex flex-wrap gap-1 mt-1">
								{#each agent.teams as team}
									<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono bg-blue-100 text-blue-800">
										{team}
									</span>
								{/each}
							</dd>
						</div>
					</dl>
					<p class="mt-3 text-xs text-gray-400">
						Teams are immutable after creation. To change teams, delete and recreate the agent.
					</p>
				</div>

				<!-- Scope editor -->
				<form
					onsubmit={(e) => {
						e.preventDefault();
						handleSubmit();
					}}
				>
					<div class="mb-4">
						<p class="text-sm font-medium text-gray-700 mb-3">Scopes</p>
						<div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
							{#each ALL_SCOPES as scope}
								<label class="flex items-center gap-2 text-sm cursor-pointer">
									<input
										type="checkbox"
										checked={selectedScopes.includes(scope)}
										onchange={() => toggleScope(scope)}
										class="rounded border-gray-300 text-blue-600"
									/>
									<span class="font-mono text-gray-700">{scope}</span>
								</label>
							{/each}
						</div>
						<p class="mt-2 text-xs text-gray-500">
							Scope changes do not rotate credentials — the agent keeps its existing client ID and secret.
						</p>
					</div>

					<div class="flex justify-end gap-3">
						<a
							href="/organizations/{orgName}/agents"
							class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
						>
							Cancel
						</a>
						<button
							type="submit"
							disabled={isSubmitting}
							class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
						>
							{isSubmitting ? 'Saving…' : 'Save Scopes'}
						</button>
					</div>
				</form>
			</div>
		{/if}
	</main>
</div>
