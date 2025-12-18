<script lang="ts">
	import type { HeaderMutationEntry } from '$lib/api/types';

	interface Props {
		headers: HeaderMutationEntry[];
		onHeadersChange: (headers: HeaderMutationEntry[]) => void;
		label: string;
		headerType: 'request' | 'response';
	}

	let { headers, onHeadersChange, label, headerType }: Props = $props();

	const commonRequestHeaders = [
		'Authorization',
		'Content-Type',
		'Accept',
		'User-Agent',
		'X-Request-ID',
		'X-Forwarded-For',
		'X-Forwarded-Proto',
		'X-API-Key',
		'X-Custom-Header'
	];

	const commonResponseHeaders = [
		'Content-Type',
		'Cache-Control',
		'X-Content-Type-Options',
		'X-Frame-Options',
		'X-XSS-Protection',
		'Strict-Transport-Security',
		'Server',
		'X-Powered-By',
		'X-Custom-Header'
	];

	const commonHeaders = headerType === 'request' ? commonRequestHeaders : commonResponseHeaders;

	function addHeader() {
		onHeadersChange([...headers, { key: '', value: '', append: false }]);
	}

	function removeHeader(index: number) {
		onHeadersChange(headers.filter((_, i) => i !== index));
	}

	function updateHeader(
		index: number,
		field: keyof HeaderMutationEntry,
		value: string | boolean
	) {
		onHeadersChange(
			headers.map((header, i) => {
				if (i === index) {
					return { ...header, [field]: value };
				}
				return header;
			})
		);
	}
</script>

<div class="space-y-2">
	<label class="block text-sm font-medium text-gray-600">{label}</label>

	{#if headers.length === 0}
		<p class="text-xs text-gray-400 italic">No headers to add</p>
	{/if}

	{#each headers as header, index}
		<div class="flex items-center gap-2">
			<input
				type="text"
				placeholder="Header name"
				list="common-headers-{headerType}"
				value={header.key}
				oninput={(e) => updateHeader(index, 'key', (e.target as HTMLInputElement).value)}
				class="w-40 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>

			<input
				type="text"
				placeholder="Header value"
				value={header.value}
				oninput={(e) => updateHeader(index, 'value', (e.target as HTMLInputElement).value)}
				class="flex-1 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>

			<label class="flex items-center gap-1.5 text-xs text-gray-600">
				<input
					type="checkbox"
					checked={header.append}
					onchange={(e) => updateHeader(index, 'append', (e.target as HTMLInputElement).checked)}
					class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
				/>
				Append
			</label>

			<button
				type="button"
				onclick={() => removeHeader(index)}
				class="rounded-md p-1.5 text-gray-400 hover:bg-gray-100 hover:text-red-500"
				title="Remove header"
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

	<datalist id="common-headers-{headerType}">
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
		Add Header
	</button>

	<p class="text-xs text-gray-500 mt-1">
		<strong>Append mode:</strong> When enabled, adds value to existing header (comma-separated).
		When disabled, replaces existing header value.
	</p>
</div>
