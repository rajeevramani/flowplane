<!--
	RouteFilterCard.svelte

	Wrapper component for managing route-level filter overrides.
	Handles lazy loading of filter type info and per-route settings, then renders FilterOverridePanel.

	Features:
	- Loads FilterTypeInfo on mount via API
	- Lazy loads PerRouteSettings when panel is expanded
	- Saves settings changes via configureFilter API
	- Manages loading states and error handling
	- Wraps FilterOverridePanel with loaded data

	Usage example:
	```svelte
	<RouteFilterCard
		{filter}
		routeConfigName="my-routes"
		virtualHostName="example.com"
		routeName="api-route"
		isInherited={true}
		onRemove={() => handleRemove(filter.id)}
		onSettingsUpdate={() => refreshFilters()}
	/>
	```
-->
<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import type { FilterResponse, FilterTypeInfo, PerRouteSettings } from '$lib/api/types';
	import FilterOverridePanel from './FilterOverridePanel.svelte';
	import { AlertCircle, Loader2 } from 'lucide-svelte';

	interface Props {
		/** The filter being configured */
		filter: FilterResponse;
		/** Route config name for scope ID */
		routeConfigName: string;
		/** Virtual host name for scope ID */
		virtualHostName: string;
		/** Route name for scope ID */
		routeName: string;
		/** If filter is inherited from parent level */
		isInherited: boolean;
		/** Called when user wants to remove/detach the filter */
		onRemove?: () => void;
		/** Called after settings are successfully updated */
		onSettingsUpdate?: () => void;
	}

	let {
		filter,
		routeConfigName,
		virtualHostName,
		routeName,
		isInherited,
		onRemove,
		onSettingsUpdate
	}: Props = $props();

	// State management
	let filterTypeInfo = $state<FilterTypeInfo | null>(null);
	let settings = $state<PerRouteSettings | null>(null);
	let isLoadingType = $state(true);
	let isLoadingSettings = $state(false);
	let isSaving = $state(false);
	let loadError = $state<string | null>(null);
	let settingsLoadError = $state<string | null>(null);
	let hasLoadedSettings = $state(false);

	// Construct scope ID for this route - use $derived for reactivity
	const scopeId = $derived(`${routeConfigName}/${virtualHostName}/${routeName}`);

	// Load filter type info on mount
	$effect(() => {
		loadFilterTypeInfo();
	});

	async function loadFilterTypeInfo() {
		try {
			isLoadingType = true;
			loadError = null;
			filterTypeInfo = await apiClient.getFilterType(filter.filterType);
		} catch (error) {
			console.error('Failed to load filter type info:', error);
			loadError = error instanceof Error ? error.message : 'Failed to load filter type info';
		} finally {
			isLoadingType = false;
		}
	}

	async function loadSettings() {
		if (hasLoadedSettings || !filterTypeInfo) return;

		try {
			isLoadingSettings = true;
			settingsLoadError = null;

			// Get all configurations for this filter
			const configurationsResponse = await apiClient.listFilterConfigurations(filter.id);

			// Find the configuration matching this route scope
			const routeConfig = configurationsResponse.configurations.find(
				(config) => config.scopeType === 'route' && config.scopeId === scopeId
			);

			if (routeConfig && routeConfig.settings) {
				// Parse settings from the configuration - type assertion is safe here as backend validates schema
				settings = routeConfig.settings as unknown as PerRouteSettings;
			} else {
				settings = null;
			}

			hasLoadedSettings = true;
		} catch (error) {
			console.error('Failed to load filter settings:', error);
			settingsLoadError =
				error instanceof Error ? error.message : 'Failed to load filter settings';
		} finally {
			isLoadingSettings = false;
		}
	}

	async function handleSettingsChange(newSettings: PerRouteSettings | null) {
		try {
			isSaving = true;
			settingsLoadError = null;

			if (newSettings === null) {
				// Remove the configuration (reset to base)
				await apiClient.removeFilterConfiguration(filter.id, 'route', scopeId);
				settings = null;
			} else {
				// Save or update the configuration
				await apiClient.configureFilter(filter.id, {
					scopeType: 'route',
					scopeId,
					settings: newSettings
				});
				settings = newSettings;
			}

			// Notify parent of successful update
			onSettingsUpdate?.();
		} catch (error) {
			console.error('Failed to save filter settings:', error);
			settingsLoadError =
				error instanceof Error ? error.message : 'Failed to save filter settings';
		} finally {
			isSaving = false;
		}
	}

	// Load settings when FilterOverridePanel requests it (via expand)
	function handlePanelExpand() {
		if (!hasLoadedSettings) {
			loadSettings();
		}
	}
</script>

{#if loadError}
	<div class="border border-red-200 rounded-lg bg-red-50 p-4">
		<div class="flex items-start gap-2">
			<AlertCircle class="h-5 w-5 text-red-600 mt-0.5 flex-shrink-0" />
			<div class="flex-1">
				<h4 class="text-sm font-medium text-red-900">Failed to load filter</h4>
				<p class="text-sm text-red-700 mt-1">{loadError}</p>
				<button
					onclick={loadFilterTypeInfo}
					class="mt-2 text-sm text-red-600 hover:text-red-700 font-medium"
				>
					Retry
				</button>
			</div>
		</div>
	</div>
{:else if isLoadingType}
	<div class="border border-gray-200 rounded-lg bg-white p-4">
		<div class="flex items-center gap-2 text-gray-600">
			<Loader2 class="h-4 w-4 animate-spin" />
			<span class="text-sm">Loading filter information...</span>
		</div>
	</div>
{:else if filterTypeInfo}
	<div class="relative">
		{#if settingsLoadError}
			<div class="mb-2 border border-amber-200 rounded-lg bg-amber-50 p-2">
				<div class="flex items-start gap-2">
					<AlertCircle class="h-4 w-4 text-amber-600 mt-0.5 flex-shrink-0" />
					<div class="flex-1">
						<p class="text-xs text-amber-700">{settingsLoadError}</p>
						<button onclick={loadSettings} class="mt-1 text-xs text-amber-600 hover:text-amber-700 font-medium">
							Retry loading settings
						</button>
					</div>
				</div>
			</div>
		{/if}

		{#if isLoadingSettings}
			<div class="absolute inset-0 bg-white bg-opacity-75 rounded-lg flex items-center justify-center z-10">
				<div class="flex items-center gap-2 text-gray-600">
					<Loader2 class="h-4 w-4 animate-spin" />
					<span class="text-sm">Loading settings...</span>
				</div>
			</div>
		{/if}

		{#if isSaving}
			<div class="absolute inset-0 bg-white bg-opacity-75 rounded-lg flex items-center justify-center z-10">
				<div class="flex items-center gap-2 text-blue-600">
					<Loader2 class="h-4 w-4 animate-spin" />
					<span class="text-sm">Saving settings...</span>
				</div>
			</div>
		{/if}

		<FilterOverridePanel
			{filter}
			{filterTypeInfo}
			{settings}
			{isInherited}
			onSettingsChange={handleSettingsChange}
			{onRemove}
			onExpand={handlePanelExpand}
		/>
	</div>
{/if}
