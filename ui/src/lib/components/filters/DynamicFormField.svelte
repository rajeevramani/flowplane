<script lang="ts">
	import { Plus, Trash2 } from 'lucide-svelte';
	import type { FormField } from '$lib/utils/json-schema-form';
	import { getFieldDefaultValue } from '$lib/utils/json-schema-form';

	interface Props {
		field: FormField;
		value: unknown;
		onChange: (value: unknown) => void;
		errors?: string[];
		depth?: number;
	}

	let { field, value, onChange, errors = [], depth = 0 }: Props = $props();

	// For array fields, track items
	let arrayItems = $derived(Array.isArray(value) ? (value as unknown[]) : []);

	function handleStringChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onChange(target.value);
	}

	function handleNumberChange(e: Event) {
		const target = e.target as HTMLInputElement;
		const numValue = target.type === 'number' ? parseFloat(target.value) : parseInt(target.value);
		onChange(isNaN(numValue) ? 0 : numValue);
	}

	function handleBooleanChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onChange(target.checked);
	}

	function handleEnumChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		// Try to preserve the original type
		const option = field.options?.find((o) => String(o.value) === target.value);
		onChange(option?.value ?? target.value);
	}

	function handleNestedFieldChange(fieldName: string, fieldValue: unknown) {
		const currentObj = (value as Record<string, unknown>) || {};
		onChange({
			...currentObj,
			[fieldName]: fieldValue
		});
	}

	function addArrayItem() {
		if (field.itemSchema) {
			const defaultItem = getFieldDefaultValue(field.itemSchema);
			onChange([...arrayItems, defaultItem]);
		}
	}

	function removeArrayItem(index: number) {
		const newItems = [...arrayItems];
		newItems.splice(index, 1);
		onChange(newItems);
	}

	function updateArrayItem(index: number, itemValue: unknown) {
		const newItems = [...arrayItems];
		newItems[index] = itemValue;
		onChange(newItems);
	}

	const hasError = $derived(errors.length > 0);
	const inputClasses = $derived(
		`w-full px-3 py-2 text-sm border rounded-md focus:outline-none focus:ring-2 ${
			hasError
				? 'border-red-300 focus:ring-red-500 focus:border-red-500'
				: 'border-gray-300 focus:ring-blue-500 focus:border-blue-500'
		}`
	);
</script>

