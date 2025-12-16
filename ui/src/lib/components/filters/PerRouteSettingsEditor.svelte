<script lang="ts">
	import type { FilterConfigBehavior, PerRouteSettings, FilterTypeInfo } from '$lib/api/types';
	import DynamicFilterForm from './DynamicFilterForm.svelte';
	import { Info } from 'lucide-svelte';

	interface Props {
		/** Filter type info for schema-based form generation */
		filterTypeInfo: FilterTypeInfo;
		/** Current settings (null = not configured) */
		settings: PerRouteSettings | null;
		/** Callback when settings change */
		onSettingsChange: (settings: PerRouteSettings | null) => void;
		/** Whether the form is disabled */
		disabled?: boolean;
	}

	let { filterTypeInfo, settings, onSettingsChange, disabled = false }: Props = $props();

	// Local state for behavior selection
	let behavior = $state<FilterConfigBehavior>(settings?.behavior ?? 'use_base');
	let overrideConfig = $state<Record<string, unknown>>(settings?.config ?? {});
	let requirementName = $state(settings?.requirementName ?? '');

	// Sync local state when props change
	$effect(() => {
		if (settings) {
			behavior = settings.behavior;
			overrideConfig = settings.config ?? {};
			requirementName = settings.requirementName ?? '';
		} else {
			behavior = 'use_base';
			overrideConfig = {};
			requirementName = '';
		}
	});

	// Get available behaviors based on filter type's perRouteBehavior
	const availableBehaviors = $derived(() => {
		const behaviors: { value: FilterConfigBehavior; label: string; description: string }[] = [
			{ value: 'use_base', label: 'Use base config', description: 'Apply the base filter configuration' }
		];

		switch (filterTypeInfo.perRouteBehavior) {
			case 'full_config':
				behaviors.push(
					{ value: 'disable', label: 'Disable', description: 'Skip this filter for this scope' },
					{ value: 'override', label: 'Override settings', description: 'Use custom configuration for this scope' }
				);
				break;
			case 'reference_only':
				behaviors.push(
					{ value: 'disable', label: 'Disable', description: 'Skip this filter for this scope' },
					{ value: 'override', label: 'Use requirement', description: 'Reference a specific requirement name' }
				);
				break;
			case 'disable_only':
				behaviors.push(
					{ value: 'disable', label: 'Disable', description: 'Skip this filter for this scope' }
				);
				break;
			case 'not_supported':
				// Only use_base is available
				break;
		}

		return behaviors;
	});

	function handleBehaviorChange(newBehavior: FilterConfigBehavior) {
		behavior = newBehavior;
		emitChange();
	}

	function handleConfigChange(config: Record<string, unknown>) {
		overrideConfig = config;
		emitChange();
	}

	function handleRequirementChange(event: Event) {
		const target = event.target as HTMLInputElement;
		requirementName = target.value;
		emitChange();
	}

	function emitChange() {
		if (behavior === 'use_base') {
			// Use base means no special settings
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
	}

	// Get description for the current filter type's per-route behavior
	const behaviorDescription = $derived(() => {
		switch (filterTypeInfo.perRouteBehavior) {
			case 'full_config':
				return 'This filter supports full configuration override at the route level.';
			case 'reference_only':
				return 'This filter uses requirement references. Override by specifying a requirement name.';
			case 'disable_only':
				return 'This filter can only be disabled at the route level; configuration cannot be overridden.';
			case 'not_supported':
				return 'This filter does not support per-route configuration.';
			default:
				return '';
		}
	});
</script>

<div class="space-y-4">
	<!-- Info banner about per-route behavior -->
	<div class="bg-purple-50 border border-purple-200 rounded-lg p-3">
		<div class="flex items-start gap-2">
			<Info class="h-4 w-4 text-purple-500 mt-0.5 flex-shrink-0" />
			<p class="text-sm text-purple-700">{behaviorDescription()}</p>
		</div>
	</div>

	<!-- Behavior selection -->
	<div class="space-y-2">
		<label class="block text-sm font-medium text-gray-700">Behavior</label>
		<div class="space-y-2">
			{#each availableBehaviors() as option}
				<label
					class="flex items-start gap-3 p-3 border rounded-lg cursor-pointer transition-colors"
					class:border-purple-500={behavior === option.value}
					class:bg-purple-50={behavior === option.value}
					class:border-gray-200={behavior !== option.value}
					class:hover:bg-gray-50={behavior !== option.value && !disabled}
					class:opacity-50={disabled}
				>
					<input
						type="radio"
						name="behavior"
						value={option.value}
						checked={behavior === option.value}
						onchange={() => handleBehaviorChange(option.value)}
						{disabled}
						class="mt-1 h-4 w-4 text-purple-600 border-gray-300 focus:ring-purple-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-900">{option.label}</span>
						<p class="text-xs text-gray-500 mt-0.5">{option.description}</p>
					</div>
				</label>
			{/each}
		</div>
	</div>

	<!-- Override configuration form -->
	{#if behavior === 'override'}
		<div class="border-t border-gray-200 pt-4">
			{#if filterTypeInfo.perRouteBehavior === 'reference_only'}
				<!-- JWT-style requirement name input -->
				<div>
					<label for="requirement-name" class="block text-sm font-medium text-gray-700 mb-1">
						Requirement Name
					</label>
					<input
						id="requirement-name"
						type="text"
						value={requirementName}
						oninput={handleRequirementChange}
						placeholder="Enter requirement name from provider config"
						{disabled}
						class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:ring-purple-500 focus:border-purple-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Reference a requirement defined in the filter's base configuration.
					</p>
				</div>
			{:else if filterTypeInfo.perRouteBehavior === 'full_config'}
				<!-- Full config override using dynamic form -->
				<div class="bg-gray-50 border border-gray-200 rounded-lg p-4">
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
</div>
