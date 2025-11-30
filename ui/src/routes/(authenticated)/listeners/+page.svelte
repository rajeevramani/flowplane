<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Eye, Lock, LockOpen, Pencil } from 'lucide-svelte';
	import DataTable from '$lib/components/DataTable.svelte';
	import StatusIndicator from '$lib/components/StatusIndicator.svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import ListenerConfigEditor from '$lib/components/ListenerConfigEditor.svelte';
	import type {
		ListenerResponse,
		RouteResponse,
		ImportSummary,
		ListenerFilterChainInput,
		ListenerFilterInput,
		UpdateListenerBody
	} from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Data
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedListener = $state<ListenerResponse | null>(null);

	// Edit mode state
	let editMode = $state(false);
	let isSaving = $state(false);
	let saveError = $state<string | null>(null);
	let editedAddress = $state('');
	let editedPort = $state(8080);
	let editedProtocol = $state('HTTP');
	let editedFilterChains = $state<ListenerFilterChainInput[]>([]);

	onMount(async () => {
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				await loadData();
			}
		});
	});

	onDestroy(() => {
		if (unsubscribe) unsubscribe();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [listenersData, routesData, importsData] = await Promise.all([
				apiClient.listListeners(),
				apiClient.listRoutes(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			listeners = listenersData;
			routes = routesData;
			imports = importsData;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Table columns
	const columns = [
		{ key: 'name', label: 'Name', sortable: true },
		{ key: 'team', label: 'Team', sortable: true },
		{ key: 'address', label: 'Address' },
		{ key: 'protocol', label: 'Protocol' },
		{ key: 'tls', label: 'TLS' },
		{ key: 'routes', label: 'Routes' },
		{ key: 'status', label: 'Status' },
		{ key: 'source', label: 'Source' }
	];

	// Transform listeners for table display
	let tableData = $derived(
		listeners
			.filter((listener) => {
				// Filter by team if a team is selected
				if (currentTeam && listener.team !== currentTeam) return false;

				// Filter by search query
				if (!searchQuery) return true;
				const query = searchQuery.toLowerCase();
				return (
					listener.name.toLowerCase().includes(query) ||
					listener.address.toLowerCase().includes(query) ||
					listener.team.toLowerCase().includes(query)
				);
			})
			.map((listener) => {
				const config = listener.config || {};

				// Check for TLS
				const hasTls = config.filter_chains?.some(
					(fc: { tls_context?: unknown }) => fc.tls_context
				);

				// Count routes associated with this listener
				const routeCount = getRoutesForListener(listener).length;

				// Get source info
				const importRecord = listener.importId
					? imports.find((i) => i.id === listener.importId)
					: null;

				return {
					id: listener.name,
					name: listener.name,
					team: listener.team,
					address: `${listener.address}:${listener.port}`,
					protocol: listener.protocol || 'HTTP',
					hasTls,
					routes: routeCount,
					source: importRecord ? importRecord.specName : 'Manual',
					sourceType: importRecord ? 'import' : 'manual',
					_raw: listener
				};
			})
	);

	function getRoutesForListener(listener: ListenerResponse): RouteResponse[] {
		const routeNames = new Set<string>();

		listener.config?.filter_chains?.forEach(
			(fc: { filters?: { filter_type?: { HttpConnectionManager?: { route_config_name?: string } } }[] }) => {
				fc.filters?.forEach((f) => {
					const routeConfigName = f.filter_type?.HttpConnectionManager?.route_config_name;
					if (routeConfigName) {
						routeNames.add(routeConfigName);
					}
				});
			}
		);

		return routes.filter((r) => routeNames.has(r.name));
	}

	function openDrawer(row: Record<string, unknown>) {
		selectedListener = (row._raw as ListenerResponse) || null;
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedListener = null;
		editMode = false;
		saveError = null;
	}

	function parseFilterChains(listener: ListenerResponse): ListenerFilterChainInput[] {
		const config = listener.config || {};
		const chains = config.filter_chains || [];

		return chains.map((fc: any) => {
			const filters: ListenerFilterInput[] = (fc.filters || []).map((f: any) => {
				if (f.filter_type?.HttpConnectionManager) {
					return {
						name: f.name || 'http_connection_manager',
						type: 'httpConnectionManager' as const,
						routeConfigName: f.filter_type.HttpConnectionManager.route_config_name,
						inlineRouteConfig: f.filter_type.HttpConnectionManager.inline_route_config,
						accessLog: f.filter_type.HttpConnectionManager.access_log,
						tracing: f.filter_type.HttpConnectionManager.tracing,
						httpFilters: f.filter_type.HttpConnectionManager.http_filters
					};
				} else if (f.filter_type?.TcpProxy) {
					return {
						name: f.name || 'tcp_proxy',
						type: 'tcpProxy' as const,
						cluster: f.filter_type.TcpProxy.cluster,
						accessLog: f.filter_type.TcpProxy.access_log
					};
				}
				return {
					name: f.name || 'unknown',
					type: 'httpConnectionManager' as const
				};
			});

			return {
				name: fc.name,
				filters,
				tlsContext: fc.tls_context ? {
					certChainFile: fc.tls_context.cert_chain_file,
					privateKeyFile: fc.tls_context.private_key_file,
					caCertFile: fc.tls_context.ca_cert_file,
					requireClientCertificate: fc.tls_context.require_client_certificate,
					minTlsVersion: fc.tls_context.min_tls_version || 'V1_2'
				} : undefined
			};
		});
	}

	function startEdit() {
		if (!selectedListener) return;

		editedAddress = selectedListener.address;
		editedPort = selectedListener.port;
		editedProtocol = selectedListener.protocol || 'HTTP';
		editedFilterChains = parseFilterChains(selectedListener);
		editMode = true;
		saveError = null;
	}

	function cancelEdit() {
		editMode = false;
		saveError = null;
	}

	async function saveEdit() {
		if (!selectedListener) return;

		isSaving = true;
		saveError = null;

		try {
			const body: UpdateListenerBody = {
				address: editedAddress,
				port: editedPort,
				protocol: editedProtocol,
				filterChains: editedFilterChains
			};

			await apiClient.updateListener(selectedListener.name, body);
			await loadData();

			// Update selectedListener with the new data
			const updated = listeners.find(l => l.name === selectedListener?.name);
			if (updated) {
				selectedListener = updated;
			}

			editMode = false;
		} catch (err) {
			saveError = err instanceof Error ? err.message : 'Failed to update listener';
		} finally {
			isSaving = false;
		}
	}

	async function handleDelete(listenerName: string) {
		if (!confirm(`Are you sure you want to delete the listener "${listenerName}"?`)) return;

		try {
			await apiClient.deleteListener(listenerName);
			await loadData();
			closeDrawer();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete listener';
		}
	}
</script>

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">Listeners</h1>
		<p class="mt-1 text-sm text-gray-600">Network listeners accepting incoming traffic</p>
	</div>
</div>

<!-- Error Message -->
{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

<!-- Search Bar -->
<div class="mb-6">
	<input
		type="text"
		bind:value={searchQuery}
		placeholder="Search listeners..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- Data Table -->
<DataTable
	{columns}
	data={tableData}
	loading={isLoading}
	emptyMessage="No listeners found."
	rowKey="id"
	onRowClick={openDrawer}
>
	{#snippet cell({ row, column })}
		{#if column.key === 'name'}
			<span class="font-medium text-blue-600 hover:text-blue-800">{row.name}</span>
		{:else if column.key === 'team'}
			<Badge variant="indigo">{row.team}</Badge>
		{:else if column.key === 'address'}
			<span class="font-mono text-gray-900">{row.address}</span>
		{:else if column.key === 'protocol'}
			<Badge variant="gray">{row.protocol}</Badge>
		{:else if column.key === 'tls'}
			{#if row.hasTls}
				<div class="flex items-center gap-1 text-green-600">
					<Lock class="h-4 w-4" />
					<span>Enabled</span>
				</div>
			{:else}
				<div class="flex items-center gap-1 text-gray-400">
					<LockOpen class="h-4 w-4" />
					<span>Disabled</span>
				</div>
			{/if}
		{:else if column.key === 'routes'}
			<span class="text-gray-600">{row.routes} route{row.routes !== 1 ? 's' : ''}</span>
		{:else if column.key === 'status'}
			<StatusIndicator status="active" />
		{:else if column.key === 'source'}
			{#if row.sourceType === 'import'}
				<Badge variant="purple">{row.source}</Badge>
			{:else}
				<span class="text-gray-500">{row.source}</span>
			{/if}
		{:else}
			{String(row[column.key] ?? '')}
		{/if}
	{/snippet}

	{#snippet actions({ row })}
		<button
			onclick={(e) => {
				e.stopPropagation();
				openDrawer(row);
			}}
			class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-blue-600 transition-colors"
			title="View details"
		>
			<Eye class="h-4 w-4" />
		</button>
	{/snippet}
</DataTable>

<!-- Detail Drawer -->
<DetailDrawer
	open={drawerOpen}
	title={selectedListener?.name || ''}
	subtitle={selectedListener ? `Team: ${selectedListener.team}` : undefined}
	onClose={closeDrawer}
>
	{#if selectedListener}
		{@const config = selectedListener.config || {}}

		{#if editMode}
			<!-- Edit Mode -->
			<div class="space-y-4">
				{#if saveError}
					<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-4">
						<p class="text-red-800 text-sm">{saveError}</p>
					</div>
				{/if}

				<ListenerConfigEditor
					address={editedAddress}
					port={editedPort}
					protocol={editedProtocol}
					filterChains={editedFilterChains}
					onAddressChange={(addr) => editedAddress = addr}
					onPortChange={(p) => editedPort = p}
					onProtocolChange={(proto) => editedProtocol = proto}
					onFilterChainsChange={(chains) => editedFilterChains = chains}
					showAddressPortWarning={true}
				/>
			</div>
		{:else}
			<!-- View Mode -->
			<div class="space-y-6">
				<!-- Overview -->
				<ConfigCard title="Overview" variant="gray">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Address</dt>
							<dd class="font-mono text-gray-900">{selectedListener.address}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Port</dt>
							<dd class="font-mono text-gray-900">{selectedListener.port}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Protocol</dt>
							<dd class="text-gray-900">{selectedListener.protocol || 'HTTP'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Version</dt>
							<dd class="text-gray-900">{selectedListener.version}</dd>
						</div>
					</dl>
				</ConfigCard>

				<!-- Filter Chains -->
				{#if config.filter_chains?.length}
					<ConfigCard title="Filter Chains" variant="blue">
						<div class="space-y-3">
							{#each config.filter_chains as fc, i}
								<div class="p-3 bg-white rounded border border-blue-200">
									<div class="font-medium text-gray-900">{fc.name || `Chain ${i + 1}`}</div>
									{#if fc.filters?.length}
										<div class="mt-2 space-y-1">
											{#each fc.filters as filter}
												{@const routeConfigName = filter.filter_type?.HttpConnectionManager?.route_config_name}
												<div class="text-sm text-gray-600">
													{filter.name || (filter.filter_type?.HttpConnectionManager ? 'HttpConnectionManager' : 'Filter')}
													{#if routeConfigName}
														<span class="text-gray-400"> → {routeConfigName}</span>
													{/if}
												</div>
											{/each}
										</div>
									{/if}
									{#if fc.tls_context}
										<div class="mt-2 flex items-center gap-1 text-green-600 text-sm">
											<Lock class="h-3 w-3" />
											TLS Enabled
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}

				<!-- Associated Routes -->
				{#if getRoutesForListener(selectedListener).length > 0}
					{@const listenerRoutes = getRoutesForListener(selectedListener)}
					<ConfigCard title="Associated Routes" variant="green" collapsible defaultCollapsed>
						<div class="space-y-2">
							{#each listenerRoutes as route}
								<div class="p-2 bg-white rounded border border-green-200">
									<div class="font-medium text-gray-900">{route.name}</div>
									<div class="text-sm text-gray-500">{route.pathPrefix}</div>
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}
			</div>
		{/if}
	{/if}

	{#snippet footer()}
		{#if editMode}
			<!-- Edit Mode Footer -->
			<div class="flex justify-between items-center">
				<Button variant="ghost" onclick={cancelEdit} disabled={isSaving}>
					Cancel
				</Button>
				<div class="flex gap-3">
					<Button
						variant="danger"
						onclick={() => selectedListener && handleDelete(selectedListener.name)}
						disabled={isSaving}
					>
						Delete
					</Button>
					<Button variant="primary" onclick={saveEdit} loading={isSaving}>
						{isSaving ? 'Saving...' : 'Save Changes'}
					</Button>
				</div>
			</div>
		{:else}
			<!-- View Mode Footer -->
			<div class="flex justify-between items-center">
				<Button variant="ghost" onclick={closeDrawer}>Close</Button>
				<div class="flex gap-3">
					<Button variant="secondary" onclick={startEdit}>
						<Pencil class="h-4 w-4 mr-2" />
						Edit
					</Button>
					<Button
						variant="danger"
						onclick={() => selectedListener && handleDelete(selectedListener.name)}
					>
						Delete
					</Button>
				</div>
			</div>
		{/if}
	{/snippet}
</DetailDrawer>
