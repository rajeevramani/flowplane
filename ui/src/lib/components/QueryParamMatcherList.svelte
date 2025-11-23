<script lang="ts">
	import type { QueryParameterMatchDefinition } from '$lib/api/types';

	interface Props {
		params: QueryParameterMatchDefinition[];
		onParamsChange: (params: QueryParameterMatchDefinition[]) => void;
	}

	let { params, onParamsChange }: Props = $props();

	const matchTypes = [
		{ value: 'exact', label: 'Exact' },
		{ value: 'regex', label: 'Regex' },
		{ value: 'present', label: 'Present' }
	];

	function addParam() {
		onParamsChange([...params, { name: '', value: '' }]);
	}

	function removeParam(index: number) {
		onParamsChange(params.filter((_, i) => i !== index));
	}

	function updateParam(index: number, field: keyof QueryParameterMatchDefinition | 'matchType', value: unknown) {
		onParamsChange(
			params.map((param, i) => {
				if (i === index) {
					const updated = { ...param };
					if (field === 'matchType') {
						// Clear other match fields when changing type
						delete updated.value;
						delete updated.regex;
						delete updated.present;
						if (value === 'exact') {
							updated.value = '';
						} else if (value === 'regex') {
							updated.regex = '';
						} else if (value === 'present') {
							updated.present = true;
						}
					} else {
						(updated as Record<string, unknown>)[field] = value;
					}
					return updated;
				}
				return param;
			})
		);
	}

	function getMatchType(param: QueryParameterMatchDefinition): string {
		if (param.present !== undefined) return 'present';
		if (param.regex !== undefined) return 'regex';
		return 'exact';
	}
</script>

<div class="space-y-2">
	<label class="block text-sm font-medium text-gray-600">Query Parameter Matchers</label>

	{#each params as param, index}
		<div class="flex items-center gap-2">
			<input
				type="text"
				placeholder="Parameter name"
				value={param.name}
				oninput={(e) => updateParam(index, 'name', (e.target as HTMLInputElement).value)}
				class="w-40 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>

			<select
				value={getMatchType(param)}
				onchange={(e) => updateParam(index, 'matchType', (e.target as HTMLSelectElement).value)}
				class="w-24 rounded-md border border-gray-300 bg-white px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			>
				{#each matchTypes as mt}
					<option value={mt.value}>{mt.label}</option>
				{/each}
			</select>

			{#if getMatchType(param) !== 'present'}
				<input
					type="text"
					placeholder={getMatchType(param) === 'regex' ? 'regex pattern' : 'value'}
					value={param.value ?? param.regex ?? ''}
					oninput={(e) =>
						updateParam(
							index,
							getMatchType(param) === 'regex' ? 'regex' : 'value',
							(e.target as HTMLInputElement).value
						)}
					class="flex-1 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>
			{:else}
				<span class="flex-1 text-sm text-gray-500 italic">Parameter must be present</span>
			{/if}

			<button
				type="button"
				onclick={() => removeParam(index)}
				class="rounded-md p-1.5 text-gray-400 hover:bg-gray-100 hover:text-red-500"
				title="Remove query parameter matcher"
			>
				<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M6 18L18 6M6 6l12 12"
					/>
				</svg>
			</button>
		</div>
	{/each}

	<button
		type="button"
		onclick={addParam}
		class="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-700"
	>
		<svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
		</svg>
		Add Query Parameter Matcher
	</button>
</div>
