<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import type { McpConnectionInfo } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Cable, RefreshCw, CheckCircle2, Clock, AlertCircle, Loader2 } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import StatCard from '$lib/components/StatCard.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let currentTeam = $state<string>('');

	// Data
	let connections = $state<McpConnectionInfo[]>([]);

	// Auto-refresh interval
	let refreshInterval: ReturnType<typeof setInterval> | null = null;
	const REFRESH_INTERVAL_MS = 5000;

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadConnections();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		await loadConnections();
		// Start auto-refresh
		refreshInterval = setInterval(loadConnections, REFRESH_INTERVAL_MS);
	});

	onDestroy(() => {
		if (refreshInterval) {
			clearInterval(refreshInterval);
		}
	});

	async function loadConnections() {
		if (!currentTeam) return;

		// Don't show loading indicator for background refreshes
		const isInitialLoad = connections.length === 0;
		if (isInitialLoad) {
			isLoading = true;
		}
		error = null;

		try {
			const response = await apiClient.listMcpConnections(currentTeam);
			connections = response.connections;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load MCP connections';
		} finally {
			isLoading = false;
		}
	}

	// Derive connection status based on last activity
	function getConnectionStatus(conn: McpConnectionInfo): 'active' | 'idle' | 'initializing' {
		if (!conn.initialized) {
			return 'initializing';
		}
		const lastActivity = new Date(conn.lastActivity);
		const now = new Date();
		const diffSeconds = (now.getTime() - lastActivity.getTime()) / 1000;
		return diffSeconds < 30 ? 'active' : 'idle';
	}

	// Format relative time
	function formatRelativeTime(dateStr: string): string {
		const date = new Date(dateStr);
		const now = new Date();
		const diffSeconds = Math.floor((now.getTime() - date.getTime()) / 1000);

		if (diffSeconds < 60) return `${diffSeconds}s ago`;
		if (diffSeconds < 3600) return `${Math.floor(diffSeconds / 60)}m ago`;
		if (diffSeconds < 86400) return `${Math.floor(diffSeconds / 3600)}h ago`;
		return `${Math.floor(diffSeconds / 86400)}d ago`;
	}

	// Stats derived from connections
	let stats = $derived({
		total: connections.length,
		active: connections.filter((c) => getConnectionStatus(c) === 'active').length,
		idle: connections.filter((c) => getConnectionStatus(c) === 'idle').length,
		initializing: connections.filter((c) => getConnectionStatus(c) === 'initializing').length,
		sse: connections.filter((c) => c.connectionType === 'sse').length,
		http: connections.filter((c) => c.connectionType === 'http').length
	});
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<div class="flex items-center justify-between">
			<div class="flex items-center gap-3">
				<Cable class="h-8 w-8 text-blue-600" />
				<div>
					<h1 class="text-3xl font-bold text-gray-900">MCP Connections</h1>
					<p class="mt-1 text-sm text-gray-600">
						Active MCP connections and sessions for the <span class="font-medium">{currentTeam}</span>
						team
					</p>
				</div>
			</div>
			<div class="flex items-center gap-2 text-sm text-gray-500">
				<RefreshCw class="h-4 w-4 animate-spin" />
				<span>Auto-refreshing every 5s</span>
			</div>
		</div>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6 flex gap-3">
		<Button onclick={loadConnections} variant="secondary">
			<RefreshCw class="h-4 w-4 mr-2" />
			Refresh Now
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<StatCard title="Total Connections" value={stats.total} colorClass="blue" />
		<StatCard title="Active" value={stats.active} colorClass="green" />
		<StatCard title="Idle" value={stats.idle} colorClass="gray" />
		<StatCard title="Initializing" value={stats.initializing} colorClass="yellow" />
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading connections...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if connections.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Cable class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">No active connections</h3>
			<p class="text-sm text-gray-600 mb-6">
				MCP clients will appear here when they connect via SSE or HTTP.
			</p>
		</div>
	{:else}
		<!-- Connections Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Connection ID
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Type
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Client
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Protocol
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Status
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Created
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Last Activity
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Log Level
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each connections as conn (conn.connectionId)}
						{@const status = getConnectionStatus(conn)}
						<tr class="hover:bg-gray-50">
							<td class="px-6 py-4 whitespace-nowrap">
								<code class="text-sm font-mono text-gray-900">{conn.connectionId}</code>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span
									class="inline-flex px-2 py-1 rounded text-xs font-medium
									{conn.connectionType === 'sse'
										? 'bg-indigo-100 text-indigo-800'
										: 'bg-cyan-100 text-cyan-800'}"
								>
									{conn.connectionType.toUpperCase()}
								</span>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								{#if conn.clientInfo}
									<div class="text-sm">
										<div class="font-medium text-gray-900">{conn.clientInfo.name}</div>
										<div class="text-gray-500">v{conn.clientInfo.version}</div>
									</div>
								{:else}
									<span class="text-sm text-gray-400 italic">Not initialized</span>
								{/if}
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								{#if conn.protocolVersion}
									<span class="text-sm text-gray-900">{conn.protocolVersion}</span>
								{:else}
									<span class="text-sm text-gray-400">-</span>
								{/if}
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								{#if status === 'active'}
									<span
										class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium bg-green-100 text-green-800"
									>
										<CheckCircle2 class="h-3.5 w-3.5" />
										Active
									</span>
								{:else if status === 'idle'}
									<span
										class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium bg-gray-100 text-gray-800"
									>
										<Clock class="h-3.5 w-3.5" />
										Idle
									</span>
								{:else}
									<span
										class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium bg-yellow-100 text-yellow-800"
									>
										<Loader2 class="h-3.5 w-3.5 animate-spin" />
										Initializing
									</span>
								{/if}
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span class="text-sm text-gray-600" title={conn.createdAt}>
									{formatRelativeTime(conn.createdAt)}
								</span>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span class="text-sm text-gray-600" title={conn.lastActivity}>
									{formatRelativeTime(conn.lastActivity)}
								</span>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span
									class="inline-flex px-2 py-1 rounded text-xs font-medium
									{conn.logLevel === 'debug'
										? 'bg-purple-100 text-purple-800'
										: conn.logLevel === 'info'
											? 'bg-blue-100 text-blue-800'
											: conn.logLevel === 'warning'
												? 'bg-yellow-100 text-yellow-800'
												: 'bg-red-100 text-red-800'}"
								>
									{conn.logLevel}
								</span>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Result Count -->
		<div class="mt-4 text-center">
			<p class="text-sm text-gray-600">
				Showing {connections.length} connection{connections.length !== 1 ? 's' : ''}
			</p>
		</div>
	{/if}
</div>
