<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { ArrowLeft, Trash2, FileText, Database, Server, Route as RouteIcon } from 'lucide-svelte';
	import type { ImportDetailsResponse, RouteResponse, ClusterResponse, ListenerResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';

	let activeTab = $state<'details' | 'json'>('details');
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let importId = $state('');

	// Data
	let importDetails = $state<ImportDetailsResponse | null>(null);
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);

	// Get import ID from URL
	$effect(() => {
		const id = $page.params.id;
		if (id && id !== importId) {
			importId = id;
			loadData();
		}
	});

	onMount(async () => {
		importId = $page.params.id ?? '';
		await loadData();
	});

	async function loadData() {
		if (!importId) return;

		isLoading = true;
		error = null;

		try {
			const [importData, routesData, clustersData, listenersData] = await Promise.all([
				apiClient.getImport(importId),
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners()
			]);

			importDetails = importData;
			routes = routesData.filter(r => r.importId === importId);
			clusters = clustersData.filter(c => c.importId === importId);
			listeners = listenersData.filter(l => l.importId === importId);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load import details';
		} finally {
			isLoading = false;
		}
	}

	// Navigate back to imports list
	function handleBack() {
		goto('/imports');
	}

	// Delete import
	async function handleDelete() {
		if (!importDetails) return;

		if (!confirm(`Are you sure you want to delete the import "${importDetails.specName}"? This will also delete all associated routes, clusters, and listeners.`)) {
			return;
		}

		try {
			await apiClient.deleteImport(importId);
			goto('/imports');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete import';
		}
	}

	// Format date
	function formatDate(dateStr: string): string {
		if (!dateStr) return 'N/A';
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Derived JSON for preview
	let jsonPayload = $derived(
		importDetails ? JSON.stringify({
			id: importDetails.id,
			specName: importDetails.specName,
			specVersion: importDetails.specVersion,
			specChecksum: importDetails.specChecksum,
			team: importDetails.team,
			listenerName: importDetails.listenerName,
			importedAt: importDetails.importedAt,
			updatedAt: importDetails.updatedAt,
			routeCount: importDetails.routeCount,
			clusterCount: importDetails.clusterCount,
			listenerCount: importDetails.listenerCount,
			routes: routes.map(r => ({ name: r.name, pathPrefix: r.pathPrefix, team: r.team })),
			clusters: clusters.map(c => ({ name: c.name, serviceName: c.serviceName, team: c.team })),
			listeners: listeners.map(l => ({ name: l.name, address: l.address, port: l.port, team: l.team }))
		}, null, 2) : ''
	);
</script>

<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-6">
		<button
			onclick={handleBack}
			class="inline-flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Imports
		</button>

		{#if importDetails}
			<div class="flex items-start justify-between">
				<div>
					<h1 class="text-3xl font-bold text-gray-900">{importDetails.specName}</h1>
					<p class="mt-2 text-sm text-gray-600">
						{#if importDetails.specVersion}
							<span class="font-medium">Version {importDetails.specVersion}</span>
							<span class="mx-2">•</span>
						{/if}
						<span>Team: <Badge variant="indigo">{importDetails.team}</Badge></span>
						<span class="mx-2">•</span>
						<span>Import ID: <code class="text-xs bg-gray-100 px-2 py-0.5 rounded">{importDetails.id}</code></span>
					</p>
				</div>
				<Button variant="danger" onclick={handleDelete}>
					<Trash2 class="h-4 w-4 mr-2" />
					Delete Import
				</Button>
			</div>
		{/if}
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading import details...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if importDetails}
		<!-- Tabs -->
		<div class="border-b border-gray-200 mb-6">
			<nav class="-mb-px flex space-x-8">
				<button
					onclick={() => activeTab = 'details'}
					class="py-4 px-1 border-b-2 font-medium text-sm transition-colors {activeTab === 'details'
						? 'border-blue-500 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
				>
					Details
				</button>
				<button
					onclick={() => activeTab = 'json'}
					class="py-4 px-1 border-b-2 font-medium text-sm transition-colors {activeTab === 'json'
						? 'border-blue-500 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
				>
					JSON Preview
				</button>
			</nav>
		</div>

		<!-- Tab Content -->
		{#if activeTab === 'details'}
			<div class="space-y-6">
				<!-- Overview Section -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Overview</h2>
					<dl class="grid grid-cols-1 md:grid-cols-2 gap-6">
						<div>
							<dt class="text-sm font-medium text-gray-500">Import ID</dt>
							<dd class="mt-1 text-sm font-mono text-gray-900 bg-gray-50 px-3 py-2 rounded">{importDetails.id}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Spec Checksum</dt>
							<dd class="mt-1 text-sm font-mono text-gray-900 bg-gray-50 px-3 py-2 rounded truncate" title={importDetails.specChecksum || 'N/A'}>
								{importDetails.specChecksum || 'N/A'}
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Listener</dt>
							<dd class="mt-1 text-sm text-gray-900">
								{#if importDetails.listenerName}
									<Badge variant="blue">{importDetails.listenerName}</Badge>
								{:else}
									<span class="text-gray-400">None</span>
								{/if}
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Team</dt>
							<dd class="mt-1 text-sm text-gray-900">
								<Badge variant="indigo">{importDetails.team}</Badge>
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Imported</dt>
							<dd class="mt-1 text-sm text-gray-900">{formatDate(importDetails.importedAt)}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Last Updated</dt>
							<dd class="mt-1 text-sm text-gray-900">{formatDate(importDetails.updatedAt)}</dd>
						</div>
					</dl>
				</div>

				<!-- Resource Counts -->
				<div class="grid grid-cols-1 md:grid-cols-3 gap-6">
					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<div class="flex items-center justify-between">
							<div>
								<p class="text-sm font-medium text-gray-600">Routes Created</p>
								<p class="mt-2 text-3xl font-bold text-blue-600">{importDetails.routeCount}</p>
							</div>
							<div class="p-3 bg-blue-100 rounded-lg">
								<RouteIcon class="h-8 w-8 text-blue-600" />
							</div>
						</div>
					</div>

					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<div class="flex items-center justify-between">
							<div>
								<p class="text-sm font-medium text-gray-600">Clusters Created</p>
								<p class="mt-2 text-3xl font-bold text-green-600">{importDetails.clusterCount}</p>
							</div>
							<div class="p-3 bg-green-100 rounded-lg">
								<Database class="h-8 w-8 text-green-600" />
							</div>
						</div>
					</div>

					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<div class="flex items-center justify-between">
							<div>
								<p class="text-sm font-medium text-gray-600">Listeners Used</p>
								<p class="mt-2 text-3xl font-bold text-purple-600">{importDetails.listenerCount}</p>
							</div>
							<div class="p-3 bg-purple-100 rounded-lg">
								<Server class="h-8 w-8 text-purple-600" />
							</div>
						</div>
					</div>
				</div>

				<!-- Routes List -->
				{#if routes.length > 0}
					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
							<RouteIcon class="h-5 w-5" />
							Route Configurations ({routes.length})
						</h2>
						<div class="space-y-4">
							{#each routes as routeConfig}
								<div class="border border-gray-200 rounded-lg">
									<!-- Route Config Header -->
									<div class="p-4 bg-gray-50 border-b border-gray-200">
										<div class="flex items-start justify-between">
											<div class="flex-1">
												<h3 class="text-sm font-semibold text-gray-900">{routeConfig.name}</h3>
												{#if routeConfig.pathPrefix}
													<p class="mt-1 text-xs font-mono text-gray-600">{routeConfig.pathPrefix}</p>
												{/if}
											</div>
											<Badge variant="indigo">{routeConfig.team}</Badge>
										</div>
									</div>

									<!-- Virtual Hosts and Routes -->
									{#if routeConfig.config?.virtualHosts}
										<div class="divide-y divide-gray-200">
											{#each routeConfig.config.virtualHosts as vhost, vhostIdx}
												<div class="p-4">
													<!-- Virtual Host Header -->
													<div class="mb-3">
														<div class="flex items-center justify-between">
															<h4 class="text-sm font-medium text-gray-900">
																Virtual Host: {vhost.name}
															</h4>
															{#if vhost.domains && vhost.domains.length > 0}
																<div class="flex flex-wrap gap-1">
																	{#each vhost.domains as domain}
																		<Badge variant="blue" size="sm">{domain}</Badge>
																	{/each}
																</div>
															{/if}
														</div>
													</div>

													<!-- Individual Routes -->
													{#if vhost.routes && vhost.routes.length > 0}
														<div class="space-y-3 mt-3">
															<p class="text-xs font-medium text-gray-500 uppercase">Routes ({vhost.routes.length})</p>
															{#each vhost.routes as individualRoute, routeIdx}
																<div class="bg-white border border-gray-200 rounded-md p-3">
																	<!-- Route Match -->
																	<div class="mb-2">
																		<span class="text-xs font-medium text-gray-700">Match:</span>
																		<div class="mt-1 space-y-1">
																			{#if individualRoute.match?.prefix}
																				<div class="text-xs bg-blue-50 text-blue-700 px-2 py-1 rounded font-mono">
																					Prefix: {individualRoute.match.prefix}
																				</div>
																			{/if}
															{#if individualRoute.match?.path}
																{@const pathMatch = individualRoute.match.path}
																{@const pathValue = pathMatch.template || pathMatch.value || ''}
																{@const pathType = pathMatch.type || 'exact'}
																<div class="text-xs bg-blue-50 text-blue-700 px-2 py-1 rounded font-mono">
																	Path ({pathType}): {pathValue}
																</div>
															{/if}
																			{#if individualRoute.match?.regex}
																				<div class="text-xs bg-purple-50 text-purple-700 px-2 py-1 rounded font-mono">
																					Regex: {individualRoute.match.regex}
																				</div>
																			{/if}
																			{#if individualRoute.match?.headers}
																				{#each individualRoute.match.headers as header}
																					<div class="text-xs bg-gray-50 text-gray-700 px-2 py-1 rounded">
																						Header: <span class="font-mono">{header.name}: {header.value || header.exactMatch || header.prefixMatch || header.regexMatch || '*'}</span>
																					</div>
																				{/each}
																			{/if}
																		</div>
																	</div>

																	<!-- Route Action -->
																	{#if individualRoute.route}
																		<div class="mb-2">
																			<span class="text-xs font-medium text-gray-700">Action:</span>
																			<div class="mt-1 space-y-1">
																				{#if individualRoute.route.cluster}
																					<div class="text-xs bg-green-50 text-green-700 px-2 py-1 rounded">
																						Target Cluster: <span class="font-mono font-medium">{individualRoute.route.cluster}</span>
																					</div>
																				{/if}
																				{#if individualRoute.route.weightedClusters}
																					<div class="text-xs bg-green-50 text-green-700 px-2 py-1 rounded">
																						<div class="font-medium mb-1">Weighted Clusters:</div>
																						{#each individualRoute.route.weightedClusters.clusters as wc}
																							<div class="ml-2">
																								• {wc.name} ({wc.weight}%)
																							</div>
																						{/each}
																					</div>
																				{/if}
																				{#if individualRoute.route.timeout}
																					<div class="text-xs text-gray-600">
																						Timeout: {individualRoute.route.timeout}
																					</div>
																				{/if}
																			</div>
																		</div>
																	{/if}

																	<!-- Route Filters -->
																	{#if individualRoute.requestHeadersToAdd || individualRoute.responseHeadersToAdd || individualRoute.requestHeadersToRemove}
																		<div class="mb-2">
																			<span class="text-xs font-medium text-gray-700">Filters:</span>
																			<div class="mt-1 space-y-1">
																				{#if individualRoute.requestHeadersToAdd}
																					{#each individualRoute.requestHeadersToAdd as header}
																						<div class="text-xs bg-orange-50 text-orange-700 px-2 py-1 rounded">
																							Add Request Header: <span class="font-mono">{header.header?.key}: {header.header?.value}</span>
																						</div>
																					{/each}
																				{/if}
																				{#if individualRoute.responseHeadersToAdd}
																					{#each individualRoute.responseHeadersToAdd as header}
																						<div class="text-xs bg-orange-50 text-orange-700 px-2 py-1 rounded">
																							Add Response Header: <span class="font-mono">{header.header?.key}: {header.header?.value}</span>
																						</div>
																					{/each}
																				{/if}
																				{#if individualRoute.requestHeadersToRemove}
																					{#each individualRoute.requestHeadersToRemove as headerName}
																						<div class="text-xs bg-red-50 text-red-700 px-2 py-1 rounded">
																							Remove Request Header: <span class="font-mono">{headerName}</span>
																						</div>
																					{/each}
																				{/if}
																			</div>
																		</div>
																	{/if}

																	<!-- Route Metadata -->
																	{#if individualRoute.name}
																		<div class="text-xs text-gray-500 mt-2">
																			Route Name: <span class="font-mono">{individualRoute.name}</span>
																		</div>
																	{/if}
																</div>
															{/each}
														</div>
													{/if}
												</div>
											{/each}
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Clusters List -->
				{#if clusters.length > 0}
					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
							<Database class="h-5 w-5" />
							Clusters ({clusters.length})
						</h2>
						<div class="border border-gray-200 rounded-lg divide-y divide-gray-200">
							{#each clusters as cluster}
								{@const config = cluster.config || {}}
								<div class="p-4 hover:bg-gray-50 transition-colors">
									<div class="flex items-start justify-between mb-2">
										<div class="flex-1">
											<h3 class="text-sm font-medium text-gray-900">{cluster.serviceName}</h3>
											<p class="mt-1 text-xs font-mono text-gray-500">{cluster.name}</p>
										</div>
										<Badge variant="indigo">{cluster.team}</Badge>
									</div>

									{#if config.loadAssignment?.endpoints}
										<div class="mt-2">
											<span class="text-xs text-gray-500 font-medium">Endpoints:</span>
											<div class="mt-1 flex flex-wrap gap-2">
												{#each config.loadAssignment.endpoints as endpoint}
													{#if endpoint.lbEndpoints}
														{#each endpoint.lbEndpoints as lbEndpoint}
															{#if lbEndpoint.endpoint?.address?.socketAddress}
																<span class="inline-block px-2 py-1 bg-blue-50 text-blue-700 rounded text-xs font-mono">
																	{lbEndpoint.endpoint.address.socketAddress.address}:{lbEndpoint.endpoint.address.socketAddress.portValue}
																</span>
															{/if}
														{/each}
													{/if}
												{/each}
											</div>
										</div>
									{/if}

									<div class="mt-2 grid grid-cols-2 gap-4 text-xs">
										{#if config.lbPolicy}
											<div>
												<span class="text-gray-500">LB Policy:</span>
												<span class="ml-1 font-medium text-gray-900">{config.lbPolicy}</span>
											</div>
										{/if}
										{#if config.connectTimeout}
											<div>
												<span class="text-gray-500">Connect Timeout:</span>
												<span class="ml-1 font-medium text-gray-900">{config.connectTimeout}</span>
											</div>
										{/if}
									</div>

									{#if config.healthChecks && config.healthChecks.length > 0}
										<div class="mt-2">
											<Badge variant="green" size="sm">Health Checks Enabled</Badge>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Listeners List -->
				{#if listeners.length > 0}
					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
							<Server class="h-5 w-5" />
							Listeners ({listeners.length})
						</h2>
						<div class="border border-gray-200 rounded-lg divide-y divide-gray-200">
							{#each listeners as listener}
								{@const config = listener.config || {}}
								{@const hasTls = config.filter_chains?.some((fc: any) => fc.tls_context)}
								<div class="p-4 hover:bg-gray-50 transition-colors">
									<div class="flex items-start justify-between mb-2">
										<div class="flex-1">
											<h3 class="text-sm font-medium text-gray-900">{listener.name}</h3>
											<p class="mt-1 text-xs font-mono text-gray-500">
												{listener.address}:{listener.port}
											</p>
										</div>
										<Badge variant="indigo">{listener.team}</Badge>
									</div>

									<div class="mt-2 flex flex-wrap gap-2">
										<Badge variant="blue" size="sm">{listener.protocol || 'HTTP'}</Badge>
										{#if hasTls}
											<Badge variant="green" size="sm">TLS Enabled</Badge>
										{/if}
										{#if config.filter_chains && config.filter_chains.length > 0}
											<Badge variant="gray" size="sm">{config.filter_chains.length} Filter Chain(s)</Badge>
										{/if}
									</div>

									{#if config.filter_chains && config.filter_chains.length > 0}
										<div class="mt-2">
											<span class="text-xs text-gray-500">Route Configs:</span>
											<div class="mt-1 flex flex-wrap gap-2">
												{#each config.filter_chains as fc}
													{#if fc.filters}
														{#each fc.filters as filter}
															{#if filter.filter_type?.HttpConnectionManager?.route_config_name}
																<span class="inline-block px-2 py-1 bg-purple-50 text-purple-700 rounded text-xs font-mono">
																	{filter.filter_type.HttpConnectionManager.route_config_name}
																</span>
															{/if}
														{/each}
													{/if}
												{/each}
											</div>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}
			</div>
		{:else if activeTab === 'json'}
			<JsonPanel jsonString={jsonPayload} />
		{/if}
	{/if}
</div>
