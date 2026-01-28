<script lang="ts">
	import type { McpConnectionStatus } from '$lib/types/mcp';

	interface Props {
		status: McpConnectionStatus;
		onRefresh?: () => void;
	}

	let { status, onRefresh = () => {} }: Props = $props();
</script>

<div class="flex items-center gap-3">
	<!-- Connection status indicator -->
	<div class="flex items-center gap-2">
		<span
			class="inline-flex h-2 w-2 rounded-full"
			class:bg-green-500={status.connected}
			class:bg-red-500={!status.connected}
			aria-label={status.connected ? 'Connected' : 'Disconnected'}
		/>

		<!-- Status text -->
		<span class="text-sm text-gray-700">
			{#if status.connected}
				<span class="font-medium text-green-700">MCP Connected</span>
				{#if status.serverInfo}
					<span class="text-gray-500 ml-1">
						({status.serverInfo.name} v{status.serverInfo.version})
					</span>
				{/if}
			{:else}
				<span class="font-medium text-red-700">MCP Disconnected</span>
				{#if status.error}
					<span class="text-red-600 ml-1">- {status.error}</span>
				{/if}
			{/if}
		</span>
	</div>

	<!-- Refresh button -->
	<button
		onclick={onRefresh}
		class="text-xs px-2 py-1 text-blue-600 hover:text-blue-800 hover:bg-blue-50 rounded transition-colors"
		type="button"
	>
		Refresh
	</button>
</div>
