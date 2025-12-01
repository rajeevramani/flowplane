<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Server, Activity, Shield, Heart } from 'lucide-svelte';
	import type { ClusterResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let clusters = $state<ClusterResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

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
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [clustersData, importsData] = await Promise.all([
				apiClient.listClusters(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			clusters = clustersData;
			imports = importsData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Extract endpoints from cluster config (handles both simple and xDS format)
	function extractEndpoints(config: Record<string, unknown>): { host: string; port: number }[] {
		// Simple format: endpoints directly on config
		if (Array.isArray(config.endpoints)) {
			const endpoints = config.endpoints as { host?: string; port?: number }[];
			return endpoints
				.filter(ep => ep.host && ep.host !== '')
				.map(ep => ({
					host: ep.host as string,
					port: ep.port || 8080
				}));
		}

		// xDS format: nested in loadAssignment
		const loadAssignment = (config.loadAssignment || config.load_assignment) as Record<string, unknown> | undefined;
		if (!loadAssignment) return [];

		const localityEndpoints = (loadAssignment.endpoints || loadAssignment.locality_lb_endpoints) as Record<string, unknown>[] | undefined;
		if (!localityEndpoints || localityEndpoints.length === 0) return [];

		const lbEndpoints = (localityEndpoints[0].lbEndpoints || localityEndpoints[0].lb_endpoints) as Record<string, unknown>[] | undefined;
		if (!lbEndpoints) return [];

		return lbEndpoints.map((ep) => {
			const endpoint = ep.endpoint as Record<string, unknown> | undefined;
			const address = endpoint?.address as Record<string, unknown> | undefined;
			const socketAddress = (address?.socketAddress || address?.socket_address) as Record<string, unknown> | undefined;

			return {
				host: (socketAddress?.address || '') as string,
				port: (socketAddress?.portValue || socketAddress?.port_value || 8080) as number
			};
		}).filter(ep => ep.host !== '');
	}

	// Calculate stats
	let stats = $derived({
		totalClusters: clusters.length,
		totalEndpoints: clusters.reduce((sum, cluster) => {
			const config = cluster.config || {};
			return sum + extractEndpoints(config).length;
		}, 0),
		activeHealthChecks: clusters.filter(c => {
			const config = c.config || {};
			return Boolean(config.healthChecks?.length);
		}).length,
		activeCircuitBreakers: clusters.filter(c => {
			const config = c.config || {};
			return Boolean(config.circuitBreakers);
		}).length
	});

	// Filter clusters
	let filteredClusters = $derived(
		searchQuery
			? clusters.filter(cluster =>
				cluster.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				cluster.serviceName.toLowerCase().includes(searchQuery.toLowerCase()) ||
				cluster.team.toLowerCase().includes(searchQuery.toLowerCase())
			)
			: clusters
	);

	// Get cluster features
	function getClusterFeatures(cluster: ClusterResponse) {
		const config = cluster.config || {};
		return {
			hasHealthCheck: Boolean(config.healthChecks?.length),
			hasCircuitBreaker: Boolean(config.circuitBreakers),
			hasOutlierDetection: Boolean(config.outlierDetection)
		};
	}

	// Get source type
	function getSourceType(cluster: ClusterResponse): { type: string; name?: string } {
		if (cluster.importId) {
			const importRecord = imports.find(i => i.id === cluster.importId);
			return {
				type: 'import',
				name: importRecord?.specName || 'OpenAPI Import'
			};
		}
		return { type: 'manual', name: 'Manual' };
	}

	// Format LB policy
	function formatLbPolicy(policy: string): string {
		return policy.replace(/_/g, ' ').toLowerCase().replace(/\b\w/g, (c) => c.toUpperCase());
	}

	// Navigate to create page
	function handleCreate() {
		goto('/clusters/create');
	}

	// Navigate to edit page
	function handleEdit(clusterName: string) {
		goto(`/clusters/${encodeURIComponent(clusterName)}/edit`);
	}

	// Delete cluster
	async function handleDelete(cluster: ClusterResponse) {
		if (!confirm(`Are you sure you want to delete the cluster "${cluster.name}"? This action cannot be undone.`)) {
			return;
		}

		try {
			await apiClient.deleteCluster(cluster.name);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete cluster';
		}
	}
</script>

<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Clusters</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage backend service clusters for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Cluster
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Clusters</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalClusters}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Endpoints</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalEndpoints}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Activity class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Active Health Checks</p>
					<p class="text-2xl font-bold text-gray-900">{stats.activeHealthChecks}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Heart class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Active Circuit Breakers</p>
					<p class="text-2xl font-bold text-gray-900">{stats.activeCircuitBreakers}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Shield class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name or service..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading clusters...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredClusters.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Server class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No clusters found' : 'No clusters yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Get started by creating a new cluster'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Cluster
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Service Name
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Endpoints
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							LB Policy
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Features
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Source
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredClusters as cluster}
						{@const config = cluster.config || {}}
						{@const endpoints = extractEndpoints(config)}
						{@const features = getClusterFeatures(cluster)}
						{@const source = getSourceType(cluster)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Service Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{cluster.serviceName}</span>
									<span class="text-xs text-gray-500 font-mono">{cluster.name}</span>
								</div>
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{cluster.team}</Badge>
							</td>

							<!-- Endpoints -->
							<td class="px-6 py-4">
								<div class="flex flex-col gap-1">
									<span class="text-sm text-gray-900">{endpoints.length} endpoint{endpoints.length !== 1 ? 's' : ''}</span>
									{#if endpoints.length > 0}
										<span class="text-xs text-gray-500 font-mono">{endpoints[0].host}:{endpoints[0].port}</span>
										{#if endpoints.length > 1}
											<span class="text-xs text-gray-400">+{endpoints.length - 1} more</span>
										{/if}
									{/if}
								</div>
							</td>

							<!-- LB Policy -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">{formatLbPolicy(config.lbPolicy || 'ROUND_ROBIN')}</span>
							</td>

							<!-- Features -->
							<td class="px-6 py-4">
								<div class="flex flex-wrap gap-1">
									{#if features.hasHealthCheck}
										<Badge variant="green" size="sm">HC</Badge>
									{/if}
									{#if features.hasCircuitBreaker}
										<Badge variant="yellow" size="sm">CB</Badge>
									{/if}
									{#if features.hasOutlierDetection}
										<Badge variant="orange" size="sm">OD</Badge>
									{/if}
									{#if !features.hasHealthCheck && !features.hasCircuitBreaker && !features.hasOutlierDetection}
										<span class="text-xs text-gray-400">None</span>
									{/if}
								</div>
							</td>

							<!-- Source -->
							<td class="px-6 py-4">
								{#if source.type === 'import'}
									<Badge variant="purple">{source.name}</Badge>
								{:else}
									<span class="text-sm text-gray-500">{source.name}</span>
								{/if}
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleEdit(cluster.name)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit cluster"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(cluster)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete cluster"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination Placeholder -->
		{#if filteredClusters.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredClusters.length} clusters</p>
			</div>
		{/if}
	{/if}
</div>
