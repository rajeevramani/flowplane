<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Loader2 } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { FilterType, FilterConfig, FilterTypeInfo } from '$lib/api/types';
	import Badge from '$lib/components/Badge.svelte';
	import DynamicFilterForm from '$lib/components/filters/DynamicFilterForm.svelte';
	import { generateDefaultValues, generateFormFields } from '$lib/utils/json-schema-form';
	import { ErrorAlert, FormActions, PageHeader } from '$lib/components/forms';
	import { validateRequired, validateMaxLength, runValidators } from '$lib/utils/validators';

	// Form state
	let currentTeam = $state<string>('');
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let isLoadingFilterTypes = $state(true);

	let filterName = $state('');
	let filterDescription = $state('');
	let filterType = $state<string>('header_mutation');

	// Dynamic filter types from API
	let filterTypes = $state<FilterTypeInfo[]>([]);
	let selectedFilterType = $derived(filterTypes.find((ft) => ft.name === filterType));

	// Generic config for dynamic forms
	let dynamicConfig = $state<Record<string, unknown>>({});

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Load filter types on mount
	onMount(async () => {
		try {
			const response = await apiClient.listFilterTypes();
			// Only show implemented filter types
			filterTypes = response.filterTypes.filter((ft) => ft.isImplemented);

			// Set default filter type
			if (filterTypes.length > 0 && !filterTypes.find((ft) => ft.name === filterType)) {
				filterType = filterTypes[0].name;
			}

			// Initialize config with default values for the selected filter type
			initializeConfig();
		} catch (e) {
			console.error('Failed to load filter types:', e);
			error = 'Failed to load available filter types';
		} finally {
			isLoadingFilterTypes = false;
		}
	});

	// Initialize config with default values when filter type changes
	function initializeConfig() {
		if (selectedFilterType?.configSchema) {
			const fields = generateFormFields(selectedFilterType.configSchema);
			dynamicConfig = generateDefaultValues(fields);
		} else {
			dynamicConfig = {};
		}
	}

	// Watch for filter type changes and reinitialize config
	$effect(() => {
		if (selectedFilterType) {
			initializeConfig();
		}
	});

	// Build the filter config based on filter type
	// Wrap in tagged enum format: { type: '...', config: {...} }
	// This matches Rust #[serde(tag = "type", content = "config")] serialization
	function buildFilterConfig(): FilterConfig {
		return {
			type: filterType as FilterConfig['type'],
			config: dynamicConfig
		} as FilterConfig;
	}

	// Validate form using reusable validators
	function validateForm(): string | null {
		return runValidators([
			() => validateRequired(filterName, 'Filter name'),
			() => validateMaxLength(filterName, 255, 'Filter name')
		]);
		// For dynamic forms, we rely on schema-based validation on the backend
	}

	// Handle form submission
	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;

		try {
			await apiClient.createFilter({
				name: filterName.trim(),
				filterType: filterType as FilterType,
				description: filterDescription.trim() || undefined,
				config: buildFilterConfig(),
				team: currentTeam
			});
			goto('/filters');
		} catch (e) {
			console.error('Create filter failed:', e);
			error = e instanceof Error ? e.message : 'Failed to create filter';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/filters');
	}

	function handleDynamicConfigChange(config: Record<string, unknown>) {
		dynamicConfig = config;
	}
</script>

<div class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Page Header with Back Button -->
	<PageHeader
		title="Create Filter"
		subtitle="Create a reusable filter configuration"
		onBack={handleCancel}
	/>

	<!-- Loading State -->
	{#if isLoadingFilterTypes}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<Loader2 class="w-8 h-8 text-blue-600 animate-spin" />
				<span class="text-sm text-gray-600">Loading filter types...</span>
			</div>
		</div>
	{:else}
		<!-- Error Message -->
		<ErrorAlert message={error} />

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
						placeholder="e.g., add-security-headers"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						A unique name to identify this filter within your team
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
					<textarea
						bind:value={filterDescription}
						placeholder="Optional description of what this filter does"
						rows="2"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					></textarea>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Team</label>
					<input
						type="text"
						value={currentTeam}
						disabled
						class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
					/>
					<p class="text-xs text-gray-500 mt-1">Filters are scoped to your current team</p>
				</div>
			</div>
		</div>

		<!-- Filter Type -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Filter Type</h2>
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Select Filter Type <span class="text-red-500">*</span>
					</label>
					<select
						bind:value={filterType}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						{#each filterTypes as ft}
							<option value={ft.name}>
								{ft.displayName} - {ft.description}
							</option>
						{/each}
					</select>
				</div>

				<!-- Attachment Points Info -->
				{#if selectedFilterType}
					<div class="p-3 bg-gray-50 border border-gray-200 rounded-md">
						<div class="flex items-center justify-between">
							<span class="text-sm font-medium text-gray-700">Can attach to:</span>
							<div class="flex gap-1">
								{#each selectedFilterType.attachmentPoints as point}
									<Badge variant="blue">{point}</Badge>
								{/each}
							</div>
						</div>
						<p class="text-sm text-gray-500 mt-2">{selectedFilterType.description}</p>
						{#if selectedFilterType.requiresListenerConfig}
							<p class="text-xs text-amber-600 mt-1">
								This filter requires listener-level configuration
							</p>
						{/if}
					</div>
				{/if}
			</div>
		</div>

		<!-- Configuration -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			{#if selectedFilterType}
				<h2 class="text-lg font-semibold text-gray-900 mb-4">
					{selectedFilterType.displayName} Configuration
				</h2>
				<DynamicFilterForm
					filterType={selectedFilterType}
					config={dynamicConfig}
					onConfigChange={handleDynamicConfigChange}
				/>
			{:else}
				<div class="text-center py-8 text-gray-500">
					<p>Select a filter type to configure.</p>
				</div>
			{/if}
		</div>

		<!-- Action Buttons -->
		<FormActions
			{isSubmitting}
			submitLabel="Create Filter"
			submittingLabel="Creating..."
			onSubmit={handleSubmit}
			onCancel={handleCancel}
		/>
	{/if}
</div>
