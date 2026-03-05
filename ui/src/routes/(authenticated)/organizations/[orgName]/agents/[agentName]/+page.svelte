<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { AgentInfo, GrantResponse, CreateGrantRequest } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';
	import DeleteConfirmModal from '$lib/components/DeleteConfirmModal.svelte';

	let orgName = $derived($page.params.orgName ?? '');
	let agentName = $derived($page.params.agentName ?? '');

	let agent = $state<AgentInfo | null>(null);
	let grants = $state<GrantResponse[]>([]);
	let isLoading = $state(true);
	let loadError = $state<string | null>(null);

	// Grant creation
	let showCreateForm = $state(false);
	let isCreating = $state(false);
	let createError = $state<string | null>(null);

	// cp-tool grant form
	let grantResourceType = $state('');
	let grantAction = $state('');
	let grantTeam = $state('');

	// gateway/route grant form
	let grantRouteId = $state('');
	let grantMethods = $state<string[]>([]);

	// Available teams for the org
	let availableTeams = $state<string[]>([]);

	// Delete modal
	let showDeleteModal = $state(false);
	let grantToDelete = $state<GrantResponse | null>(null);
	let isDeleting = $state(false);

	// Agent delete
	let showDeleteAgentModal = $state(false);
	let isDeletingAgent = $state(false);

	const CP_RESOURCES = [
		'aggregated-schemas', 'api', 'audit', 'clusters', 'custom-wasm-filters',
		'dataplanes', 'filters', 'learning-sessions', 'listeners',
		'proxy-certificates', 'reports', 'routes', 'secrets'
	];

	const CP_ACTIONS = ['read', 'write', 'delete', 'create', 'execute'];
	const HTTP_METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'];

	let agentContext = $derived(agent?.agentContext ?? null);
	let isCpTool = $derived(agentContext === 'cp-tool');
	let isGatewayOrConsumer = $derived(agentContext === 'gateway-tool' || agentContext === 'api-consumer');

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isOrgAdmin(sessionInfo.scopes)) {
				goto(`/organizations/${orgName}/agents`);
				return;
			}
			// Load agent info
			const listResp = await apiClient.listOrgAgents(orgName);
			const found = listResp.agents.find((a: AgentInfo) => a.name === agentName);
			if (!found) {
				loadError = `Agent '${agentName}' not found`;
				isLoading = false;
				return;
			}
			agent = found;

			// Load teams
			const teamsResp = await apiClient.listOrgTeams(orgName);
			availableTeams = teamsResp.teams.map((t) => t.name);
			if (availableTeams.length > 0 && !grantTeam) {
				grantTeam = availableTeams[0];
			}

			// Load grants
			await loadGrants();
		} catch {
			goto('/login');
		} finally {
			isLoading = false;
		}
	});

	async function loadGrants() {
		try {
			const resp = await apiClient.listAgentGrants(orgName, agentName);
			grants = resp.grants;
		} catch (err: unknown) {
			loadError = err instanceof Error ? err.message : 'Failed to load grants';
		}
	}

	function resetCreateForm() {
		grantResourceType = '';
		grantAction = '';
		grantTeam = availableTeams.length > 0 ? availableTeams[0] : '';
		grantRouteId = '';
		grantMethods = [];
		createError = null;
	}

	function openCreateForm() {
		resetCreateForm();
		showCreateForm = true;
	}

	function toggleMethod(method: string) {
		if (grantMethods.includes(method)) {
			grantMethods = grantMethods.filter((m) => m !== method);
		} else {
			grantMethods = [...grantMethods, method];
		}
	}

	async function handleCreateGrant() {
		if (!grantTeam) {
			createError = 'Team is required';
			return;
		}

		isCreating = true;
		createError = null;

		try {
			const request: CreateGrantRequest = { team: grantTeam, grantType: '' };

			if (isCpTool) {
				if (!grantResourceType || !grantAction) {
					createError = 'Resource type and action are required';
					isCreating = false;
					return;
				}
				request.grantType = 'cp-tool';
				request.resourceType = grantResourceType;
				request.action = grantAction;
			} else if (isGatewayOrConsumer) {
				if (!grantRouteId) {
					createError = 'Route ID is required';
					isCreating = false;
					return;
				}
				// api-consumer sends "route", gateway-tool sends "gateway-tool"
				request.grantType = agentContext === 'api-consumer' ? 'route' : 'gateway-tool';
				request.routeId = grantRouteId;
				if (grantMethods.length > 0) {
					request.allowedMethods = grantMethods;
				}
			} else {
				createError = 'Agent has no context set — cannot create grants';
				isCreating = false;
				return;
			}

			await apiClient.createAgentGrant(orgName, agentName, request);
			showCreateForm = false;
			await loadGrants();
		} catch (err: unknown) {
			if (err instanceof Error) {
				if (err.message.includes('409') || err.message.includes('already exists')) {
					createError = 'This grant already exists';
				} else if (err.message.includes('403')) {
					createError = 'Org admin privileges required';
				} else {
					createError = err.message;
				}
			} else {
				createError = 'Failed to create grant';
			}
		} finally {
			isCreating = false;
		}
	}

	function openDeleteGrant(grant: GrantResponse) {
		grantToDelete = grant;
		showDeleteModal = true;
	}

	async function handleDeleteGrant() {
		if (!grantToDelete) return;
		isDeleting = true;
		try {
			await apiClient.deleteAgentGrant(orgName, agentName, grantToDelete.id);
			showDeleteModal = false;
			grantToDelete = null;
			await loadGrants();
		} catch {
			// Error handled by modal
		} finally {
			isDeleting = false;
		}
	}

	async function handleDeleteAgent() {
		isDeletingAgent = true;
		try {
			await apiClient.deleteOrgAgent(orgName, agentName);
			goto(`/organizations/${orgName}/agents`);
		} catch {
			isDeletingAgent = false;
		}
	}

	function formatGrantResource(grant: GrantResponse): string {
		if (grant.grantType === 'cp-tool') {
			return `${grant.resourceType ?? ''}:${grant.action ?? ''}`;
		}
		return grant.routeId ?? '-';
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	function contextBadgeClass(ctx: string | null | undefined): string {
		switch (ctx) {
			case 'cp-tool':
				return 'bg-purple-100 text-purple-800';
			case 'gateway-tool':
				return 'bg-green-100 text-green-800';
			case 'api-consumer':
				return 'bg-amber-100 text-amber-800';
			default:
				return 'bg-gray-100 text-gray-700';
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
					<h1 class="text-xl font-bold text-gray-900">Agent: {agentName}</h1>
				</div>
				<button
					onclick={() => (showDeleteAgentModal = true)}
					class="px-4 py-2 text-sm font-medium text-red-600 border border-red-300 rounded-md hover:bg-red-50"
				>
					Delete Agent
				</button>
			</div>
		</div>
	</nav>

	<main class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		{#if isLoading}
			<div class="flex justify-center items-center py-12">
				<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
			</div>
		{:else if loadError}
			<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{loadError}</p>
			</div>
		{:else if agent}
			<!-- Agent Info -->
			<div class="bg-white rounded-lg shadow-md p-6 mb-6">
				<h2 class="text-sm font-medium text-gray-500 uppercase tracking-wider mb-3">
					Agent Details
				</h2>
				<dl class="grid grid-cols-2 sm:grid-cols-4 gap-4">
					<div>
						<dt class="text-xs text-gray-500">Name</dt>
						<dd class="text-sm font-mono text-gray-900 mt-0.5">{agent.name}</dd>
					</div>
					<div>
						<dt class="text-xs text-gray-500">Username</dt>
						<dd class="text-sm font-mono text-gray-900 mt-0.5">{agent.username}</dd>
					</div>
					<div>
						<dt class="text-xs text-gray-500">Context</dt>
						<dd class="mt-0.5">
							<span
								class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono {contextBadgeClass(agent.agentContext)}"
							>
								{agent.agentContext ?? 'none'}
							</span>
						</dd>
					</div>
					<div>
						<dt class="text-xs text-gray-500">Created</dt>
						<dd class="text-sm text-gray-900 mt-0.5">{formatDate(agent.createdAt)}</dd>
					</div>
					<div class="col-span-2 sm:col-span-4">
						<dt class="text-xs text-gray-500">Teams</dt>
						<dd class="flex flex-wrap gap-1 mt-1">
							{#each agent.teams as team}
								<span
									class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono bg-blue-100 text-blue-800"
								>
									{team}
								</span>
							{/each}
						</dd>
					</div>
				</dl>
			</div>

			<!-- Grants Section -->
			<div class="bg-white rounded-lg shadow-md overflow-hidden">
				<div class="px-6 py-4 border-b border-gray-200 flex justify-between items-center">
					<h2 class="text-lg font-semibold text-gray-900">Grants</h2>
					{#if agentContext}
						<button
							onclick={openCreateForm}
							class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
						>
							Add Grant
						</button>
					{/if}
				</div>

				{#if grants.length === 0}
					<div class="text-center py-12">
						<p class="text-gray-500">No grants. This agent has no permissions.</p>
					</div>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full divide-y divide-gray-200">
							<thead class="bg-gray-50">
								<tr>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Type
									</th>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Resource
									</th>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Methods
									</th>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Team
									</th>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Created
									</th>
									<th
										class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Expires
									</th>
									<th
										class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
									>
										Actions
									</th>
								</tr>
							</thead>
							<tbody class="bg-white divide-y divide-gray-200">
								{#each grants as grant (grant.id)}
									<tr class="hover:bg-gray-50">
										<td class="px-6 py-4 whitespace-nowrap">
											<span
												class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono {grant.grantType === 'cp-tool'
													? 'bg-purple-100 text-purple-800'
													: 'bg-green-100 text-green-800'}"
											>
												{grant.grantType}
											</span>
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm font-mono text-gray-900">
											{formatGrantResource(grant)}
										</td>
										<td class="px-6 py-4">
											{#if grant.allowedMethods && grant.allowedMethods.length > 0}
												<div class="flex flex-wrap gap-1">
													{#each grant.allowedMethods as method}
														<span
															class="inline-flex items-center px-1.5 py-0.5 rounded text-xs bg-gray-100 text-gray-700"
														>
															{method}
														</span>
													{/each}
												</div>
											{:else}
												<span class="text-xs text-gray-400">-</span>
											{/if}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{grant.team ?? '-'}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{formatDate(grant.createdAt)}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{grant.expiresAt ? formatDate(grant.expiresAt) : 'Never'}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-right">
											<button
												onclick={() => openDeleteGrant(grant)}
												class="text-red-600 hover:text-red-900 text-sm"
											>
												Delete
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</div>

			<!-- Create Grant Form -->
			{#if showCreateForm}
				<div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50" role="dialog" aria-modal="true">
					<div class="bg-white rounded-lg shadow-xl p-6 max-w-lg w-full mx-4">
						<h3 class="text-lg font-semibold text-gray-900 mb-4">Add Grant</h3>

						{#if createError}
							<div class="mb-4 bg-red-50 border-l-4 border-red-500 p-3">
								<p class="text-red-800 text-sm">{createError}</p>
							</div>
						{/if}

						<div class="space-y-4">
							<!-- Team selector -->
							<div>
								<label for="grant-team" class="block text-sm font-medium text-gray-700 mb-1">
									Team <span class="text-red-500">*</span>
								</label>
								<select
									id="grant-team"
									bind:value={grantTeam}
									class="w-full px-3 py-2 border border-gray-300 rounded-md"
								>
									{#each availableTeams as team}
										<option value={team}>{team}</option>
									{/each}
								</select>
							</div>

							{#if isCpTool}
								<!-- cp-tool: resource type + action -->
								<div>
									<label for="grant-resource" class="block text-sm font-medium text-gray-700 mb-1">
										Resource Type <span class="text-red-500">*</span>
									</label>
									<select
										id="grant-resource"
										bind:value={grantResourceType}
										class="w-full px-3 py-2 border border-gray-300 rounded-md"
									>
										<option value="">Select resource...</option>
										{#each CP_RESOURCES as res}
											<option value={res}>{res}</option>
										{/each}
									</select>
								</div>
								<div>
									<label for="grant-action" class="block text-sm font-medium text-gray-700 mb-1">
										Action <span class="text-red-500">*</span>
									</label>
									<select
										id="grant-action"
										bind:value={grantAction}
										class="w-full px-3 py-2 border border-gray-300 rounded-md"
									>
										<option value="">Select action...</option>
										{#each CP_ACTIONS as act}
											<option value={act}>{act}</option>
										{/each}
									</select>
								</div>
							{:else if isGatewayOrConsumer}
								<!-- gateway/route: route ID + methods -->
								<div>
									<label for="grant-route" class="block text-sm font-medium text-gray-700 mb-1">
										Route ID <span class="text-red-500">*</span>
									</label>
									<input
										id="grant-route"
										type="text"
										bind:value={grantRouteId}
										placeholder="Route ID (UUID)"
										class="w-full px-3 py-2 border border-gray-300 rounded-md font-mono text-sm"
									/>
									<p class="mt-1 text-xs text-gray-500">
										Enter the route ID. Route must have exposure: external.
									</p>
								</div>
								<div>
									<p class="block text-sm font-medium text-gray-700 mb-1">
										Allowed Methods
									</p>
									<div class="flex flex-wrap gap-2">
										{#each HTTP_METHODS as method}
											<label class="flex items-center gap-1.5 text-sm cursor-pointer">
												<input
													type="checkbox"
													checked={grantMethods.includes(method)}
													onchange={() => toggleMethod(method)}
													class="rounded border-gray-300 text-blue-600"
												/>
												<span class="font-mono text-gray-700">{method}</span>
											</label>
										{/each}
									</div>
									<p class="mt-1 text-xs text-gray-500">
										Leave empty to allow all methods.
									</p>
								</div>
							{:else}
								<p class="text-sm text-gray-500">
									This agent has no context set. Set agent_context to create grants.
								</p>
							{/if}
						</div>

						<div class="mt-6 flex justify-end gap-3">
							<button
								onclick={() => (showCreateForm = false)}
								class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
							>
								Cancel
							</button>
							<button
								onclick={handleCreateGrant}
								disabled={isCreating}
								class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
							>
								{isCreating ? 'Creating...' : 'Create Grant'}
							</button>
						</div>
					</div>
				</div>
			{/if}
		{/if}
	</main>
</div>

<!-- Delete grant confirmation -->
<DeleteConfirmModal
	show={showDeleteModal}
	resourceType="Grant"
	resourceName={grantToDelete ? formatGrantResource(grantToDelete) : ''}
	onConfirm={handleDeleteGrant}
	onCancel={() => {
		showDeleteModal = false;
		grantToDelete = null;
	}}
	loading={isDeleting}
	warningMessage="Remove this grant? The agent will immediately lose this permission."
/>

<!-- Delete agent confirmation -->
<DeleteConfirmModal
	show={showDeleteAgentModal}
	resourceType="Agent"
	resourceName={agentName}
	onConfirm={handleDeleteAgent}
	onCancel={() => (showDeleteAgentModal = false)}
	loading={isDeletingAgent}
	warningMessage="Deleting this agent will immediately revoke its access. This cannot be undone."
/>
