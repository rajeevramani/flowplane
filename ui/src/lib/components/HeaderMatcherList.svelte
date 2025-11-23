<script lang="ts">
	import type { HeaderMatchDefinition } from '$lib/api/types';

	interface Props {
		headers: HeaderMatchDefinition[];
		onHeadersChange: (headers: HeaderMatchDefinition[]) => void;
	}

	let { headers, onHeadersChange }: Props = $props();

	const matchTypes = [
		{ value: 'exact', label: 'Exact' },
		{ value: 'regex', label: 'Regex' },
		{ value: 'present', label: 'Present' }
	];

	const commonHeaders = [
		'Content-Type',
		'Authorization',
		'X-Request-ID',
		'X-Forwarded-For',
		'Accept',
		'User-Agent'
	];

	function addHeader() {
		onHeadersChange([...headers, { name: '', value: '' }]);
	}

	function removeHeader(index: number) {
		onHeadersChange(headers.filter((_, i) => i !== index));
	}

	function updateHeader(index: number, field: keyof HeaderMatchDefinition | 'matchType', value: unknown) {
		onHeadersChange(
			headers.map((header, i) => {
				if (i === index) {
					const updated = { ...header };
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
				return header;
			})
		);
	}

	function getMatchType(header: HeaderMatchDefinition): string {
		if (header.present !== undefined) return 'present';
		if (header.regex !== undefined) return 'regex';
		return 'exact';
	}
</script>

<div class="space-y-2">
	<label class="block text-sm font-medium text-gray-600">Header Matchers</label>

	{#each headers as header, index}
		<div class="flex items-center gap-2">
			<input
				type="text"
				placeholder="Header name"
				list="common-headers"
				value={header.name}
				oninput={(e) => updateHeader(index, 'name', (e.target as HTMLInputElement).value)}
				class="w-40 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>

			<select
				value={getMatchType(header)}
				onchange={(e) => updateHeader(index, 'matchType', (e.target as HTMLSelectElement).value)}
				class="w-24 rounded-md border border-gray-300 bg-white px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			>
				{#each matchTypes as mt}
					<option value={mt.value}>{mt.label}</option>
				{/each}
			</select>

			{#if getMatchType(header) !== 'present'}
				<input
					type="text"
					placeholder={getMatchType(header) === 'regex' ? 'regex pattern' : 'value'}
					value={header.value ?? header.regex ?? ''}
					oninput={(e) =>
						updateHeader(
							index,
							getMatchType(header) === 'regex' ? 'regex' : 'value',
							(e.target as HTMLInputElement).value
						)}
					class="flex-1 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>
			{:else}
				<span class="flex-1 text-sm text-gray-500 italic">Header must be present</span>
			{/if}

			<button
				type="button"
				onclick={() => removeHeader(index)}
				class="rounded-md p-1.5 text-gray-400 hover:bg-gray-100 hover:text-red-500"
				title="Remove header matcher"
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

	<datalist id="common-headers">
		{#each commonHeaders as h}
			<option value={h}></option>
		{/each}
	</datalist>

	<button
		type="button"
		onclick={addHeader}
		class="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-700"
	>
		<svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
		</svg>
		Add Header Matcher
	</button>
</div>
