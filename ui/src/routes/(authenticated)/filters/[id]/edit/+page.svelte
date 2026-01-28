<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, Loader2, Server, Sliders, ExternalLink } from 'lucide-svelte';
	import type { FilterResponse, FilterConfig, FilterTypeInfo, FilterStatusResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import DynamicFilterForm from '$lib/components/filters/DynamicFilterForm.svelte';

	// Page state
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let filter = $state<FilterResponse | null>(null);
	let filterStatus = $state<FilterStatusResponse | null>(null);

	// Filter type info from API (provides schema for dynamic forms)
	let filterTypeInfo = $state<FilterTypeInfo | null>(null);

	// Form state
	let filterName = $state('');
	let filterDescription = $state('');

	// Dynamic config - used for all filter types
	let dynamicConfig = $state<Record<string, unknown>>({});

	function getFilterId(): string {
		return $page.params.id ?? '';
	}

	onMount(async () => {
		await loadFilter();
	});

	async function loadFilter() {
		isLoading = true;
		error = null;

		try {
			const data = await apiClient.getFilter(getFilterId());
			filter = data;

			// Populate form fields
			filterName = data.name;
			filterDescription = data.description || '';

			// Load filter type info for the dynamic form
			try {
				filterTypeInfo = await apiClient.getFilterType(data.filterType);
			} catch (e) {
				console.warn('Could not load filter type info:', e);
			}

			// Load filter status (installations + configurations)
			try {
				filterStatus = await apiClient.getFilterStatus(getFilterId());
			} catch (e) {
				console.warn('Could not load filter status:', e);
			}

			// Load the config from the filter data
			// The backend returns { type: 'filter_type', config: { ... } }
			// We need to extract just the config part for the dynamic form
			const filterConfig = data.config as unknown as { config: Record<string, unknown> };
			dynamicConfig = filterConfig.config || {};
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load filter';
		} finally {
			isLoading = false;
		}
	}

	function formatFilterType(type: string): string {
		return type
			.split('_')
			.map((word) => word.charAt(0).toUpperCase() + word.slice(1))
			.join(' ');
	}

	// Build filter config for submission
	function buildFilterConfig(): FilterConfig {
		if (!filter) {
			throw new Error('No filter loaded');
		}

		return {
			type: filter.filterType as FilterConfig['type'],
			config: dynamicConfig
		} as FilterConfig;
	}

	// Validation
	function validateForm(): string | null {
		if (!filterName.trim()) {
			return 'Filter name is required';
		}
		if (filterName.length > 255) {
			return 'Filter name must be 255 characters or less';
		}

		// For dynamic forms, we rely on backend schema validation
		return null;
	}

	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;
		try {
			await apiClient.updateFilter(getFilterId(), {
				name: filterName.trim(),
				description: filterDescription.trim() || undefined,
				config: buildFilterConfig()
			});
			goto('/filters');
		} catch (e) {
			console.error('Update filter failed:', e);
			error = e instanceof Error ? e.message : 'Failed to update filter';
		} finally {
			isSubmitting = false;
		}
	}

	async function handleDelete() {
		if (!confirm(`Are you sure you want to delete "${filter?.name}"? This cannot be undone.`)) {
			return;
		}
		try {
			await apiClient.deleteFilter(getFilterId());
			goto('/filters');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete filter';
		}
	}

	function handleCancel() {
		goto('/filters');
	}

	function handleDynamicConfigChange(config: Record<string, unknown>) {
		dynamicConfig = config;
	}

	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<Loader2 class="w-8 h-8 text-blue-600 animate-spin" />
				<span class="text-sm text-gray-600">Loading filter...</span>
			</div>
		</div>
	{:else if error && !filter}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
			<Button onclick={handleCancel} variant="secondary" size="sm">Back to Filters</Button>
		</div>
	{:else if filter}
		<!-- Header -->
		<div class="mb-6">
			<div class="flex items-center gap-4 mb-2">
				<button
					onclick={handleCancel}
					class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
				>
					<ArrowLeft class="w-5 h-5" />
				</button>
				<div class="flex-1">
					<div class="flex items-center gap-3">
						<h1 class="text-2xl font-bold text-gray-900">{filter.name}</h1>
						<Badge variant="blue"
							>{filterTypeInfo?.displayName || formatFilterType(filter.filterType)}</Badge
						>
						<span class="px-2 py-0.5 rounded text-xs font-medium bg-gray-100 text-gray-600">
							v{filter.version}
						</span>
					</div>
					{#if filter.description}
						<p class="text-sm text-gray-600 mt-1">{filter.description}</p>
					{/if}
				</div>
			</div>
		</div>

		{#if error}
			<div class="mb-6 bg-red-50 border border-red-200 rounded-md p-4">
				<p class="text-sm text-red-800">{error}</p>
			</div>
		{/if}

		<!-- Basic Information -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Filter Name <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={filterName}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
					<textarea
						bind:value={filterDescription}
						rows="2"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					></textarea>
				</div>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Team</label>
						<input
							type="text"
							value={filter.team}
							disabled
							class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
						/>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Filter Type</label>
						<input
							type="text"
							value={filterTypeInfo?.displayName || formatFilterType(filter.filterType)}
							disabled
							class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
						/>
						<p class="text-xs text-gray-500 mt-1">Filter type cannot be changed after creation</p>
					</div>
				</div>
			</div>
		</div>

		<!-- Attachment Points -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<div class="flex items-center justify-between mb-4">
				<h2 class="text-lg font-semibold text-gray-900">Attachment Points</h2>
				<div class="flex gap-1">
					{#if filterTypeInfo}
						{#each filterTypeInfo.attachmentPoints as point}
							<Badge variant="blue">{point}</Badge>
						{/each}
					{:else}
						<!-- Fallback for filters without type info -->
						<Badge variant="blue">route</Badge>
					{/if}
				</div>
			</div>
			<p class="text-sm text-gray-500">
				{filterTypeInfo?.description || `${formatFilterType(filter.filterType)} filter`}
			</p>
		</div>

		<!-- Install/Configure Status -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<div class="flex items-center justify-between mb-4">
				<h2 class="text-lg font-semibold text-gray-900">Install/Configure Status</h2>
				<div class="flex gap-2">
					<a
						href={`/filters/${getFilterId()}/install`}
						class="inline-flex items-center px-3 py-1.5 text-sm font-medium text-green-700 bg-green-50 border border-green-200 rounded-md hover:bg-green-100 transition-colors"
					>
						<Server class="h-4 w-4 mr-1.5" />
						Install
					</a>
					<a
						href={`/filters/${getFilterId()}/configure`}
						class="inline-flex items-center px-3 py-1.5 text-sm font-medium text-orange-700 bg-orange-50 border border-orange-200 rounded-md hover:bg-orange-100 transition-colors"
					>
						<Sliders class="h-4 w-4 mr-1.5" />
						Configure
					</a>
				</div>
			</div>

			{#if filterStatus}
				<div class="grid grid-cols-2 gap-6">
					<!-- Installations -->
					<div>
						<h3 class="text-sm font-medium text-gray-700 mb-2 flex items-center">
							<Server class="h-4 w-4 mr-1.5 text-green-600" />
							Installed On ({filterStatus.installations.length} listener{filterStatus.installations.length !== 1 ? 's' : ''})
						</h3>
						{#if filterStatus.installations.length > 0}
							<div class="space-y-1">
								{#each filterStatus.installations as inst}
									<div class="flex items-center justify-between px-2 py-1.5 bg-green-50 rounded text-sm">
										<span class="font-medium text-gray-900">{inst.listenerName}</span>
										<span class="text-gray-500 text-xs">Order: {inst.order}</span>
									</div>
								{/each}
							</div>
						{:else}
							<p class="text-sm text-gray-500 italic">Not installed on any listeners</p>
						{/if}
					</div>

					<!-- Configurations -->
					<div>
						<h3 class="text-sm font-medium text-gray-700 mb-2 flex items-center">
							<Sliders class="h-4 w-4 mr-1.5 text-orange-600" />
							Configured For ({filterStatus.configurations.length} scope{filterStatus.configurations.length !== 1 ? 's' : ''})
						</h3>
						{#if filterStatus.configurations.length > 0}
							<div class="space-y-1">
								{#each filterStatus.configurations as config}
									{@const typeColor = config.scopeType === 'route-config' ? 'blue' : config.scopeType === 'virtual-host' ? 'purple' : 'orange'}
									<div class="flex items-center justify-between px-2 py-1.5 bg-gray-50 rounded text-sm">
										<span class="font-medium text-gray-900">{config.scopeName}</span>
										<Badge variant={typeColor}>{config.scopeType}</Badge>
									</div>
								{/each}
							</div>
						{:else}
							<p class="text-sm text-gray-500 italic">Not configured for any scopes</p>
						{/if}
					</div>
				</div>
			{:else}
				<p class="text-sm text-gray-500">Loading status...</p>
			{/if}
		</div>

		<!-- Configuration Section -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">
				{filterTypeInfo?.displayName || formatFilterType(filter.filterType)} Configuration
			</h2>

			{#if filterTypeInfo}
				<DynamicFilterForm
					filterType={filterTypeInfo}
					config={dynamicConfig}
					onConfigChange={handleDynamicConfigChange}
				/>
			{:else}
				<!-- Fallback: show raw JSON for unknown filter types -->
				<div class="text-center py-8 text-gray-500">
					<p>Configuration editor is not available for this filter type.</p>
					<p class="text-sm mt-2">Current configuration:</p>
					<pre class="mt-4 p-4 bg-gray-50 rounded-md text-left text-sm overflow-auto">
{JSON.stringify(dynamicConfig, null, 2)}</pre>
				</div>
			{/if}
		</div>

		<!-- Metadata -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata</h2>
			<div class="grid grid-cols-2 gap-4 text-sm">
				<div>
					<span class="text-gray-500">Created:</span>
					<span class="text-gray-900 ml-2">{formatDate(filter.createdAt)}</span>
				</div>
				<div>
					<span class="text-gray-500">Updated:</span>
					<span class="text-gray-900 ml-2">{formatDate(filter.updatedAt)}</span>
				</div>
				<div>
					<span class="text-gray-500">Version:</span>
					<span class="text-gray-900 ml-2">{filter.version}</span>
				</div>
				<div>
					<span class="text-gray-500">Source:</span>
					<span class="text-gray-900 ml-2 capitalize">{filter.source}</span>
				</div>
			</div>
		</div>

		<!-- Actions -->
		<div class="flex justify-between">
			<Button onclick={handleDelete} variant="danger" disabled={isSubmitting}>Delete Filter</Button>
			<div class="flex gap-3">
				<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>Cancel</Button>
				<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
					{isSubmitting ? 'Saving...' : 'Save Changes'}
				</Button>
			</div>
		</div>
	{/if}
</div>
