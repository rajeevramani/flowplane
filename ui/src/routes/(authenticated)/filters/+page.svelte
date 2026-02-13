<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Filter as FilterIcon, Settings, Clock, Server, Sliders } from 'lucide-svelte';
	import type { FilterResponse, FilterStatusResponse, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let sessionInfo = $state<SessionInfoResponse | null>(null);

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let filters = $state<FilterResponse[]>([]);
	let filterStatuses = $state<Map<string, FilterStatusResponse>>(new Map());

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
			const filtersData = await apiClient.listFilters();
			filters = filtersData;

			// Load status for each filter (installations + configurations)
			const statusMap = new Map<string, FilterStatusResponse>();
			await Promise.all(
				filtersData.map(async (filter) => {
					try {
						const status = await apiClient.getFilterStatus(filter.id);
						statusMap.set(filter.id, status);
					} catch (e) {
						// Ignore errors for individual status fetches
						console.warn(`Failed to load status for filter ${filter.id}:`, e);
					}
				})
			);
			filterStatuses = statusMap;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load filters';
			console.error('Failed to load filters:', e);
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived(() => {
		const teamFilters = filters.filter(f => f.team === currentTeam);
		let installedCount = 0;
		let configuredCount = 0;

		teamFilters.forEach(f => {
			const status = filterStatuses.get(f.id);
			if (status) {
				if (status.installations.length > 0) installedCount++;
				if (status.configurations.length > 0) configuredCount++;
			}
		});

		return {
			totalFilters: teamFilters.length,
			installedFilters: installedCount,
			configuredFilters: configuredCount
		};
	});

	// Filter filters by team and search
	let filteredFilters = $derived(
		filters
			.filter(filter => filter.team === currentTeam)
			.filter(filter =>
				!searchQuery ||
				filter.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				(filter.description && filter.description.toLowerCase().includes(searchQuery.toLowerCase()))
			)
	);

	// Format filter type for display (convert snake_case to Title Case)
	function formatFilterType(type: string): string {
		return type.split('_').map(word => word.charAt(0).toUpperCase() + word.slice(1)).join(' ');
	}

	// Navigate to create page
	function handleCreate() {
		goto('/filters/create');
	}

	// Navigate to edit page
	function handleEdit(filterId: string) {
		goto(`/filters/${encodeURIComponent(filterId)}/edit`);
	}

	// Delete filter
	async function handleDelete(filter: FilterResponse) {
		if (!confirm(`Are you sure you want to delete the filter "${filter.name}"? This action cannot be undone.`)) {
			return;
		}

		// Clear any previous action error
		actionError = null;

		try {
			await apiClient.deleteFilter(filter.id);
			await loadData();
		} catch (err) {
			actionError = err instanceof Error ? err.message : 'Failed to delete filter';
		}
	}

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

{#if sessionInfo?.isPlatformAdmin}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">HTTP Filters</h1>
		<p class="mt-2 text-sm text-gray-600">Platform-wide filter summary across all organizations and teams.</p>
	</div>
	{#if $adminSummaryLoading}
		<div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div></div>
	{:else if $adminSummaryError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4"><p class="text-sm text-red-800">{$adminSummaryError}</p></div>
	{:else if $adminSummary}
		<AdminResourceSummary summary={$adminSummary} highlightResource="filters" />
	{/if}
</div>
{:else}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">HTTP Filters</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage reusable HTTP filters for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Filter
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Filters</p>
					<p class="text-2xl font-bold text-gray-900">{stats().totalFilters}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<FilterIcon class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Installed on Listeners</p>
					<p class="text-2xl font-bold text-gray-900">{stats().installedFilters}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Server class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Configured for Routes</p>
					<p class="text-2xl font-bold text-gray-900">{stats().configuredFilters}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Sliders class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name or description..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Action Error (e.g., delete failed) -->
	{#if actionError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800">{actionError}</p>
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading filters...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredFilters.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<FilterIcon class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No filters found' : 'No filters yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Get started by creating a new reusable filter'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Filter
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Name
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Type
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Installed On
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Configured For
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredFilters as filter}
						{@const status = filterStatuses.get(filter.id)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{filter.name}</span>
									<span class="text-xs text-gray-500">{filter.description || '-'}</span>
								</div>
							</td>

							<!-- Type -->
							<td class="px-6 py-4">
								<Badge variant="blue">{formatFilterType(filter.filterType)}</Badge>
							</td>

							<!-- Installed On (Listeners) -->
							<td class="px-6 py-4">
								{#if status && status.installations.length > 0}
									<div class="flex flex-col gap-1">
										<span class="inline-flex items-center px-2 py-0.5 text-xs rounded bg-green-100 text-green-700">
											<Server class="h-3 w-3 mr-1" />
											{status.installations.length} {status.installations.length === 1 ? 'listener' : 'listeners'}
										</span>
									</div>
								{:else}
									<span class="px-2 py-0.5 text-xs rounded bg-gray-100 text-gray-500">
										Not installed
									</span>
								{/if}
							</td>

							<!-- Configured For (Routes) -->
							<td class="px-6 py-4">
								{#if status && status.configurations.length > 0}
									{@const routeConfigs = status.configurations.filter(c => c.scopeType === 'route-config').length}
									{@const vhosts = status.configurations.filter(c => c.scopeType === 'virtual-host').length}
									{@const routes = status.configurations.filter(c => c.scopeType === 'route').length}
									<div class="flex flex-col gap-1">
										{#if routeConfigs > 0}
											<span class="inline-flex items-center px-2 py-0.5 text-xs rounded bg-blue-100 text-blue-700">
												{routeConfigs} route-config{routeConfigs !== 1 ? 's' : ''}
											</span>
										{/if}
										{#if vhosts > 0}
											<span class="inline-flex items-center px-2 py-0.5 text-xs rounded bg-purple-100 text-purple-700">
												{vhosts} vhost{vhosts !== 1 ? 's' : ''}
											</span>
										{/if}
										{#if routes > 0}
											<span class="inline-flex items-center px-2 py-0.5 text-xs rounded bg-orange-100 text-orange-700">
												{routes} route{routes !== 1 ? 's' : ''}
											</span>
										{/if}
									</div>
								{:else}
									<span class="px-2 py-0.5 text-xs rounded bg-gray-100 text-gray-500">
										Not configured
									</span>
								{/if}
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{filter.team}</Badge>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-1">
									<button
										onclick={() => goto(`/filters/${encodeURIComponent(filter.id)}/install`)}
										class="p-2 text-green-600 hover:bg-green-50 rounded-md transition-colors"
										title="Install filter on listeners"
									>
										<Server class="h-4 w-4" />
									</button>
									<button
										onclick={() => goto(`/filters/${encodeURIComponent(filter.id)}/configure`)}
										class="p-2 text-orange-600 hover:bg-orange-50 rounded-md transition-colors"
										title="Configure filter for routes"
									>
										<Sliders class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleEdit(filter.id)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit filter"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(filter)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete filter"
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
		{#if filteredFilters.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredFilters.length} filters</p>
			</div>
		{/if}
	{/if}
</div>
{/if}
