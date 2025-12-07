<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Filter as FilterIcon, Settings, Clock, Link } from 'lucide-svelte';
	import type { FilterResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let filters = $state<FilterResponse[]>([]);

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
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const filtersData = await apiClient.listFilters();
			filters = filtersData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load filters';
			console.error('Failed to load filters:', e);
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived({
		totalFilters: filters.filter(f => f.team === currentTeam).length,
		headerMutationFilters: filters.filter(f => f.team === currentTeam && f.filterType === 'header_mutation').length,
		rateLimitFilters: filters.filter(f => f.team === currentTeam && f.filterType === 'local_rate_limit').length
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

<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
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
					<p class="text-2xl font-bold text-gray-900">{stats.totalFilters}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<FilterIcon class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Header Mutation</p>
					<p class="text-2xl font-bold text-gray-900">{stats.headerMutationFilters}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Settings class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Rate Limit</p>
					<p class="text-2xl font-bold text-gray-900">{stats.rateLimitFilters}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Clock class="h-6 w-6 text-orange-600" />
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
							Description
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Attachments
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
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
					{#each filteredFilters as filter}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{filter.name}</span>
									<span class="text-xs text-gray-500 font-mono">{filter.id}</span>
								</div>
							</td>

							<!-- Type -->
							<td class="px-6 py-4">
								<Badge variant="blue">{formatFilterType(filter.filterType)}</Badge>
							</td>

							<!-- Description -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">
									{filter.description || '-'}
								</span>
							</td>

							<!-- Attachments -->
							<td class="px-6 py-4">
								{#if filter.attachmentCount && filter.attachmentCount > 0}
									<span class="px-2 py-0.5 text-xs rounded bg-indigo-100 text-indigo-700">
										{filter.attachmentCount} {filter.attachmentCount === 1 ? 'resource' : 'resources'}
									</span>
								{:else}
									<span class="px-2 py-0.5 text-xs rounded bg-gray-100 text-gray-600">
										Not attached
									</span>
								{/if}
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{filter.team}</Badge>
							</td>

							<!-- Created -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">{formatDate(filter.createdAt)}</span>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => goto(`/filters/attach?filterId=${encodeURIComponent(filter.id)}`)}
										class="p-2 text-purple-600 hover:bg-purple-50 rounded-md transition-colors"
										title="Attach filter to resources"
									>
										<Link class="h-4 w-4" />
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
