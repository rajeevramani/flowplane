<script lang="ts">
	import type { EndpointRequest } from '$lib/api/types';

	interface Props {
		endpoints: EndpointRequest[];
		lbPolicy?: string;
		onEndpointsChange: (endpoints: EndpointRequest[]) => void;
		onLbPolicyChange?: (policy: string) => void;
	}

	let { endpoints, lbPolicy = 'ROUND_ROBIN', onEndpointsChange, onLbPolicyChange }: Props =
		$props();

	const lbPolicies = [
		{ value: 'ROUND_ROBIN', label: 'Round Robin' },
		{ value: 'LEAST_REQUEST', label: 'Least Request' },
		{ value: 'RANDOM', label: 'Random' },
		{ value: 'RING_HASH', label: 'Ring Hash' },
		{ value: 'MAGLEV', label: 'Maglev' }
	];

	function addEndpoint() {
		onEndpointsChange([...endpoints, { host: '', port: 8080 }]);
	}

	function removeEndpoint(index: number) {
		if (endpoints.length > 1) {
			const newEndpoints = endpoints.filter((_, i) => i !== index);
			onEndpointsChange(newEndpoints);
		}
	}

	function updateEndpoint(index: number, field: 'host' | 'port', value: string | number) {
		const newEndpoints = endpoints.map((endpoint, i) => {
			if (i === index) {
				return { ...endpoint, [field]: field === 'port' ? Number(value) : value };
			}
			return endpoint;
		});
		onEndpointsChange(newEndpoints);
	}

	function handleLbPolicyChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onLbPolicyChange?.(target.value);
	}

	$effect(() => {
		// Show LB policy only when there are multiple endpoints
		if (endpoints.length <= 1 && lbPolicy !== 'ROUND_ROBIN') {
			onLbPolicyChange?.('ROUND_ROBIN');
		}
	});
</script>

<div class="space-y-3">
	<label class="block text-sm font-medium text-gray-700">Upstream Endpoints</label>

	{#each endpoints as endpoint, index}
		<div class="flex items-center gap-2">
			<input
				type="text"
				placeholder="hostname or IP"
				value={endpoint.host}
				oninput={(e) => updateEndpoint(index, 'host', (e.target as HTMLInputElement).value)}
				class="flex-1 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>
			<span class="text-gray-500">:</span>
			<input
				type="number"
				min="1"
				max="65535"
				placeholder="port"
				value={endpoint.port}
				oninput={(e) => updateEndpoint(index, 'port', (e.target as HTMLInputElement).value)}
				class="w-24 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			/>
			<button
				type="button"
				onclick={() => removeEndpoint(index)}
				disabled={endpoints.length <= 1}
				class="rounded-md p-2 text-gray-400 hover:bg-gray-100 hover:text-red-500 disabled:cursor-not-allowed disabled:opacity-50"
				title="Remove endpoint"
			>
				<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
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
		onclick={addEndpoint}
		class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
	>
		<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
		</svg>
		Add Endpoint
	</button>

	{#if endpoints.length > 1}
		<div class="mt-4">
			<label for="lb-policy" class="block text-sm font-medium text-gray-700"
				>Load Balancing Policy</label
			>
			<select
				id="lb-policy"
				value={lbPolicy}
				onchange={handleLbPolicyChange}
				class="mt-1 block w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			>
				{#each lbPolicies as policy}
					<option value={policy.value}>{policy.label}</option>
				{/each}
			</select>
		</div>
	{/if}
</div>