<div class="space-y-1" style:padding-left={depth > 0 ? `${depth * 16}px` : undefined}>
	<!-- Field Label -->
	<label class="flex items-center gap-1 text-sm font-medium text-gray-700">
		{field.label}
		{#if field.required}
			<span class="text-red-500">*</span>
		{/if}
	</label>

	<!-- Field Description -->
	{#if field.description}
		<p class="text-xs text-gray-500">{field.description}</p>
	{/if}

	<!-- Field Input -->
	{#if field.type === 'string'}
		{#if field.format === 'textarea' || (field.originalSchema.maxLength && field.originalSchema.maxLength > 200)}
			<textarea
				value={String(value ?? '')}
				oninput={handleStringChange}
				class={inputClasses}
				rows={3}
				placeholder={field.placeholder || `Enter ${field.label.toLowerCase()}`}
			></textarea>
		{:else}
			<input
				type={field.format === 'uri' ? 'url' : field.format === 'email' ? 'email' : 'text'}
				value={String(value ?? '')}
				oninput={handleStringChange}
				class={inputClasses}
				placeholder={field.placeholder || `Enter ${field.label.toLowerCase()}`}
			/>
		{/if}
	{:else if field.type === 'number' || field.type === 'integer'}
		<input
			type="number"
			value={Number(value ?? 0)}
			oninput={handleNumberChange}
			step={field.type === 'integer' ? 1 : 'any'}
			min={field.originalSchema.minimum}
			max={field.originalSchema.maximum}
			class={inputClasses}
			placeholder={field.placeholder || `Enter ${field.label.toLowerCase()}`}
		/>
	{:else if field.type === 'boolean'}
		<div class="flex items-center gap-2">
			<input
				type="checkbox"
				checked={Boolean(value)}
				onchange={handleBooleanChange}
				class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
			/>
			<span class="text-sm text-gray-600">Enabled</span>
		</div>
	{:else if field.type === 'enum'}
		<select value={String(value ?? '')} onchange={handleEnumChange} class={inputClasses}>
			<option value="" disabled>Select {field.label.toLowerCase()}</option>
			{#each field.options || [] as option}
				<option value={String(option.value)}>{option.label}</option>
			{/each}
		</select>
	{:else if field.type === 'object' && field.nested}
		<div class="border border-gray-200 rounded-md p-3 bg-gray-50 space-y-3">
			{#each field.nested as nestedField}
				<svelte:self
					field={nestedField}
					value={(value as Record<string, unknown>)?.[nestedField.name]}
					onChange={(v: unknown) => handleNestedFieldChange(nestedField.name, v)}
					depth={depth + 1}
				/>
			{/each}
		</div>
	{:else if field.type === 'object'}
		<!-- Object with additionalProperties (like headers map) - show as key-value JSON editor -->
		<textarea
			value={typeof value === 'object' && value !== null ? JSON.stringify(value, null, 2) : '{}'}
			oninput={(e) => {
				const target = e.target as HTMLTextAreaElement;
				const trimmed = target.value.trim();
				if (trimmed === '' || trimmed === '{}') {
					onChange({});
				} else {
					try {
						const parsed = JSON.parse(target.value);
						if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
							onChange(parsed);
						}
					} catch {
						// Keep current value if not valid JSON
					}
				}
			}}
			class={inputClasses}
			rows={3}
			placeholder={'{"key": "value"}'}
		></textarea>
		<p class="text-xs text-gray-500">Enter headers as JSON object</p>
	{:else if field.type === 'array' && field.itemSchema}
		<div class="space-y-2">
			{#each arrayItems as item, index}
				<div class="flex items-start gap-2">
					<div class="flex-1 border border-gray-200 rounded-md p-3 bg-gray-50">
						{#if field.itemSchema.type === 'object'}
							{#each field.itemSchema.nested || [] as nestedField}
								<svelte:self
									field={nestedField}
									value={(item as Record<string, unknown>)?.[nestedField.name]}
									onChange={(v: unknown) => {
										const currentItem = (item as Record<string, unknown>) || {};
										updateArrayItem(index, { ...currentItem, [nestedField.name]: v });
									}}
									depth={depth + 1}
								/>
							{/each}
						{:else}
							<svelte:self
								field={field.itemSchema}
								value={item}
								onChange={(v: unknown) => updateArrayItem(index, v)}
								depth={depth + 1}
							/>
						{/if}
					</div>
					<button
						type="button"
						onclick={() => removeArrayItem(index)}
						class="p-2 text-red-600 hover:bg-red-50 rounded transition-colors"
						title="Remove item"
					>
						<Trash2 class="w-4 h-4" />
					</button>
				</div>
			{/each}

			<button
				type="button"
				onclick={addArrayItem}
				class="flex items-center gap-1 px-3 py-1.5 text-sm text-blue-600 hover:bg-blue-50 rounded transition-colors"
			>
				<Plus class="w-4 h-4" />
				Add {field.label}
			</button>
		</div>
	{:else}
		<!-- Fallback for unknown types - show as JSON -->
		<textarea
			value={typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value ?? '')}
			oninput={(e) => {
				const target = e.target as HTMLTextAreaElement;
				try {
					onChange(JSON.parse(target.value));
				} catch {
					// Keep as string if not valid JSON
					onChange(target.value);
				}
			}}
			class={inputClasses}
			rows={4}
			placeholder="Enter JSON configuration"
		></textarea>
	{/if}

	<!-- Validation Errors -->
	{#if errors.length > 0}
		<div class="space-y-1">
			{#each errors as error}
				<p class="text-xs text-red-600">{error}</p>
			{/each}
		</div>
	{/if}
</div>
