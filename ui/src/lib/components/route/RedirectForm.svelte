<script lang="ts">
	import { REDIRECT_CODES } from "$lib/constants";

	interface Props {
		hostRedirect: string;
		pathRedirect: string;
		responseCode: number;
	}

	let {
		hostRedirect = $bindable(),
		pathRedirect = $bindable(),
		responseCode = $bindable(),
	}: Props = $props();

	// Derived: check if redirect is valid
	let isValid = $derived(hostRedirect || pathRedirect);
</script>

<div class="space-y-4">
	<div>
		<label
			for="host-redirect"
			class="block text-sm font-medium text-gray-700 mb-1"
		>
			Host Redirect
			<span class="text-xs text-gray-400">(optional)</span>
		</label>
		<input
			id="host-redirect"
			type="text"
			bind:value={hostRedirect}
			placeholder="example.com"
			class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
		/>
	</div>
	<div>
		<label
			for="path-redirect"
			class="block text-sm font-medium text-gray-700 mb-1"
		>
			Path Redirect
			<span class="text-xs text-gray-400">(optional)</span>
		</label>
		<input
			id="path-redirect"
			type="text"
			bind:value={pathRedirect}
			placeholder="/new-path"
			class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
		/>
	</div>
	<div>
		<label
			for="response-code"
			class="block text-sm font-medium text-gray-700 mb-1"
		>
			Response Code
		</label>
		<select
			id="response-code"
			bind:value={responseCode}
			class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
		>
			{#each REDIRECT_CODES as rc}
				<option value={rc.value}>{rc.label}</option>
			{/each}
		</select>
	</div>
	{#if !isValid}
		<p class="text-sm text-amber-600">
			Please specify at least a host or path redirect.
		</p>
	{/if}
</div>
