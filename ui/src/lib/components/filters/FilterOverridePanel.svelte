<!--
	FilterOverridePanel.svelte

	A reusable component for managing per-route filter overrides in the route edit page.
	This component handles the 3-level filter hierarchy: route_config → virtual_host → route.

	Features:
	- Display filter information with type badges and status indicators
	- Support for inherited filters (from route config or virtual host)
	- Per-route override behaviors: use_base, disable, override
	- Dynamic form for full config overrides (when filter supports it)
	- Reference-only overrides (for JWT auth requirement names)
	- Explicit Save/Cancel buttons (no auto-save)

	Usage example:
	```svelte
	<script>
		import { FilterOverridePanel } from '$lib/components/filters';

		let filter = { ... }; // FilterResponse from API
		let filterTypeInfo = { ... }; // FilterTypeInfo from API
		let settings = null; // Current PerRouteSettings or null

		function handleSettingsChange(newSettings) {
			// Save settings to backend
			settings = newSettings;
		}

		function handleRemove() {
			// Detach filter from route
		}
	</script>

	<FilterOverridePanel
		{filter}
		{filterTypeInfo}
		{settings}
		isInherited={true}
		onSettingsChange={handleSettingsChange}
		onRemove={handleRemove}
	/>
	```
-->
<script lang="ts">
	import { Settings, Trash2, ChevronDown, ChevronUp, Save, X } from 'lucide-svelte';
	import type { FilterResponse, FilterTypeInfo, PerRouteSettings } from '$lib/api/types';
	import DynamicFilterForm from './DynamicFilterForm.svelte';

	interface Props {
		/** The filter being configured */
		filter: FilterResponse;
		/** Schema and UI hints for the filter */
		filterTypeInfo: FilterTypeInfo;
		/** Current override settings (from server) */
		settings: PerRouteSettings | null;
		/** If filter is inherited from parent level (VH or config) */
		isInherited?: boolean;
		/** Called when user explicitly saves settings */
		onSettingsChange: (settings: PerRouteSettings | null) => void;
		/** Called when user wants to detach the filter */
		onRemove?: () => void;
		/** Called when the override panel is expanded */
		onExpand?: () => void;
	}

	let {
		filter,
		filterTypeInfo,
		settings,
		isInherited = false,
		onSettingsChange,
		onRemove,
		onExpand
	}: Props = $props();

	// Local editing state (not synced to server until Save is clicked)
	let showOverridePanel = $state(false);
	let behavior = $state<'use_base' | 'disable' | 'override'>(settings?.behavior ?? 'use_base');
	let overrideConfig = $state<Record<string, unknown>>(settings?.config ?? {});
	let requirementName = $state(settings?.requirementName ?? '');

	// Track if user has made changes that need saving
	let isDirty = $state(false);

	// Initialize from settings when they first become available
	let hasInitialized = $state(false);
	$effect(() => {
		if (!hasInitialized && settings !== null) {
			behavior = settings.behavior;
			overrideConfig = settings.config ?? {};
			requirementName = settings.requirementName ?? '';
			if (settings.behavior !== 'use_base') {
				showOverridePanel = true;
			}
			hasInitialized = true;
			isDirty = false;
		} else if (hasInitialized && settings === null) {
			// Settings cleared externally
			behavior = 'use_base';
			overrideConfig = {};
			requirementName = '';
			showOverridePanel = false;
			hasInitialized = false;
			isDirty = false;
		}
	});

	// Derived state for badge display - based on SAVED settings, not local edits
	const hasOverride = $derived(settings !== null && settings.behavior !== 'use_base');
	const canOverride = $derived(filterTypeInfo.perRouteBehavior !== 'not_supported');
	const isConfigurable = $derived(
		filterTypeInfo.perRouteBehavior === 'full_config' ||
			filterTypeInfo.perRouteBehavior === 'reference_only'
	);

	function getFilterTypeLabel(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'Header Mutation';
			case 'jwt_auth':
				return 'JWT Auth';
			case 'cors':
				return 'CORS';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'Rate Limit';
			case 'ext_authz':
				return 'External Auth';
			case 'custom_response':
				return 'Custom Response';
			case 'mcp':
				return 'MCP';
			default:
				return filterType;
		}
	}

	function getFilterTypeBadgeColor(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'bg-blue-100 text-blue-800';
			case 'jwt_auth':
				return 'bg-green-100 text-green-800';
			case 'cors':
				return 'bg-purple-100 text-purple-800';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'bg-amber-100 text-amber-800';
			case 'ext_authz':
				return 'bg-red-100 text-red-800';
			case 'custom_response':
				return 'bg-indigo-100 text-indigo-800';
			case 'mcp':
				return 'bg-teal-100 text-teal-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	// Handle local changes (no API call - just update local state)
	function handleBehaviorChange(newBehavior: 'use_base' | 'disable' | 'override') {
		behavior = newBehavior;
		isDirty = true;
	}

	function handleConfigChange(config: Record<string, unknown>) {
		overrideConfig = config;
		isDirty = true;
	}

	function handleRequirementChange(event: Event) {
		const target = event.target as HTMLInputElement;
		requirementName = target.value;
		isDirty = true;
	}

	// Save changes to server
	function handleSave() {
		if (behavior === 'use_base') {
			onSettingsChange(null);
		} else if (behavior === 'disable') {
			onSettingsChange({ behavior: 'disable' });
		} else if (behavior === 'override') {
			if (filterTypeInfo.perRouteBehavior === 'reference_only') {
				onSettingsChange({
					behavior: 'override',
					requirementName: requirementName || undefined
				});
			} else {
				onSettingsChange({
					behavior: 'override',
					config: Object.keys(overrideConfig).length > 0 ? overrideConfig : undefined
				});
			}
		}
		isDirty = false;
	}

	// Cancel changes and revert to saved state
	function handleCancel() {
		behavior = settings?.behavior ?? 'use_base';
		overrideConfig = settings?.config ?? {};
		requirementName = settings?.requirementName ?? '';
		isDirty = false;
	}

	function toggleOverridePanel() {
		const wasHidden = !showOverridePanel;
		showOverridePanel = !showOverridePanel;

		// Call onExpand when panel is being expanded
		if (wasHidden && showOverridePanel && onExpand) {
			onExpand();
		}
	}

	function handleOverrideClick() {
		if (!hasOverride && behavior === 'use_base') {
			// Start with disable as the default override
			behavior = 'disable';
			isDirty = true;
		}
		const wasHidden = !showOverridePanel;
		showOverridePanel = true;

		// Call onExpand when panel is being expanded
		if (wasHidden && onExpand) {
			onExpand();
		}
	}

	function handleResetClick() {
		behavior = 'use_base';
		overrideConfig = {};
		requirementName = '';
		onSettingsChange(null);
		isDirty = false;
	}
</script>

<div class="border border-gray-200 rounded-lg bg-white">
	<!-- Card Header -->
	<div class="p-3 flex items-center justify-between">
		<div class="flex items-center gap-2 flex-1 min-w-0">
			<span class="text-sm font-medium text-gray-900 truncate">{filter.name}</span>
			<span
				class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded {getFilterTypeBadgeColor(
					filter.filterType
				)}"
			>
				{getFilterTypeLabel(filter.filterType)}
			</span>
			{#if isInherited}
				<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-gray-100 text-gray-600">
					inherited
				</span>
			{/if}
			{#if canOverride}
				<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-blue-100 text-blue-600">
					overridable
				</span>
			{/if}
			{#if hasOverride}
				<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-orange-100 text-orange-600">
					overridden
				</span>
			{/if}
			{#if isDirty}
				<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-yellow-100 text-yellow-700">
					unsaved
				</span>
			{/if}
		</div>

		<!-- Actions -->
		<div class="flex items-center gap-2">
			{#if hasOverride || (!hasOverride && !isInherited)}
				<button
					onclick={toggleOverridePanel}
					class="px-3 py-1.5 text-sm text-blue-600 hover:bg-blue-50 rounded-md transition-colors flex items-center gap-1.5"
					title={showOverridePanel ? 'Hide configuration' : 'Show configuration'}
				>
					<Settings class="h-3.5 w-3.5" />
					Configure
					{#if showOverridePanel}
						<ChevronUp class="h-3.5 w-3.5" />
					{:else}
						<ChevronDown class="h-3.5 w-3.5" />
					{/if}
				</button>
			{/if}

			{#if isInherited && canOverride}
				<button
					onclick={handleOverrideClick}
					class="px-3 py-1.5 text-sm text-orange-600 hover:bg-orange-50 rounded-md transition-colors"
					title="Create route-level override"
				>
					Override
				</button>
			{/if}

			{#if !isInherited && onRemove}
				<button
					onclick={onRemove}
					class="p-1.5 text-red-600 hover:bg-red-50 rounded-md transition-colors"
					title="Remove filter"
				>
					<Trash2 class="h-4 w-4" />
				</button>
			{/if}
		</div>
	</div>

	<!-- Override Panel -->
	{#if showOverridePanel}
		<div
			class="border-t px-4 py-4 space-y-4"
			class:bg-orange-50={hasOverride}
			class:border-orange-200={hasOverride}
			class:bg-blue-50={isInherited && !hasOverride}
			class:border-blue-200={isInherited && !hasOverride}
		>
			<!-- Panel Header -->
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-2">
					<Settings class="h-4 w-4 text-gray-500" />
					<h4 class="text-sm font-medium text-gray-700">Route-level Override</h4>
				</div>
				{#if hasOverride}
					<button
						onclick={handleResetClick}
						class="px-2 py-1 text-xs text-gray-600 hover:bg-white rounded transition-colors"
					>
						Reset to default
					</button>
				{/if}
			</div>

			<!-- Behavior Selection -->
			<div class="space-y-2">
				<label class="block text-sm font-medium text-gray-700">Behavior</label>
				<div class="space-y-2">
					<!-- Use base configuration -->
					<label
						class="flex items-start gap-3 p-3 border rounded-lg cursor-pointer transition-colors bg-white"
						class:border-blue-500={behavior === 'use_base'}
						class:bg-blue-50={behavior === 'use_base'}
						class:border-gray-200={behavior !== 'use_base'}
						class:hover:bg-gray-50={behavior !== 'use_base'}
					>
						<input
							type="radio"
							name="behavior-{filter.id}"
							value="use_base"
							checked={behavior === 'use_base'}
							onchange={() => handleBehaviorChange('use_base')}
							class="mt-1 h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm font-medium text-gray-900">Use base configuration</span>
							<p class="text-xs text-gray-500 mt-0.5">
								Apply the inherited filter configuration from route config or virtual host
							</p>
						</div>
					</label>

					<!-- Disable for this route -->
					{#if filterTypeInfo.perRouteBehavior !== 'not_supported'}
						<label
							class="flex items-start gap-3 p-3 border rounded-lg cursor-pointer transition-colors bg-white"
							class:border-blue-500={behavior === 'disable'}
							class:bg-blue-50={behavior === 'disable'}
							class:border-gray-200={behavior !== 'disable'}
							class:hover:bg-gray-50={behavior !== 'disable'}
						>
							<input
								type="radio"
								name="behavior-{filter.id}"
								value="disable"
								checked={behavior === 'disable'}
								onchange={() => handleBehaviorChange('disable')}
								class="mt-1 h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
							/>
							<div>
								<span class="text-sm font-medium text-gray-900">Disable for this route</span>
								<p class="text-xs text-gray-500 mt-0.5">
									Skip this filter completely for requests matching this route
								</p>
							</div>
						</label>
					{/if}

					<!-- Override configuration -->
					{#if isConfigurable}
						<label
							class="flex items-start gap-3 p-3 border rounded-lg cursor-pointer transition-colors bg-white"
							class:border-blue-500={behavior === 'override'}
							class:bg-blue-50={behavior === 'override'}
							class:border-gray-200={behavior !== 'override'}
							class:hover:bg-gray-50={behavior !== 'override'}
						>
							<input
								type="radio"
								name="behavior-{filter.id}"
								value="override"
								checked={behavior === 'override'}
								onchange={() => handleBehaviorChange('override')}
								class="mt-1 h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
							/>
							<div>
								<span class="text-sm font-medium text-gray-900">Override configuration</span>
								<p class="text-xs text-gray-500 mt-0.5">
									{#if filterTypeInfo.perRouteBehavior === 'reference_only'}
										Specify a custom requirement name for this route
									{:else}
										Use custom filter settings for this route
									{/if}
								</p>
							</div>
						</label>
					{/if}
				</div>
			</div>

			<!-- Override Configuration Form -->
			{#if behavior === 'override'}
				<div class="border-t border-gray-200 pt-4 bg-white rounded-lg p-4">
					{#if filterTypeInfo.perRouteBehavior === 'reference_only'}
						<!-- JWT-style requirement name input -->
						<div>
							<label for="requirement-name-{filter.id}" class="block text-sm font-medium text-gray-700 mb-1">
								Requirement Name
							</label>
							<input
								id="requirement-name-{filter.id}"
								type="text"
								value={requirementName}
								oninput={handleRequirementChange}
								placeholder="Enter requirement name from provider config"
								class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:ring-blue-500 focus:border-blue-500"
							/>
							<p class="text-xs text-gray-500 mt-1">
								Reference a requirement defined in the filter's base configuration.
							</p>
						</div>
					{:else if filterTypeInfo.perRouteBehavior === 'full_config'}
						<!-- Full config override using dynamic form -->
						<div>
							<h4 class="text-sm font-medium text-gray-700 mb-3">Override Configuration</h4>
							<DynamicFilterForm
								filterType={filterTypeInfo}
								config={overrideConfig}
								onConfigChange={handleConfigChange}
							/>
						</div>
					{/if}
				</div>
			{/if}

			<!-- Save/Cancel Buttons -->
			{#if isDirty}
				<div class="flex items-center justify-end gap-2 pt-3 border-t border-gray-200">
					<button
						onclick={handleCancel}
						class="px-3 py-1.5 text-sm text-gray-600 hover:bg-gray-100 rounded-md transition-colors flex items-center gap-1.5"
					>
						<X class="h-3.5 w-3.5" />
						Cancel
					</button>
					<button
						onclick={handleSave}
						class="px-3 py-1.5 text-sm text-white bg-blue-600 hover:bg-blue-700 rounded-md transition-colors flex items-center gap-1.5"
					>
						<Save class="h-3.5 w-3.5" />
						Save Override
					</button>
				</div>
			{/if}
		</div>
	{/if}
</div>
