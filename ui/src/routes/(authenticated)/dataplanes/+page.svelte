<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Server, Network, Download, Copy } from 'lucide-svelte';
	import type { DataplaneResponse, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let sessionInfo = $state<SessionInfoResponse | null>(null);

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let dataplanes = $state<DataplaneResponse[]>([]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		sessionInfo = await apiClient.getSessionInfo();
		if (sessionInfo.isPlatformAdmin) {
			try { await getAdminSummary(); } catch { /* handled by store */ }
			isLoading = false;
			return;
		}
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			dataplanes = await apiClient.listDataplanes(currentTeam);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load dataplanes';
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived({
		totalDataplanes: dataplanes.length,
		withGatewayHost: dataplanes.filter((dp) => dp.gatewayHost).length,
		withoutGatewayHost: dataplanes.filter((dp) => !dp.gatewayHost).length
	});

	// Filter dataplanes
	let filteredDataplanes = $derived(
		dataplanes
			.filter(
				(dp) =>
					!searchQuery ||
					dp.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
					(dp.gatewayHost && dp.gatewayHost.toLowerCase().includes(searchQuery.toLowerCase())) ||
					(dp.description && dp.description.toLowerCase().includes(searchQuery.toLowerCase()))
			)
	);

	// Navigate to create page
	function handleCreate() {
		goto('/dataplanes/create');
	}

	// Navigate to edit page
	function handleEdit(dataplane: DataplaneResponse) {
		goto(`/dataplanes/${encodeURIComponent(dataplane.name)}/edit`);
	}

	// Delete dataplane
	async function handleDelete(dataplane: DataplaneResponse) {
		if (
			!confirm(
				`Are you sure you want to delete the dataplane "${dataplane.name}"? This action cannot be undone.`
			)
		) {
			return;
		}

		try {
			await apiClient.deleteDataplane(dataplane.team, dataplane.name);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete dataplane';
		}
	}

	// Download envoy config
	async function handleDownloadEnvoyConfig(dataplane: DataplaneResponse) {
		try {
			const config = await apiClient.getDataplaneEnvoyConfig(dataplane.team, dataplane.name, { format: 'yaml' });
			const blob = new Blob([config], { type: 'application/yaml' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `envoy-config-${dataplane.name}.yaml`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to download envoy config';
		}
	}

	// Copy gateway host to clipboard
	async function handleCopyGatewayHost(gatewayHost: string) {
		try {
			await navigator.clipboard.writeText(gatewayHost);
		} catch (err) {
			console.error('Failed to copy to clipboard:', err);
		}
	}

	// Format date
	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

{#if sessionInfo?.isPlatformAdmin}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Dataplanes</h1>
		<p class="mt-2 text-sm text-gray-600">Platform-wide dataplane summary across all organizations and teams.</p>
	</div>
	{#if $adminSummaryLoading}
		<div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div></div>
	{:else if $adminSummaryError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4"><p class="text-sm text-red-800">{$adminSummaryError}</p></div>
	{:else if $adminSummary}
		<AdminResourceSummary summary={$adminSummary} highlightResource="dataplanes" />
	{/if}
</div>
{:else}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Dataplanes</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage Envoy instances for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Dataplane
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Dataplanes</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalDataplanes}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">With Gateway Host</p>
					<p class="text-2xl font-bold text-gray-900">{stats.withGatewayHost}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Network class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Without Gateway Host</p>
					<p class="text-2xl font-bold text-gray-900">{stats.withoutGatewayHost}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Server class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name, gateway host, or description..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading dataplanes...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredDataplanes.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Server class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No dataplanes found' : 'No dataplanes yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Dataplanes represent Envoy instances with a gateway host for MCP tool routing'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Dataplane
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-x-auto">
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
							Team
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Gateway Host
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
					{#each filteredDataplanes as dataplane}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<span class="text-sm font-medium text-gray-900">{dataplane.name}</span>
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{dataplane.team}</Badge>
							</td>

							<!-- Gateway Host -->
							<td class="px-6 py-4">
								{#if dataplane.gatewayHost}
									<div class="flex items-center gap-2">
										<span class="text-sm text-gray-900 font-mono">{dataplane.gatewayHost}</span>
										<button
											onclick={() => handleCopyGatewayHost(dataplane.gatewayHost!)}
											class="p-1 text-gray-400 hover:text-gray-600 transition-colors"
											title="Copy gateway host"
										>
											<Copy class="h-3 w-3" />
										</button>
									</div>
								{:else}
									<span class="text-sm text-gray-400">Not configured</span>
								{/if}
							</td>

							<!-- Description -->
							<td class="px-6 py-4">
								{#if dataplane.description}
									<span class="text-sm text-gray-600 truncate max-w-xs block">
										{dataplane.description}
									</span>
								{:else}
									<span class="text-sm text-gray-400">-</span>
								{/if}
							</td>

							<!-- Created -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-500">{formatDate(dataplane.createdAt)}</span>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleDownloadEnvoyConfig(dataplane)}
										class="p-2 text-green-600 hover:bg-green-50 rounded-md transition-colors"
										title="Download envoy config"
									>
										<Download class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleEdit(dataplane)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit dataplane"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(dataplane)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete dataplane"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination Placeholder -->
		{#if filteredDataplanes.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredDataplanes.length} dataplanes</p>
			</div>
		{/if}
	{/if}
</div>
{/if}
