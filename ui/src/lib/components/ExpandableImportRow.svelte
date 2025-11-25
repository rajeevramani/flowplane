<script lang="ts">
	import Badge from './Badge.svelte';
	import type { ImportSummary, RouteResponse, ClusterResponse, ListenerResponse } from '$lib/api/types';

	interface Props {
		importRecord: ImportSummary;
		routes?: RouteResponse[];
		clusters?: ClusterResponse[];
		listeners?: ListenerResponse[];
		onDelete?: (importRecord: ImportSummary) => void;
	}

	let { importRecord, routes = [], clusters = [], listeners = [], onDelete }: Props = $props();

	let isExpanded = $state(false);

	function toggle() {
		isExpanded = !isExpanded;
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function formatShortDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric'
		});
	}

	function getCreatedResources(): {
		routes: RouteResponse[];
		clusters: ClusterResponse[];
		listeners: ListenerResponse[];
	} {
		return {
			routes: routes.filter(r => r.importId === importRecord.id),
			clusters: clusters.filter(c => c.importId === importRecord.id),
			listeners: listeners.filter(l => l.importId === importRecord.id)
		};
	}

	// Get the associated listener (either created by this import or an existing one)
	function getAssociatedListener(): ListenerResponse | null {
		// First, check if we created a listener
		const createdListener = listeners.find(l => l.importId === importRecord.id);
		if (createdListener) return createdListener;

		// If not, try to find the listener by name from importRecord
		if (importRecord.listenerName) {
			return listeners.find(l => l.name === importRecord.listenerName) || null;
		}
		return null;
	}

	const associatedListener = $derived(getAssociatedListener());

	function handleDelete(e: Event) {
		e.stopPropagation();
		if (onDelete && confirm(`Delete import "${importRecord.specName}"? This will also delete all associated resources.`)) {
			onDelete(importRecord);
		}
	}

	const createdResources = $derived(getCreatedResources());
	// Count includes associated listener even if not created by this import
	const totalResources = $derived(
		createdResources.routes.length +
		createdResources.clusters.length +
		(associatedListener ? 1 : 0)
	);
</script>

<!-- Row Header -->
<button
	type="button"
	onclick={toggle}
	class="w-full flex items-center justify-between py-3 px-4 hover:bg-gray-50 transition-colors text-left group border-b border-gray-100"
