<script lang="ts">
	interface Props {
		headers: string[];
		onHeadersChange: (headers: string[]) => void;
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
		'Cookie'
	];

	const commonResponseHeaders = [
		'Server',
		'X-Powered-By',
		'X-AspNet-Version',
		'X-AspNetMvc-Version',
		'Set-Cookie',
		'Via',
		'X-Runtime'
	];

	const commonHeaders = headerType === 'request' ? commonRequestHeaders : commonResponseHeaders;

	function addHeader() {
		onHeadersChange([...headers, '']);
	}

	function removeHeader(index: number) {
		onHeadersChange(headers.filter((_, i) => i !== index));
	}

	function updateHeader(index: number, value: string) {
		onHeadersChange(headers.map((header, i) => (i === index ? value : header)));
	}
</script>

<div class="space-y-2">
	<label class="block text-sm font-medium text-gray-600">{label}</label>

	{#if headers.length === 0}
		<p class="text-xs text-gray-400 italic">No headers to remove</p>
	{/if}

	{#each headers as header, index}
		<div class="flex items-center gap-2">
			<input
				type="text"
				placeholder="Header name to remove"
				list="common-headers-remove-{headerType}"
				value={header}
				oninput={(e) => updateHeader(index, (e.target as HTMLInputElement).value)}
				class="flex-1 rounded-md border border-gray-300 px-2 py-1.5 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>

			<button
				type="button"
				onclick={() => removeHeader(index)}
				class="rounded-md p-1.5 text-gray-400 hover:bg-gray-100 hover:text-red-500"
				title="Remove from list"
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

	<datalist id="common-headers-remove-{headerType}">
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
		Add Header to Remove
	</button>
</div>
