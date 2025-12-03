<script lang="ts">
	import type {
		PathMatchType,
		HeaderMatchDefinition,
		QueryParameterMatchDefinition,
		ClusterResponse
	} from '$lib/api/types';
	import Badge from './Badge.svelte';

	export type RouteActionType = 'forward' | 'weighted' | 'redirect';

	export interface WeightedCluster {
		name: string;
		weight: number;
	}

	export interface RetryBackoff {
		baseIntervalMs?: number;
		maxIntervalMs?: number;
	}

	export interface RetryPolicy {
		maxRetries: number;
		retryOn: string[];
		perTryTimeoutSeconds?: number;
		backoff?: RetryBackoff;
	}

	export interface RouteRule {
		id: string;
		method: string;
		path: string;
		pathType: PathMatchType;
		actionType: RouteActionType;
		// Forward action fields
		cluster?: string;
		prefixRewrite?: string;
		templateRewrite?: string;
		timeoutSeconds?: number;
		retryPolicy?: RetryPolicy;
		// Weighted action fields
		weightedClusters?: WeightedCluster[];
		totalWeight?: number;
		// Redirect action fields
		hostRedirect?: string;
		pathRedirect?: string;
		responseCode?: number;
		// Common matchers
		headers?: HeaderMatchDefinition[];
		queryParams?: QueryParameterMatchDefinition[];
	}

	interface Props {
		routes: RouteRule[];
		clusters: ClusterResponse[];
		onEditRoute: (routeId: string) => void;
		onDeleteRoute: (routeId: string) => void;
	}

	let { routes, clusters, onEditRoute, onDeleteRoute }: Props = $props();

	function getMethodVariant(
		method: string
	): 'blue' | 'green' | 'yellow' | 'red' | 'gray' {
		switch (method.toUpperCase()) {
			case 'GET':
				return 'green';
			case 'POST':
				return 'blue';
			case 'PUT':
				return 'yellow';
			case 'PATCH':
				return 'yellow';
			case 'DELETE':
				return 'red';
			case '*':
				return 'gray';
			default:
				return 'gray';
		}
	}

	function getClusterDisplay(clusterName: string): string {
		const cluster = clusters.find((c) => c.name === clusterName);
		if (cluster) {
			return cluster.name;
		}
		return clusterName;
	}

	function getActionBadgeVariant(actionType: RouteActionType): 'blue' | 'yellow' | 'gray' {
		switch (actionType) {
			case 'forward':
				return 'blue';
			case 'weighted':
				return 'yellow';
			case 'redirect':
				return 'gray';
			default:
				return 'gray';
		}
	}

	function getTargetDisplay(route: RouteRule): string {
		const actionType = route.actionType || 'forward';
		switch (actionType) {
			case 'forward':
				const cluster = route.cluster ? getClusterDisplay(route.cluster) : '-';
				const rewrite = route.prefixRewrite || route.templateRewrite;
				return rewrite ? `${cluster} (rewrite: ${rewrite})` : cluster;
			case 'weighted':
				if (route.weightedClusters && route.weightedClusters.length > 0) {
					const totalWeight = route.totalWeight || route.weightedClusters.reduce((sum, c) => sum + c.weight, 0);
					return route.weightedClusters.map(c => `${c.name}: ${Math.round(c.weight / totalWeight * 100)}%`).join(', ');
				}
				return '-';
			case 'redirect':
				const parts = [];
				if (route.hostRedirect) parts.push(route.hostRedirect);
				if (route.pathRedirect) parts.push(route.pathRedirect);
				const code = route.responseCode || 302;
				return parts.length > 0 ? `${code} -> ${parts.join('')}` : `${code} redirect`;
			default:
				return route.cluster || '-';
		}
	}
</script>

{#if routes.length === 0}
	<div class="text-center py-8 text-gray-500">
		<p class="text-sm">No routes configured for this domain</p>
	</div>
{:else}
	<div class="overflow-x-auto">
		<table class="min-w-full divide-y divide-gray-200">
			<thead class="bg-gray-50">
				<tr>
					<th
						class="px-4 py-2 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>Method</th
					>
					<th
						class="px-4 py-2 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>Path</th
					>
					<th
						class="px-4 py-2 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>Match</th
					>
					<th
						class="px-4 py-2 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>Action</th
					>
					<th
						class="px-4 py-2 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>Target</th
					>
					<th
						class="px-4 py-2 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
						></th
					>
				</tr>
			</thead>
			<tbody class="bg-white divide-y divide-gray-200">
				{#each routes as route}
					<tr class="hover:bg-gray-50">
						<td class="px-4 py-3 whitespace-nowrap">
							<Badge variant={getMethodVariant(route.method)}>
								{route.method === '*' ? 'ANY' : route.method}
							</Badge>
						</td>
						<td class="px-4 py-3 whitespace-nowrap">
							<code class="text-sm text-gray-900 bg-gray-100 px-2 py-1 rounded">{route.path}</code>
							{#if (route.headers && route.headers.length > 0) || (route.queryParams && route.queryParams.length > 0)}
								<span class="ml-2 text-xs text-gray-500" title="Has advanced matching rules">
									+{(route.headers?.length || 0) + (route.queryParams?.length || 0)} matcher{(route.headers?.length || 0) + (route.queryParams?.length || 0) !== 1 ? 's' : ''}
								</span>
							{/if}
						</td>
						<td class="px-4 py-3 whitespace-nowrap">
							<span class="text-sm text-gray-600">{route.pathType}</span>
						</td>
						<td class="px-4 py-3 whitespace-nowrap">
							<div class="flex items-center gap-1.5">
								<Badge variant={getActionBadgeVariant(route.actionType || 'forward')}>
									{(route.actionType || 'forward').charAt(0).toUpperCase() + (route.actionType || 'forward').slice(1)}
								</Badge>
								{#if route.retryPolicy}
									<span
										class="inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-purple-100 text-purple-700"
										title="Retry: {route.retryPolicy.maxRetries}x on {route.retryPolicy.retryOn.join(', ')}"
									>
										<svg class="h-3 w-3 mr-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
										</svg>
										{route.retryPolicy.maxRetries}
									</span>
								{/if}
							</div>
						</td>
						<td class="px-4 py-3">
							<span class="text-sm text-gray-900 truncate max-w-xs block" title={getTargetDisplay(route)}>{getTargetDisplay(route)}</span>
						</td>
						<td class="px-4 py-3 whitespace-nowrap text-right">
							<div class="flex items-center justify-end gap-1">
								<button
									type="button"
									onclick={() => onEditRoute(route.id)}
									class="p-1.5 text-gray-400 hover:text-blue-600 hover:bg-blue-50 rounded transition-colors"
									title="Edit route"
								>
									<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
										/>
									</svg>
								</button>
								<button
									type="button"
									onclick={() => onDeleteRoute(route.id)}
									class="p-1.5 text-gray-400 hover:text-red-600 hover:bg-red-50 rounded transition-colors"
									title="Delete route"
								>
									<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
										/>
									</svg>
								</button>
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}
