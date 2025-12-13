<script lang="ts">
	import { Info, ChevronDown, ChevronRight } from 'lucide-svelte';
	import type { FilterTypeInfo } from '$lib/api/types';
	import {
		generateForm,
		getValueByPath,
		setValueByPath,
		type FormField,
		type FormSection,
		type FormConfig
	} from '$lib/utils/json-schema-form';
	import DynamicFormField from './DynamicFormField.svelte';

	interface Props {
		filterType: FilterTypeInfo;
		config: Record<string, unknown>;
		onConfigChange: (config: Record<string, unknown>) => void;
		errors?: Record<string, string[]>;
	}

	let { filterType, config, onConfigChange, errors = {} }: Props = $props();

	// Generate form configuration from schema and UI hints
	const formConfig = $derived<FormConfig>(
		generateForm(filterType.configSchema, filterType.uiHints)
	);

	// Track collapsed state for each section
	let collapsedSections = $state<Record<string, boolean>>({});

	// Initialize collapsed state based on UI hints
	$effect(() => {
		const initial: Record<string, boolean> = {};
		for (const section of formConfig.sections) {
			initial[section.name] = section.collapsedByDefault;
		}
		collapsedSections = initial;
	});

	// Get field value, using fullPath for nested fields extracted into sections
	function getFieldValue(field: FormField): unknown {
		const path = field.fullPath || field.name;
		return getValueByPath(config, path);
	}

	// Handle field change, using fullPath for nested fields extracted into sections
	function handleFieldChange(field: FormField, value: unknown) {
		const path = field.fullPath || field.name;
		onConfigChange(setValueByPath(config, path, value));
	}

	function toggleSection(sectionName: string) {
		collapsedSections = {
			...collapsedSections,
			[sectionName]: !collapsedSections[sectionName]
		};
	}

	function getFieldErrors(fieldName: string): string[] {
		return errors[fieldName] || [];
	}
</script>

<div class="space-y-6">
	<!-- Filter Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">{filterType.displayName}</p>
				<p class="mt-1">{filterType.description}</p>
			</div>
		</div>
	</div>

	<!-- Form Sections -->
	{#if formConfig.layout === 'flat'}
		<!-- Flat layout: all fields in a single column -->
		<div class="space-y-4">
			{#each formConfig.allFields as field}
				<DynamicFormField
					{field}
					value={getFieldValue(field)}
					onChange={(value) => handleFieldChange(field, value)}
					errors={getFieldErrors(field.fullPath || field.name)}
				/>
			{/each}
		</div>
	{:else}
		<!-- Sections layout -->
		{#each formConfig.sections as section}
			<div class="border border-gray-200 rounded-lg overflow-hidden">
				<!-- Section Header -->
				{#if section.collapsible}
					<button
						type="button"
						onclick={() => toggleSection(section.name)}
						class="w-full flex items-center justify-between px-4 py-3 bg-gray-50 hover:bg-gray-100 transition-colors"
					>
						<span class="text-sm font-medium text-gray-700">{section.name}</span>
						{#if collapsedSections[section.name]}
							<ChevronRight class="w-4 h-4 text-gray-500" />
						{:else}
							<ChevronDown class="w-4 h-4 text-gray-500" />
						{/if}
					</button>
				{:else}
					<div class="px-4 py-3 bg-gray-50 border-b border-gray-200">
						<span class="text-sm font-medium text-gray-700">{section.name}</span>
					</div>
				{/if}

				<!-- Section Content -->
				{#if !section.collapsible || !collapsedSections[section.name]}
					<div class="p-4 space-y-4">
						{#each section.fields as field}
							<DynamicFormField
								{field}
								value={getFieldValue(field)}
								onChange={(value) => handleFieldChange(field, value)}
								errors={getFieldErrors(field.fullPath || field.name)}
							/>
						{/each}
					</div>
				{/if}
			</div>
		{/each}
	{/if}
</div>
