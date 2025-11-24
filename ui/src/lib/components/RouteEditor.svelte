<script lang="ts">
	import type {
		PathMatchType,
		HeaderMatchDefinition,
		QueryParameterMatchDefinition,
		ClusterResponse,
		EndpointRequest
	} from '$lib/api/types';
	import type { RouteRule } from './EditableRoutesTable.svelte';
	import ClusterSelector, { type ClusterConfig } from './ClusterSelector.svelte';
	import HeaderMatcherList from './HeaderMatcherList.svelte';
	import QueryParamMatcherList from './QueryParamMatcherList.svelte';

	interface Props {
		show: boolean;
		route: RouteRule | null;
		domainName: string;
		clusters: ClusterResponse[];
		onSave: (route: RouteRule, newCluster: ClusterConfig | null) => void;
		onCancel: () => void;
	}

	let { show, route, domainName, clusters, onSave, onCancel }: Props = $props();

	const httpMethods = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', '*'];
	const pathTypes: { value: PathMatchType; label: string }[] = [
		{ value: 'prefix', label: 'Prefix' },
		{ value: 'exact', label: 'Exact' },
		{ value: 'regex', label: 'Regex' },
		{ value: 'template', label: 'Template' }
	];

	// Form state
	let method = $state('GET');
	let path = $state('/');
	let pathType = $state<PathMatchType>('prefix');
	let headers = $state<HeaderMatchDefinition[]>([]);
	let queryParams = $state<QueryParameterMatchDefinition[]>([]);
	let timeoutSeconds = $state(15);
	let clusterConfig = $state<ClusterConfig>({ mode: 'existing', existingClusterName: null });
	let showAdvanced = $state(false);

	// Reset form when route changes
	$effect(() => {
		if (show) {
			if (route) {
				// Editing existing route
				method = route.method;
				path = route.path;
				pathType = route.pathType;
				headers = [...(route.headers || [])];
				queryParams = [...(route.queryParams || [])];
				timeoutSeconds = route.timeoutSeconds || 15;
				clusterConfig = { mode: 'existing', existingClusterName: route.cluster };
				showAdvanced = (route.headers?.length || 0) > 0 || (route.queryParams?.length || 0) > 0;
			} else {
				// Creating new route
				method = 'GET';
				path = '/';
				pathType = 'prefix';
				headers = [];
				queryParams = [];
				timeoutSeconds = 15;
				clusterConfig = { mode: 'existing', existingClusterName: null };
				showAdvanced = false;
			}
		}
	});

	function handleSave() {
		const targetCluster =
			clusterConfig.mode === 'existing'
				? clusterConfig.existingClusterName || ''
				: clusterConfig.newClusterConfig?.name || '';

		const savedRoute: RouteRule = {
			id: route?.id || crypto.randomUUID(),
			method,
			path,
			pathType,
			cluster: targetCluster,
			headers: headers.length > 0 ? headers : undefined,
			queryParams: queryParams.length > 0 ? queryParams : undefined,
			timeoutSeconds
		};

		const newCluster = clusterConfig.mode === 'new' ? clusterConfig : null;
		onSave(savedRoute, newCluster);
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onCancel();
		}
	}
</script>

{#if show}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		onclick={handleBackdropClick}
	>
		<div
			class="bg-white rounded-lg shadow-xl w-full max-w-2xl mx-4 max-h-[90vh] overflow-y-auto"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div class="px-6 py-4 border-b border-gray-200 flex items-center justify-between">
				<h2 class="text-lg font-semibold text-gray-900">
					{route ? 'Edit Route' : 'Add Route'} to {domainName}
				</h2>
				<button
					type="button"
					onclick={onCancel}
					class="p-1 text-gray-400 hover:text-gray-600 rounded"
				>
					<svg class="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M6 18L18 6M6 6l12 12"
						/>
					</svg>
				</button>
			</div>

			<!-- Body -->
			<div class="px-6 py-4 space-y-6">
				<!-- Method, Path, Match Type -->
				<div class="flex items-end gap-3">
					<div class="w-28">
						<label for="route-method" class="block text-sm font-medium text-gray-700 mb-1"
							>Method</label
						>
						<select
							id="route-method"
							bind:value={method}
							class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						>
							{#each httpMethods as m}
								<option value={m}>{m === '*' ? 'ANY' : m}</option>
							{/each}
						</select>
					</div>
					<div class="flex-1">
						<label for="route-path" class="block text-sm font-medium text-gray-700 mb-1">Path</label
						>
						<input
							id="route-path"
							type="text"
							bind:value={path}
							placeholder="/api/v1/users"
							class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						/>
					</div>
					<div class="w-32">
						<label for="route-match" class="block text-sm font-medium text-gray-700 mb-1"
							>Match</label
						>
						<select
							id="route-match"
							bind:value={pathType}
							class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						>
							{#each pathTypes as pt}
								<option value={pt.value}>{pt.label}</option>
							{/each}
						</select>
					</div>
				</div>

				<!-- Target Cluster -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">Target Cluster</label>
					<ClusterSelector
						{clusters}
						config={clusterConfig}
						onConfigChange={(c) => (clusterConfig = c)}
					/>
				</div>

				<!-- Advanced Options (collapsible) -->
				<div class="border-t border-gray-200 pt-4">
					<button
						type="button"
						onclick={() => (showAdvanced = !showAdvanced)}
						class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
					>
						<svg
							class="h-4 w-4 transition-transform {showAdvanced ? 'rotate-90' : ''}"
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
						Advanced Options (Headers, Query Params, Timeout)
					</button>

					{#if showAdvanced}
						<div class="mt-4 space-y-4 pl-6">
							<!-- Header Matchers -->
							<HeaderMatcherList
								{headers}
								onHeadersChange={(h) => (headers = h)}
							/>

							<!-- Query Param Matchers -->
							<QueryParamMatcherList
								params={queryParams}
								onParamsChange={(p) => (queryParams = p)}
							/>

							<!-- Timeout -->
							<div>
								<label for="route-timeout" class="block text-sm font-medium text-gray-600 mb-1"
									>Timeout (seconds)</label
								>
								<input
									id="route-timeout"
									type="number"
									min="1"
									max="3600"
									bind:value={timeoutSeconds}
									class="w-24 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
								/>
							</div>
						</div>
					{/if}
				</div>
			</div>

			<!-- Footer -->
			<div class="px-6 py-4 border-t border-gray-200 flex justify-end gap-3">
				<button
					type="button"
					onclick={onCancel}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Cancel
				</button>
				<button
					type="button"
					onclick={handleSave}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					{route ? 'Save Changes' : 'Add Route'}
				</button>
			</div>
		</div>
	</div>
{/if}
