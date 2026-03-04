<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { AgentInfo } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';
	import DeleteConfirmModal from '$lib/components/DeleteConfirmModal.svelte';

	let orgName = $derived($page.params.orgName ?? '');

	let agents = $state<AgentInfo[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let userIsOrgAdmin = $state(false);

	// Delete modal state
	let showDeleteModal = $state(false);
	let agentToDelete = $state<AgentInfo | null>(null);
	let isDeleting = $state(false);
	let deleteError = $state<string | null>(null);

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			userIsOrgAdmin = isOrgAdmin(sessionInfo.scopes);
			if (!userIsOrgAdmin) {
				goto(`/organizations/${orgName}/teams`);
				return;
			}
			await loadAgents();
		} catch {
			goto('/login');
		}
	});

	async function loadAgents() {
		isLoading = true;
		error = null;
		try {
			const response = await apiClient.listOrgAgents(orgName);
			agents = response.agents;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load agents';
		} finally {
			isLoading = false;
		}
	}

	function openDeleteModal(agent: AgentInfo) {
		agentToDelete = agent;
		showDeleteModal = true;
		deleteError = null;
	}

	function closeDeleteModal() {
		showDeleteModal = false;
		agentToDelete = null;
		deleteError = null;
	}

	async function handleDelete() {
		if (!agentToDelete) return;
		isDeleting = true;
		deleteError = null;
		try {
			await apiClient.deleteOrgAgent(orgName, agentToDelete.name);
			agents = agents.filter((a) => a.agentId !== agentToDelete!.agentId);
			closeDeleteModal();
		} catch (err: unknown) {
			deleteError = err instanceof Error ? err.message : 'Failed to delete agent';
		} finally {
			isDeleting = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	function parseBaseScopes(scopes: string[]): string[] {
		const base = new Set<string>();
		for (const scope of scopes) {
			// Fully-qualified: "team:engineering:clusters:read" → "clusters:read"
			const parts = scope.split(':');
			if (parts.length === 4 && parts[0] === 'team') {
				base.add(`${parts[2]}:${parts[3]}`);
			} else if (parts.length === 2) {
				base.add(scope);
			}
		}
		return Array.from(base).sort();
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
					<h1 class="text-xl font-bold text-gray-900">Agents — {orgName}</h1>
				</div>
				{#if userIsOrgAdmin}
					<a
						href="/organizations/{orgName}/agents/create"
						class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
					>
						Create Agent
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
			{:else if agents.length === 0}
				<div class="text-center py-12">
					<p class="text-gray-500">No agents found</p>
					{#if userIsOrgAdmin}
						<a
							href="/organizations/{orgName}/agents/create"
							class="mt-4 inline-block px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
						>
							Create your first agent
						</a>
					{/if}
				</div>
			{:else}
				<div class="overflow-x-auto">
					<table class="min-w-full divide-y divide-gray-200">
						<thead class="bg-gray-50">
							<tr>
								<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
									Name
								</th>
								<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
									Username
								</th>
								<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
									Teams
								</th>
								<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
									Scopes
								</th>
								<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
									Created
								</th>
								<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
									Actions
								</th>
							</tr>
						</thead>
						<tbody class="bg-white divide-y divide-gray-200">
							{#each agents as agent (agent.agentId)}
								<tr class="hover:bg-gray-50">
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm font-medium text-gray-900 font-mono">{agent.name}</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm text-gray-600 font-mono">{agent.username}</div>
									</td>
									<td class="px-6 py-4">
										<div class="flex flex-wrap gap-1">
											{#each agent.teams as team}
												<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono bg-blue-100 text-blue-800">
													{team}
												</span>
											{/each}
										</div>
									</td>
									<td class="px-6 py-4">
										<div class="flex flex-wrap gap-1">
											{#each parseBaseScopes(agent.scopes) as scope}
												<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono bg-gray-100 text-gray-700">
													{scope}
												</span>
											{/each}
											{#if agent.scopes.length === 0}
												<span class="text-xs text-gray-400">No scopes</span>
											{/if}
										</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
										{formatDate(agent.createdAt)}
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
										<div class="flex justify-end gap-3">
											<a
												href="/organizations/{orgName}/agents/{agent.name}"
												class="text-blue-600 hover:text-blue-900"
											>
												Edit Scopes
											</a>
											<button
												onclick={() => openDeleteModal(agent)}
												class="text-red-600 hover:text-red-900"
											>
												Delete
											</button>
										</div>
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

<DeleteConfirmModal
	show={showDeleteModal}
	resourceType="Agent"
	resourceName={agentToDelete?.name ?? ''}
	onConfirm={handleDelete}
	onCancel={closeDeleteModal}
	loading={isDeleting}
	warningMessage="Deleting this agent will immediately revoke its access. This cannot be undone."
/>
