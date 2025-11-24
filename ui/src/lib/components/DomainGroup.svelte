<script lang="ts">
	import type { ClusterResponse } from '$lib/api/types';
	import EditableRoutesTable from './EditableRoutesTable.svelte';
	import type { RouteRule } from './EditableRoutesTable.svelte';

	export interface DomainGroupData {
		id: string;
		domains: string[];
		routes: RouteRule[];
	}

	interface Props {
		group: DomainGroupData;
		clusters: ClusterResponse[];
		onEditDomain: (groupId: string) => void;
		onDeleteDomain: (groupId: string) => void;
		onAddRoute: (groupId: string) => void;
		onEditRoute: (groupId: string, routeId: string) => void;
		onDeleteRoute: (groupId: string, routeId: string) => void;
	}

	let {
		group,
		clusters,
		onEditDomain,
		onDeleteDomain,
		onAddRoute,
		onEditRoute,
		onDeleteRoute
	}: Props = $props();

	let isExpanded = $state(true);

	function toggleExpand() {
		isExpanded = !isExpanded;
	}
</script>

<div class="border border-gray-200 rounded-lg overflow-hidden">
	<!-- Domain Header -->
	<div class="bg-gray-50 px-4 py-3 flex items-center justify-between">
		<div class="flex items-center gap-3">
			<button
				type="button"
				onclick={toggleExpand}
				class="p-1 rounded hover:bg-gray-200 transition-colors"
				title={isExpanded ? 'Collapse' : 'Expand'}
			>
				<svg
					class="h-5 w-5 text-gray-500 transition-transform {isExpanded ? 'rotate-90' : ''}"
					fill="none"
					stroke="currentColor"
					viewBox="0 0 24 24"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M9 5l7 7-7 7"
					/>
				</svg>
			</button>
			<div>
				<span class="font-medium text-gray-900">{group.domains.join(', ')}</span>
				<span class="ml-2 text-sm text-gray-500">({group.routes.length} route{group.routes.length !== 1 ? 's' : ''})</span>
			</div>
		</div>
		<div class="flex items-center gap-2">
			<button
				type="button"
				onclick={() => onEditDomain(group.id)}
				class="px-3 py-1 text-sm text-blue-600 hover:bg-blue-50 rounded transition-colors"
			>
				Edit
			</button>
			<button
				type="button"
				onclick={() => onDeleteDomain(group.id)}
				class="px-3 py-1 text-sm text-red-600 hover:bg-red-50 rounded transition-colors"
			>
				Delete
			</button>
		</div>
	</div>

	<!-- Routes Table (collapsible) -->
	{#if isExpanded}
		<div class="p-4 bg-white">
			<EditableRoutesTable
				routes={group.routes}
				{clusters}
				onEditRoute={(routeId) => onEditRoute(group.id, routeId)}
				onDeleteRoute={(routeId) => onDeleteRoute(group.id, routeId)}
			/>
			<div class="mt-3">
				<button
					type="button"
					onclick={() => onAddRoute(group.id)}
					class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
				>
					<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M12 4v16m8-8H4"
						/>
					</svg>
					Add Route to {group.domains[0]}
				</button>
			</div>
		</div>
	{/if}
</div>
