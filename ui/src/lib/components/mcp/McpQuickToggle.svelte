<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import type { RouteListViewDto } from '$lib/types/route-view';

	interface Props {
		/** The route to toggle MCP for */
		route: RouteListViewDto;
		/** Current team name for API calls */
		team: string;
		/** Callback when toggle action completes (success or failure) */
		onToggle?: () => void;
		/** Callback to open the full MCP enable modal (for routes without metadata) */
		onEnableMcp?: (routeId: string, path: string) => void;
	}

	let { route, team, onToggle, onEnableMcp }: Props = $props();

	let isLoading = $state(false);
	let error = $state<string | null>(null);

	async function handleToggle() {
		if (route.mcpEnabled) {
			// Disable MCP
			isLoading = true;
			error = null;
			try {
				await apiClient.disableMcp(team, route.routeId);
				onToggle?.();
			} catch (e) {
				error = e instanceof Error ? e.message : 'Failed to disable MCP';
				console.error('Failed to disable MCP:', e);
			} finally {
				isLoading = false;
			}
		} else {
			// Check if route has required metadata for quick enable
			if (route.operationId && route.summary) {
				// Quick enable with defaults
				isLoading = true;
				error = null;
				try {
					await apiClient.enableMcp(team, route.routeId, {});
					onToggle?.();
				} catch (e) {
					// If quick enable fails, fall back to modal
					if (onEnableMcp) {
						onEnableMcp(route.routeId, route.pathPattern);
					} else {
						error = e instanceof Error ? e.message : 'Failed to enable MCP';
						console.error('Failed to enable MCP:', e);
					}
				} finally {
					isLoading = false;
				}
			} else {
				// No metadata - need to open modal for user to provide details
				if (onEnableMcp) {
					onEnableMcp(route.routeId, route.pathPattern);
				}
			}
		}
	}
</script>

<button
	onclick={handleToggle}
	disabled={isLoading}
	class="mcp-toggle px-3 py-1 text-sm font-medium rounded-full transition-all duration-200 ease-in-out
		{route.mcpEnabled
			? 'bg-emerald-100 text-emerald-700 border border-emerald-300 hover:bg-emerald-200'
			: 'bg-gray-100 text-gray-600 border border-gray-300 hover:bg-gray-200'}
		{isLoading ? 'opacity-50 cursor-not-allowed' : 'hover:scale-[1.02]'}
		focus:outline-none focus:ring-2 focus:ring-offset-1
		{route.mcpEnabled ? 'focus:ring-emerald-500' : 'focus:ring-gray-500'}"
	title={route.mcpEnabled
		? `Disable MCP for ${route.routeName}`
		: route.operationId
			? `Enable MCP for ${route.routeName}`
			: 'Enable MCP (requires additional info)'}
>
	{#if isLoading}
		<span class="inline-flex items-center gap-1">
			<svg class="animate-spin h-3 w-3" viewBox="0 0 24 24" fill="none">
				<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"
				></circle>
				<path
					class="opacity-75"
					fill="currentColor"
					d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
				></path>
			</svg>
			<span>...</span>
		</span>
	{:else if route.mcpEnabled}
		<span class="inline-flex items-center gap-1">
			<svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
			</svg>
			Enabled
		</span>
	{:else}
		<span>Enable</span>
	{/if}
</button>

{#if error}
	<span class="text-xs text-red-600 ml-2" title={error}>Error</span>
{/if}
