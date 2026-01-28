<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import { Bot, Search, RefreshCw, CheckCircle2, AlertCircle } from 'lucide-svelte';
	import type { McpTool, McpToolCategory, LearnedSchemaAvailability } from '$lib/api/types';
	import type { McpConnectionStatus as McpConnectionStatusType } from '$lib/types/mcp';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import StatCard from '$lib/components/StatCard.svelte';
	import { McpToolCard, ToolDetailModal, EditToolModal, ApplyLearnedSchemaModal, McpConnectionStatus } from '$lib/components/mcp';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let currentTeam = $state<string>('');

	// Data
	let tools = $state<McpTool[]>([]);

	// Connection status state
	let connectionStatus = $state<McpConnectionStatusType>({
		connected: false
	});

	// Filter state
	let searchQuery = $state('');
	let categoryFilter = $state<'all' | McpToolCategory>('all');
	let enabledFilter = $state<'all' | 'enabled' | 'disabled'>('all');

	// Modal state
	let selectedToolForModal = $state<McpTool | null>(null);
	let isDetailModalOpen = $state(false);
	let isEditModalOpen = $state(false);
	let isApplyLearnedModalOpen = $state(false);

	// Apply learned schema state
	let learnedSchemaAvailability = $state<LearnedSchemaAvailability | null>(null);
	let learnedSchemaAvailabilityMap = $state<Map<string, LearnedSchemaAvailability>>(new Map());

	// Helper to check if a tool has incomplete information
	function isToolIncomplete(tool: McpTool): boolean {
		return (
			!tool.description ||
			!tool.inputSchema ||
			(typeof tool.inputSchema === 'object' && !Object.keys(tool.inputSchema).length) ||
			!tool.outputSchema ||
			(typeof tool.outputSchema === 'object' && !Object.keys(tool.outputSchema).length)
		);
	}

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadTools();
			checkConnectionStatus();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		await loadTools();
		await checkConnectionStatus();
	});

	async function loadTools() {
		if (!currentTeam) return;

		isLoading = true;
		error = null;

		try {
			const response = await apiClient.listMcpTools(currentTeam);
			tools = response.tools;

			// Load learned schema availability for tools with routeIds
			await loadLearnedSchemaAvailability();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load MCP tools';
		} finally {
			isLoading = false;
		}
	}

	// Check MCP server connection status
	async function checkConnectionStatus() {
		if (!currentTeam) return;

		try {
			const pingResult = await apiClient.pingMcpServer(currentTeam);

			if (pingResult.success) {
				connectionStatus = {
					connected: true,
					serverInfo: {
						name: 'flowplane',
						version: '1.0.0'
					},
					protocolVersion: '2024-11-05',
					lastPing: new Date()
				};
			} else {
				connectionStatus = {
					connected: false,
					lastPing: new Date(),
					error: pingResult.error || 'Connection failed'
				};
			}
		} catch (e) {
			connectionStatus = {
				connected: false,
				lastPing: new Date(),
				error: e instanceof Error ? e.message : 'Failed to ping MCP server'
			};
		}
	}

	// Load learned schema availability for all tools with route IDs
	async function loadLearnedSchemaAvailability() {
		const newMap = new Map<string, LearnedSchemaAvailability>();

		// Check each tool that has a routeId
		const checks = tools
			.filter(tool => tool.routeId)
			.map(async (tool) => {
				try {
					const availability = await apiClient.checkLearnedSchema(currentTeam, tool.routeId!);
					if (availability.available) {
						newMap.set(tool.id, availability);
					}
				} catch (e) {
					// Silently ignore errors for individual checks
					console.error(`Failed to check learned schema for ${tool.name}:`, e);
				}
			});

		await Promise.all(checks);
		learnedSchemaAvailabilityMap = newMap;
	}

	// Filtered tools
	let filteredTools = $derived(
		tools.filter((tool) => {
			// Category filter
			if (categoryFilter !== 'all' && tool.category !== categoryFilter) return false;

			// Enabled filter
			if (enabledFilter === 'enabled' && !tool.enabled) return false;
			if (enabledFilter === 'disabled' && tool.enabled) return false;

			// Search filter
			if (searchQuery) {
				const query = searchQuery.toLowerCase();
				const matchesName = tool.name.toLowerCase().includes(query);
				const matchesDescription = tool.description?.toLowerCase().includes(query);
				const matchesPath = tool.httpPath?.toLowerCase().includes(query);
				if (!matchesName && !matchesDescription && !matchesPath) return false;
			}

			return true;
		})
	);

	// Stats
	let stats = $derived({
		total: tools.length,
		enabled: tools.filter((t) => t.enabled).length,
		gatewayApi: tools.filter((t) => t.category === 'gateway_api').length,
		learned: tools.filter((t) => t.schemaSource === 'learned').length,
		incomplete: tools.filter((t) => isToolIncomplete(t)).length
	});

	// Toggle tool enabled state
	async function handleToggle(tool: McpTool) {
		// Built-in tools cannot be toggled
		if (tool.isBuiltin) return;

		try {
			await apiClient.updateMcpTool(currentTeam, tool.name, { enabled: !tool.enabled });
			await loadTools();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to update tool';
		}
	}

	// View tool details
	function handleViewTool(tool: McpTool) {
		selectedToolForModal = tool;
		isDetailModalOpen = true;
	}

	// Open edit modal from detail modal
	function handleEditTool(tool: McpTool) {
		// Built-in tools cannot be edited
		if (tool.isBuiltin) return;

		selectedToolForModal = tool;
		isDetailModalOpen = false;
		isEditModalOpen = true;
	}

	// Open apply learned schema modal
	async function handleApplyLearnedSchema(tool: McpTool) {
		if (!tool.routeId) return;

		selectedToolForModal = tool;
		isDetailModalOpen = false;

		try {
			// Fetch fresh availability data
			learnedSchemaAvailability = await apiClient.checkLearnedSchema(currentTeam, tool.routeId);
			isApplyLearnedModalOpen = true;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to check learned schema availability';
		}
	}

	// Confirm apply learned schema
	async function handleConfirmApplyLearned(force: boolean) {
		if (!selectedToolForModal?.routeId) return;

		try {
			await apiClient.applyLearnedSchema(currentTeam, selectedToolForModal.routeId, force);
			isApplyLearnedModalOpen = false;
			await loadTools(); // Reload to get updated tool
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to apply learned schema';
		}
	}

	// Save tool changes - only update editable metadata fields
	async function handleSaveTool(updatedTool: McpTool) {
		try {
			// Find the original tool to get its name for the API path
			const originalTool = tools.find((t) => t.id === updatedTool.id);
			if (!originalTool) {
				error = 'Tool not found';
				return;
			}

			// Only send editable metadata fields - route definition fields are read-only
			await apiClient.updateMcpTool(currentTeam, originalTool.name, {
				name: updatedTool.name,
				description: updatedTool.description || undefined,
				inputSchema: updatedTool.inputSchema,
				outputSchema: updatedTool.outputSchema,
				enabled: updatedTool.enabled
				// Note: httpMethod, httpPath, and category are not sent - they are read-only
			});
			await loadTools();
			// Update selectedToolForModal with fresh data
			const refreshedTool = tools.find((t) => t.id === updatedTool.id);
			if (refreshedTool) {
				selectedToolForModal = refreshedTool;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to update tool';
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<div class="flex items-center justify-between">
			<div class="flex items-center gap-3">
				<Bot class="h-8 w-8 text-blue-600" />
				<div>
					<h1 class="text-3xl font-bold text-gray-900">MCP Tools</h1>
					<p class="mt-1 text-sm text-gray-600">
						Manage Model Context Protocol tools for the <span class="font-medium">{currentTeam}</span> team
					</p>
				</div>
			</div>
			<McpConnectionStatus status={connectionStatus} onRefresh={checkConnectionStatus} />
		</div>
		<!-- Inline stats badges -->
		<div class="mt-4 flex items-center gap-4 text-sm flex-wrap">
			<div class="flex items-center gap-2">
				<CheckCircle2 class="w-4 h-4 text-green-600" />
				<span class="text-gray-700">{stats.enabled} tools enabled</span>
			</div>
			{#if stats.incomplete > 0}
				<div class="flex items-center gap-2">
					<AlertCircle class="w-4 h-4 text-amber-600" />
					<span class="text-gray-700">{stats.incomplete} tools with incomplete info</span>
				</div>
			{/if}
		</div>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6 flex gap-3">
		<Button onclick={loadTools} variant="secondary">
			<RefreshCw class="h-4 w-4 mr-2" />
			Refresh
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<StatCard title="Total Tools" value={stats.total} colorClass="blue" />
		<StatCard title="Enabled" value={stats.enabled} colorClass="green" />
		<StatCard title="Gateway APIs" value={stats.gatewayApi} colorClass="purple" />
		<StatCard title="Learned Schemas" value={stats.learned} colorClass="orange" />
	</div>

	<!-- Filters -->
	<div class="flex flex-col sm:flex-row gap-4 mb-6">
		<!-- Search -->
		<div class="relative flex-1">
			<Search class="absolute left-3 top-1/2 -translate-y-1/2 h-5 w-5 text-gray-400" />
			<input
				type="text"
				bind:value={searchQuery}
				placeholder="Search tools, routes, or descriptions..."
				class="w-full pl-10 pr-4 py-2.5 border border-gray-300 rounded-lg bg-white text-gray-900 placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
			/>
		</div>

		<!-- Category Filter (button tabs) -->
		<div class="flex gap-2 flex-wrap">
			<button
				onclick={() => (categoryFilter = 'all')}
				class="px-4 py-2.5 rounded-lg transition-colors {categoryFilter === 'all'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 border border-gray-300 hover:bg-gray-50'}"
			>
				All
			</button>
			<button
				onclick={() => (categoryFilter = 'control_plane')}
				class="px-4 py-2.5 rounded-lg transition-colors {categoryFilter === 'control_plane'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 border border-gray-300 hover:bg-gray-50'}"
			>
				Control Plane
			</button>
			<button
				onclick={() => (categoryFilter = 'gateway_api')}
				class="px-4 py-2.5 rounded-lg transition-colors {categoryFilter === 'gateway_api'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 border border-gray-300 hover:bg-gray-50'}"
			>
				Gateway API
			</button>
		</div>

		<!-- Enabled Filter -->
		<select
			bind:value={enabledFilter}
			class="px-4 py-2.5 border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 bg-white"
		>
			<option value="all">All Status</option>
			<option value="enabled">Enabled Only</option>
			<option value="disabled">Disabled Only</option>
		</select>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading tools...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredTools.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Bot class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery || categoryFilter !== 'all' || enabledFilter !== 'all'
					? 'No tools found'
					: 'No MCP tools yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery || categoryFilter !== 'all' || enabledFilter !== 'all'
					? 'Try adjusting your filters'
					: 'Enable MCP on routes to create tools that AI assistants can use'}
			</p>
		</div>
	{:else}
		<!-- Tools Grid -->
		<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
			{#each filteredTools as tool (tool.id)}
				<McpToolCard
					{tool}
					onToggle={() => handleToggle(tool)}
					onView={() => handleViewTool(tool)}
					hasLearnedSchemaAvailable={learnedSchemaAvailabilityMap.has(tool.id)}
				/>
			{/each}
		</div>

		<!-- Result Count -->
		{#if filteredTools.length !== tools.length}
			<div class="mt-4 text-center">
				<p class="text-sm text-gray-600">
					Showing {filteredTools.length} of {tools.length} tools
				</p>
			</div>
		{/if}
	{/if}
</div>

<!-- Tool Detail Modal -->
<ToolDetailModal
	show={isDetailModalOpen}
	tool={selectedToolForModal}
	onClose={() => (isDetailModalOpen = false)}
	onEdit={handleEditTool}
	onApplyLearned={selectedToolForModal && learnedSchemaAvailabilityMap.has(selectedToolForModal.id)
		? () => handleApplyLearnedSchema(selectedToolForModal!)
		: undefined}
	hasLearnedSchemaAvailable={selectedToolForModal ? learnedSchemaAvailabilityMap.has(selectedToolForModal.id) : false}
/>

<!-- Edit Tool Modal -->
<EditToolModal
	show={isEditModalOpen}
	tool={selectedToolForModal}
	onClose={() => (isEditModalOpen = false)}
	onSave={handleSaveTool}
/>

<!-- Apply Learned Schema Modal -->
<ApplyLearnedSchemaModal
	show={isApplyLearnedModalOpen}
	tool={selectedToolForModal}
	learnedSchema={learnedSchemaAvailability?.schema || null}
	currentSource={learnedSchemaAvailability?.currentSource || 'manual'}
	requiresForce={learnedSchemaAvailability?.requiresForce || false}
	onConfirm={handleConfirmApplyLearned}
	onCancel={() => (isApplyLearnedModalOpen = false)}
/>
