<script lang="ts">
	interface Props {
		schema: Record<string, unknown>;
		title?: string;
	}

	let { schema, title = 'Schema' }: Props = $props();

	let viewMode = $state<'tree' | 'json'>('tree');

	function getPropertyType(prop: Record<string, unknown>): string {
		if (prop.type === 'array') {
			const items = prop.items as Record<string, unknown> | undefined;
			return `array<${items?.type || 'unknown'}>`;
		}
		return String(prop.type || 'unknown');
	}

	let properties = $derived(
		schema.properties as Record<string, Record<string, unknown>> | undefined
	);
	let required = $derived((schema.required as string[]) || []);
</script>

<div class="border border-gray-200 rounded-lg overflow-hidden">
	<div class="flex items-center justify-between px-3 py-2 bg-gray-50 border-b border-gray-200">
		<span class="text-sm font-medium text-gray-700">{title}</span>
		<div class="flex gap-1">
			<button
				onclick={() => (viewMode = 'tree')}
				class="px-2 py-1 text-xs font-medium rounded transition-colors {viewMode === 'tree'
					? 'bg-gray-200 text-gray-800'
					: 'text-gray-600 hover:bg-gray-100'}"
			>
				Tree
			</button>
			<button
				onclick={() => (viewMode = 'json')}
				class="px-2 py-1 text-xs font-medium rounded transition-colors {viewMode === 'json'
					? 'bg-gray-200 text-gray-800'
					: 'text-gray-600 hover:bg-gray-100'}"
			>
				JSON
			</button>
		</div>
	</div>

	<div class="p-3 max-h-64 overflow-auto bg-white">
		{#if viewMode === 'json'}
			<pre class="text-xs font-mono text-gray-700 whitespace-pre-wrap">{JSON.stringify(schema, null, 2)}</pre>
		{:else if properties}
			<ul class="text-sm space-y-1">
				{#each Object.entries(properties) as [name, prop]}
					<li class="flex items-center gap-2">
						<span class="font-mono text-blue-600">{name}</span>
						<span class="text-gray-400">:</span>
						<span class="text-purple-600">{getPropertyType(prop)}</span>
						{#if required.includes(name)}
							<span class="text-xs text-red-500 font-medium">required</span>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<span class="text-sm text-gray-500">No properties defined</span>
		{/if}
	</div>
</div>
