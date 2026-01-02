<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { FileCode, Search, RefreshCw, Eye, Download, AlertTriangle, FileJson } from 'lucide-svelte';
	import type { AggregatedSchemaResponse, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Badge from '$lib/components/Badge.svelte';
	import SchemaExportModal from '$lib/components/learning/SchemaExportModal.svelte';
	import { canReadSchemas } from '$lib/utils/permissions';

	let isLoading = $state(true);
	let showExportModal = $state(false);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let methodFilter = $state('');
	let currentTeam = $state<string>('');
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	// Data
	let schemas = $state<AggregatedSchemaResponse[]>([]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
		} catch (e) {
			console.error('Failed to load session info:', e);
		}
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const query: { path?: string; httpMethod?: string } = {};
			if (searchQuery) query.path = searchQuery;
			if (methodFilter) query.httpMethod = methodFilter;

			schemas = await apiClient.listAggregatedSchemas(query);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load schemas';
			console.error('Failed to load schemas:', e);
		} finally {
			isLoading = false;
		}
	}

	// Filter schemas by search
	let filteredSchemas = $derived(
		schemas.filter(
			(schema) =>
				!searchQuery || schema.path.toLowerCase().includes(searchQuery.toLowerCase())
		)
	);

	// Calculate stats
	let stats = $derived.by(() => {
		const highConfidence = schemas.filter((s) => s.confidenceScore >= 0.9).length;
		const withBreakingChanges = schemas.filter(
			(s) => s.breakingChanges && s.breakingChanges.length > 0
		).length;
		return {
			total: schemas.length,
			highConfidence,
			withBreakingChanges
		};
	});

	// Navigate to details page
	function handleView(schemaId: number) {
		goto(`/learning/schemas/${schemaId}`);
	}

	// Export as OpenAPI
	async function handleExport(schema: AggregatedSchemaResponse) {
		// Permission check
		if (sessionInfo && !canReadSchemas(sessionInfo)) {
			error = "You don't have permission to export schemas. Contact your administrator.";
			return;
		}

		try {
			const openapi = await apiClient.exportSchemaAsOpenApi(schema.id, false);
			const blob = new Blob([JSON.stringify(openapi, null, 2)], { type: 'application/json' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${schema.path.replace(/\//g, '_')}_${schema.httpMethod}.openapi.json`;
			a.click();
			URL.revokeObjectURL(url);
		} catch (e) {
			console.error('Failed to export schema:', e);
		}
	}

	// Format confidence as percentage
	function formatConfidence(score: number): string {
		return `${(score * 100).toFixed(0)}%`;
	}

	// Get confidence color
	function getConfidenceVariant(score: number): 'green' | 'yellow' | 'red' {
		if (score >= 0.9) return 'green';
		if (score >= 0.7) return 'yellow';
		return 'red';
	}

	// HTTP method colors
	const methodColors: Record<string, 'blue' | 'green' | 'yellow' | 'red' | 'purple' | 'gray'> = {
		GET: 'blue',
		POST: 'green',
		PUT: 'yellow',
		DELETE: 'red',
		PATCH: 'purple',
		HEAD: 'gray',
		OPTIONS: 'gray'
	};

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	// Handle search
	function handleSearch() {
		loadData();
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Discovered Schemas</h1>
		<p class="mt-2 text-sm text-gray-600">
			API schemas discovered through learning sessions for the <span class="font-medium"
				>{currentTeam}</span
			> team
		</p>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Schemas</p>
					<p class="text-2xl font-bold text-gray-900">{stats.total}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<FileCode class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">High Confidence (90%+)</p>
					<p class="text-2xl font-bold text-green-600">{stats.highConfidence}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<FileCode class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">With Breaking Changes</p>
					<p class="text-2xl font-bold text-orange-600">{stats.withBreakingChanges}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<AlertTriangle class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Filters Row -->
	<div class="mb-6 flex flex-col sm:flex-row gap-4">
		<!-- Search -->
		<div class="relative flex-1">
			<Search class="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-gray-400" />
			<input
				type="text"
				bind:value={searchQuery}
				onkeydown={(e) => e.key === 'Enter' && handleSearch()}
				placeholder="Search by API path..."
				class="w-full pl-10 pr-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
			/>
		</div>

		<!-- Method Filter -->
		<select
			bind:value={methodFilter}
			onchange={loadData}
			class="px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 bg-white"
		>
			<option value="">All Methods</option>
			<option value="GET">GET</option>
			<option value="POST">POST</option>
			<option value="PUT">PUT</option>
			<option value="DELETE">DELETE</option>
			<option value="PATCH">PATCH</option>
		</select>

		<!-- Export Button -->
		{#if sessionInfo && canReadSchemas(sessionInfo)}
			<button
				onclick={() => (showExportModal = true)}
				disabled={filteredSchemas.length === 0}
				class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2 transition-colors"
			>
				<FileJson class="h-4 w-4" />
				Export as OpenAPI
			</button>
		{/if}
	</div>

	<!-- Error Message -->
	{#if error}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
			{error}
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex justify-center items-center py-12">
			<RefreshCw class="h-8 w-8 animate-spin text-gray-400" />
		</div>
	{:else if filteredSchemas.length === 0}
		<div class="text-center py-12 bg-white rounded-lg border border-gray-200">
			<FileCode class="h-12 w-12 mx-auto text-gray-400 mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">No schemas found</h3>
			<p class="text-gray-500">
				{searchQuery || methodFilter
					? 'No schemas match your filters.'
					: 'Schemas will appear here after learning sessions complete.'}
			</p>
		</div>
	{:else}
		<!-- Schemas Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Endpoint
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Method
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Confidence
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Samples
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Last Observed
						</th>
						<th
							class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredSchemas as schema}
						<tr class="hover:bg-gray-50">
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<code class="text-sm font-mono text-gray-900">{schema.path}</code>
									{#if schema.breakingChanges && schema.breakingChanges.length > 0}
										<span
											class="text-xs text-orange-600 mt-1 flex items-center gap-1"
										>
											<AlertTriangle class="h-3 w-3" />
											{schema.breakingChanges.length} breaking change(s)
										</span>
									{/if}
								</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<Badge variant={methodColors[schema.httpMethod] || 'gray'}>
									{schema.httpMethod}
								</Badge>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<Badge variant={getConfidenceVariant(schema.confidenceScore)}>
									{formatConfidence(schema.confidenceScore)}
								</Badge>
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
								{schema.sampleCount.toLocaleString()}
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
								{formatDate(schema.lastObserved)}
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleView(schema.id)}
										class="p-2 text-gray-500 hover:text-blue-600 hover:bg-blue-50 rounded-lg transition-colors"
										title="View details"
									>
										<Eye class="h-4 w-4" />
									</button>
									{#if sessionInfo && canReadSchemas(sessionInfo)}
										<button
											onclick={() => handleExport(schema)}
											class="p-2 text-gray-500 hover:text-green-600 hover:bg-green-50 rounded-lg transition-colors"
											title="Export as OpenAPI"
										>
											<Download class="h-4 w-4" />
										</button>
									{/if}
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<!-- Export Modal -->
<SchemaExportModal
	isOpen={showExportModal}
	schemas={filteredSchemas}
	onClose={() => (showExportModal = false)}
/>