>
	<div class="flex items-center gap-6 flex-1 min-w-0">
		<!-- Expand Icon -->
		<svg
			class="h-4 w-4 text-gray-400 transition-transform flex-shrink-0 {isExpanded ? 'rotate-90' : ''}"
			fill="none"
			stroke="currentColor"
			viewBox="0 0 24 24"
		>
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
		</svg>

		<!-- Spec Name -->
		<div class="w-48 min-w-0">
			<span class="font-medium text-gray-900 truncate block">{importRecord.specName}</span>
		</div>

		<!-- Version -->
		<div class="w-20 text-sm text-gray-600">
			{importRecord.specVersion || '-'}
		</div>

		<!-- Team -->
		<div class="w-24">
			<Badge variant="blue">{importRecord.team}</Badge>
		</div>

		<!-- Resources Created -->
		<div class="w-32 text-sm text-gray-600">
			{totalResources} resource{totalResources !== 1 ? 's' : ''}
		</div>

		<!-- Imported Date -->
		<div class="w-28 text-sm text-gray-500">
			{formatShortDate(importRecord.importedAt)}
		</div>

		<!-- ID (truncated) -->
		<div class="flex-1 text-sm text-gray-400 font-mono truncate" title={importRecord.id}>
			{importRecord.id.slice(0, 8)}...
		</div>
	</div>

	<!-- Actions -->
	<div class="flex items-center gap-2">
		{#if onDelete}
			<button
				onclick={handleDelete}
				class="p-1.5 text-gray-400 hover:text-red-600 rounded opacity-0 group-hover:opacity-100 transition-opacity"
				title="Delete"
			>
				<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
				</svg>
			</button>
		{/if}
	</div>
</button>

<!-- Expanded Content -->
{#if isExpanded}
	<div class="bg-gray-50 border-b border-gray-200 px-4 py-4">
		<!-- Basic Info -->
		<div class="grid grid-cols-4 gap-4 mb-4">
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Spec Name</label>
				<p class="mt-1 text-sm text-gray-900">{importRecord.specName}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Version</label>
				<p class="mt-1 text-sm text-gray-900">{importRecord.specVersion || 'N/A'}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Imported</label>
				<p class="mt-1 text-sm text-gray-900">{formatDate(importRecord.importedAt)}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Updated</label>
				<p class="mt-1 text-sm text-gray-900">{formatDate(importRecord.updatedAt)}</p>
			</div>
		</div>

		<div class="mb-4">
			<label class="block text-xs font-medium text-gray-500 uppercase">Import ID</label>
			<p class="mt-1 text-sm text-gray-900 font-mono">{importRecord.id}</p>
		</div>

		<!-- Created Resources Summary -->
		<div class="grid grid-cols-3 gap-4 mb-4">
			<div class="bg-white rounded-lg border border-gray-200 p-3">
				<div class="flex items-center gap-2">
					<div class="p-2 bg-purple-50 rounded">
						<svg class="h-4 w-4 text-purple-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 7l5 5m0 0l-5 5m5-5H6" />
						</svg>
					</div>
					<div>
						<p class="text-2xl font-bold text-gray-900">{createdResources.routes.length}</p>
						<p class="text-xs text-gray-500">Routes</p>
					</div>
				</div>
			</div>
			<div class="bg-white rounded-lg border border-gray-200 p-3">
				<div class="flex items-center gap-2">
					<div class="p-2 bg-green-50 rounded">
						<svg class="h-4 w-4 text-green-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
						</svg>
					</div>
					<div>
						<p class="text-2xl font-bold text-gray-900">{createdResources.clusters.length}</p>
						<p class="text-xs text-gray-500">Clusters</p>
					</div>
				</div>
			</div>
			<div class="bg-white rounded-lg border border-gray-200 p-3">
				<div class="flex items-center gap-2">
					<div class="p-2 bg-orange-50 rounded">
						<svg class="h-4 w-4 text-orange-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
						</svg>
					</div>
					<div>
						<p class="text-2xl font-bold text-gray-900">{associatedListener ? 1 : 0}</p>
						<p class="text-xs text-gray-500">Listeners</p>
					</div>
				</div>
			</div>
		</div>

		<!-- Resource Lists -->
		{#if createdResources.routes.length > 0}
			<div class="mb-4">
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Routes</h4>
				<div class="flex flex-wrap gap-2">
					{#each createdResources.routes as route}
						<span class="inline-flex items-center px-2.5 py-1 bg-purple-50 text-purple-700 text-xs rounded-md border border-purple-200">
							{route.name}
						</span>
					{/each}
				</div>
			</div>
		{/if}

		{#if createdResources.clusters.length > 0}
			<div class="mb-4">
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Clusters</h4>
				<div class="flex flex-wrap gap-2">
					{#each createdResources.clusters as cluster}
						<span class="inline-flex items-center px-2.5 py-1 bg-green-50 text-green-700 text-xs rounded-md border border-green-200">
							{cluster.serviceName}
						</span>
					{/each}
				</div>
			</div>
		{/if}

		{#if associatedListener}
			<div>
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Listener</h4>
				<div class="flex flex-wrap gap-2">
					<span class="inline-flex items-center px-2.5 py-1 bg-orange-50 text-orange-700 text-xs rounded-md border border-orange-200">
						{associatedListener.name}
						{#if createdResources.listeners.length === 0}
							<span class="ml-1.5 px-1.5 py-0.5 bg-orange-100 text-orange-600 text-[10px] rounded" title="Using existing listener">shared</span>
						{/if}
					</span>
				</div>
			</div>
		{/if}
	</div>
{/if}
