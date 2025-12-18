<script lang="ts">
	import type {
		PathMatchType,
		HeaderMatchDefinition,
		QueryParameterMatchDefinition,
		ClusterResponse,
		HeaderMutationPerRouteConfig,
	} from "$lib/api/types";
	import type {
		RouteRule,
		RouteActionType,
		WeightedCluster,
		RetryPolicy,
	} from "./EditableRoutesTable.svelte";
	import { HTTP_METHODS, PATH_MATCH_TYPES, ROUTE_ACTION_TYPES } from "$lib/constants";
	import ClusterSelector, {
		type ClusterConfig,
	} from "./ClusterSelector.svelte";
	import HeaderMatcherList from "./HeaderMatcherList.svelte";
	import QueryParamMatcherList from "./QueryParamMatcherList.svelte";
	import HeaderMutationPerRouteForm from "./filters/HeaderMutationPerRouteForm.svelte";
	import RetryPolicyForm, {
		type RetryPreset,
		retryPresets,
		detectPresetFromConditions,
	} from "./filters/RetryPolicyForm.svelte";
	import WeightedClusterForm from "./route/WeightedClusterForm.svelte";
	import RedirectForm from "./route/RedirectForm.svelte";
	import PathRewriteForm from "./route/PathRewriteForm.svelte";

	interface Props {
		show: boolean;
		route: RouteRule | null;
		domainName: string;
		clusters: ClusterResponse[];
		onSave: (route: RouteRule, newCluster: ClusterConfig | null) => void;
		onCancel: () => void;
	}

	let { show, route, domainName, clusters, onSave, onCancel }: Props =
		$props();

	type ForwardSubTab = "target" | "resilience";

	// Form state
	let method = $state("GET");
	let path = $state("/");
	let pathType = $state<PathMatchType>("prefix");
	let actionType = $state<RouteActionType>("forward");
	let headers = $state<HeaderMatchDefinition[]>([]);
	let queryParams = $state<QueryParameterMatchDefinition[]>([]);
	// Forward action state
	let timeoutSeconds = $state(15);
	let clusterConfig = $state<ClusterConfig>({
		mode: "existing",
		existingClusterName: null,
	});
	let prefixRewrite = $state("");
	let templateRewrite = $state("");
	// Weighted action state
	let weightedClusters = $state<WeightedCluster[]>([
		{ name: "", weight: 100 },
	]);
	// Redirect action state
	let hostRedirect = $state("");
	let pathRedirect = $state("");
	let responseCode = $state(302);
	// Retry policy state (Forward action only)
	let retryEnabled = $state(false);
	let retryMaxRetries = $state(3);
	let retryPreset = $state<RetryPreset>("5xx");
	let retryOnCustom = $state<string[]>([]);
	let retryPerTryTimeout = $state<number | null>(null);
	let showBackoff = $state(false);
	let backoffBaseInterval = $state(100);
	let backoffMaxInterval = $state(1000);
	// Filter state
	let headerMutationConfig = $state<HeaderMutationPerRouteConfig | null>(
		null,
	);
	// UI state
	let showAdvanced = $state(false);
	let showFilters = $state(false);
	let forwardSubTab = $state<ForwardSubTab>("target");

	// Reset form when route changes
	$effect(() => {
		if (show) {
			if (route) {
				// Editing existing route
				method = route.method;
				path = route.path;
				pathType = route.pathType;
				actionType = route.actionType || "forward";
				headers = [...(route.headers || [])];
				queryParams = [...(route.queryParams || [])];
				timeoutSeconds = route.timeoutSeconds || 15;
				clusterConfig = {
					mode: "existing",
					existingClusterName: route.cluster || null,
				};
				prefixRewrite = route.prefixRewrite || "";
				templateRewrite = route.templateRewrite || "";
				weightedClusters = route.weightedClusters?.length
					? [...route.weightedClusters]
					: [{ name: "", weight: 100 }];
				hostRedirect = route.hostRedirect || "";
				pathRedirect = route.pathRedirect || "";
				responseCode = route.responseCode || 302;
				showAdvanced =
					(route.headers?.length || 0) > 0 ||
					(route.queryParams?.length || 0) > 0;
				// Load retry policy
				if (route.retryPolicy) {
					retryEnabled = true;
					retryMaxRetries = route.retryPolicy.maxRetries;
					const detectedPreset = detectPresetFromConditions(
						route.retryPolicy.retryOn,
					);
					retryPreset = detectedPreset;
					if (detectedPreset === "custom") {
						retryOnCustom = [...route.retryPolicy.retryOn];
					}
					retryPerTryTimeout =
						route.retryPolicy.perTryTimeoutSeconds || null;
					if (route.retryPolicy.backoff) {
						showBackoff = true;
						backoffBaseInterval =
							route.retryPolicy.backoff.baseIntervalMs || 100;
						backoffMaxInterval =
							route.retryPolicy.backoff.maxIntervalMs || 1000;
					} else {
						showBackoff = false;
						backoffBaseInterval = 100;
						backoffMaxInterval = 1000;
					}
				} else {
					retryEnabled = false;
					retryMaxRetries = 3;
					retryPreset = "5xx";
					retryOnCustom = [];
					retryPerTryTimeout = null;
					showBackoff = false;
					backoffBaseInterval = 100;
					backoffMaxInterval = 1000;
				}
				// Load filter config
				headerMutationConfig =
					(route as RouteRule & { headerMutationConfig?: HeaderMutationPerRouteConfig }).headerMutationConfig || null;
				showFilters = headerMutationConfig !== null;
				forwardSubTab = "target";
			} else {
				// Creating new route
				method = "GET";
				path = "/";
				pathType = "prefix";
				actionType = "forward";
				headers = [];
				queryParams = [];
				timeoutSeconds = 15;
				clusterConfig = { mode: "existing", existingClusterName: null };
				prefixRewrite = "";
				templateRewrite = "";
				weightedClusters = [{ name: "", weight: 100 }];
				hostRedirect = "";
				pathRedirect = "";
				responseCode = 302;
				showAdvanced = false;
				// Reset retry policy
				retryEnabled = false;
				retryMaxRetries = 3;
				retryPreset = "5xx";
				retryOnCustom = [];
				retryPerTryTimeout = null;
				showBackoff = false;
				backoffBaseInterval = 100;
				backoffMaxInterval = 1000;
				// Reset filter config
				headerMutationConfig = null;
				showFilters = false;
				forwardSubTab = "target";
			}
		}
	});

	function handleSave() {
		let savedRoute: RouteRule;

		if (actionType === "forward") {
			const targetCluster =
				clusterConfig.mode === "existing"
					? clusterConfig.existingClusterName || ""
					: clusterConfig.newClusterConfig?.name || "";

			// Build retry policy if enabled
			let retryPolicy: RetryPolicy | undefined = undefined;
			if (retryEnabled) {
				const conditions =
					retryPreset === "custom"
						? retryOnCustom
						: retryPresets.find((p) => p.value === retryPreset)
								?.conditions || [];

				retryPolicy = {
					maxRetries: retryMaxRetries,
					retryOn: conditions,
					perTryTimeoutSeconds: retryPerTryTimeout || undefined,
					backoff: showBackoff
						? {
								baseIntervalMs: backoffBaseInterval,
								maxIntervalMs: backoffMaxInterval,
							}
						: undefined,
				};
			}

			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: "forward",
				cluster: targetCluster,
				prefixRewrite: prefixRewrite || undefined,
				templateRewrite: templateRewrite || undefined,
				timeoutSeconds,
				retryPolicy,
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined,
				headerMutationConfig: headerMutationConfig || undefined,
			} as RouteRule;
		} else if (actionType === "weighted") {
			const validClusters = weightedClusters.filter(
				(c) => c.name && c.weight > 0,
			);
			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: "weighted",
				weightedClusters: validClusters,
				totalWeight: validClusters.reduce(
					(sum, c) => sum + c.weight,
					0,
				),
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined,
				headerMutationConfig: headerMutationConfig || undefined,
			} as RouteRule;
		} else {
			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: "redirect",
				hostRedirect: hostRedirect || undefined,
				pathRedirect: pathRedirect || undefined,
				responseCode,
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined,
				headerMutationConfig: headerMutationConfig || undefined,
			} as RouteRule;
		}

		const newCluster =
			actionType === "forward" && clusterConfig.mode === "new"
				? clusterConfig
				: null;
		onSave(savedRoute, newCluster);
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onCancel();
		}
	}

	function handleBackdropKeydown(event: KeyboardEvent) {
		if (event.key === "Escape") {
			onCancel();
		}
	}
</script>

{#if show}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		aria-labelledby="route-editor-title"
		onclick={handleBackdropClick}
		onkeydown={handleBackdropKeydown}
	>
		<div
			class="bg-white rounded-lg shadow-xl w-full max-w-2xl mx-4 max-h-[90vh] overflow-y-auto"
			role="document"
			onclick={(e) => e.stopPropagation()}
			onkeydown={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div
				class="px-6 py-4 border-b border-gray-200 flex items-center justify-between"
			>
				<h2 id="route-editor-title" class="text-lg font-semibold text-gray-900">
					{route ? "Edit Route" : "Add Route"} to {domainName}
				</h2>
				<button
					type="button"
					onclick={onCancel}
					class="p-1 text-gray-400 hover:text-gray-600 rounded"
					aria-label="Close dialog"
				>
					<svg
						class="h-6 w-6"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
						aria-hidden="true"
					>
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
						<label
							for="route-method"
							class="block text-sm font-medium text-gray-700 mb-1"
							>Method</label
						>
						<select
							id="route-method"
							bind:value={method}
							class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						>
							{#each HTTP_METHODS as m}
								<option value={m}
									>{m === "*" ? "ANY" : m}</option
								>
							{/each}
						</select>
					</div>
					<div class="flex-1">
						<label
							for="route-path"
							class="block text-sm font-medium text-gray-700 mb-1"
							>Path</label
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
						<label
							for="route-match"
							class="block text-sm font-medium text-gray-700 mb-1"
							>Match</label
						>
						<select
							id="route-match"
							bind:value={pathType}
							class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						>
							{#each PATH_MATCH_TYPES as pt}
								<option value={pt.value}>{pt.label}</option>
							{/each}
						</select>
					</div>
				</div>

				<!-- Action Type Tabs -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2"
						>Route Action</label
					>
					<div class="flex border-b border-gray-200" role="tablist">
						{#each ROUTE_ACTION_TYPES as at}
							<button
								type="button"
								role="tab"
								aria-selected={actionType === at.value}
								onclick={() => (actionType = at.value)}
								class="px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors {actionType ===
								at.value
									? 'text-blue-600 border-blue-600'
									: 'text-gray-500 border-transparent hover:text-gray-700 hover:border-gray-300'}"
							>
								{at.label}
							</button>
						{/each}
					</div>

					<!-- Tab Content -->
					<div class="pt-4 space-y-4" role="tabpanel">
						{#if actionType === "forward"}
							<!-- Forward Sub-tabs -->
							<div
								class="flex gap-4 border-b border-gray-100 mb-4"
								role="tablist"
							>
								<button
									type="button"
									role="tab"
									aria-selected={forwardSubTab === "target"}
									onclick={() => (forwardSubTab = "target")}
									class="pb-2 text-sm font-medium transition-colors {forwardSubTab ===
									'target'
										? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
										: 'text-gray-500 hover:text-gray-700'}"
								>
									Target
								</button>
								<button
									type="button"
									role="tab"
									aria-selected={forwardSubTab === "resilience"}
									onclick={() =>
										(forwardSubTab = "resilience")}
									class="pb-2 text-sm font-medium transition-colors {forwardSubTab ===
									'resilience'
										? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
										: 'text-gray-500 hover:text-gray-700'}"
								>
									Resilience
									{#if retryEnabled}
										<span
											class="ml-1 inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-700"
										>
											Retry
										</span>
									{/if}
								</button>
							</div>

							{#if forwardSubTab === "target"}
								<!-- Target Sub-tab Content -->
								<div role="tabpanel">
									<label
										class="block text-sm font-medium text-gray-700 mb-2"
										>Target Cluster</label
									>
									<ClusterSelector
										{clusters}
										config={clusterConfig}
										onConfigChange={(c) =>
											(clusterConfig = c)}
									/>
								</div>

								<!-- Timeout -->
								<div>
									<label
										for="route-timeout"
										class="block text-sm font-medium text-gray-700 mb-1"
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

								<!-- Path Rewrite Section -->
								<PathRewriteForm
									{pathType}
									bind:prefixRewrite
									bind:templateRewrite
								/>
							{:else}
								<!-- Resilience Sub-tab Content -->
								<div role="tabpanel">
									<RetryPolicyForm
										bind:retryEnabled
										bind:maxRetries={retryMaxRetries}
										bind:preset={retryPreset}
										bind:customConditions={retryOnCustom}
										bind:perTryTimeout={retryPerTryTimeout}
										bind:showBackoff
										bind:backoffBaseInterval
										bind:backoffMaxInterval
									/>
								</div>
							{/if}
						{:else if actionType === "weighted"}
							<!-- Weighted Tab Content -->
							<WeightedClusterForm
								{clusters}
								bind:weightedClusters
							/>
						{:else if actionType === "redirect"}
							<!-- Redirect Tab Content -->
							<RedirectForm
								bind:hostRedirect
								bind:pathRedirect
								bind:responseCode
							/>
						{/if}
					</div>
				</div>

				<!-- Advanced Options (collapsible) -->
				<div class="border-t border-gray-200 pt-4">
					<button
						type="button"
						onclick={() => (showAdvanced = !showAdvanced)}
						aria-expanded={showAdvanced}
						class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
					>
						<svg
							class="h-4 w-4 transition-transform {showAdvanced
								? 'rotate-90'
								: ''}"
							fill="none"
							stroke="currentColor"
							viewBox="0 0 24 24"
							aria-hidden="true"
						>
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M9 5l7 7-7 7"
							/>
						</svg>
						Advanced Matching (Headers, Query Params)
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
						</div>
					{/if}
				</div>

				<!-- Filters (collapsible) -->
				<div class="border-t border-gray-200 pt-4">
					<button
						type="button"
						onclick={() => (showFilters = !showFilters)}
						aria-expanded={showFilters}
						class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
					>
						<svg
							class="h-4 w-4 transition-transform {showFilters
								? 'rotate-90'
								: ''}"
							fill="none"
							stroke="currentColor"
							viewBox="0 0 24 24"
							aria-hidden="true"
						>
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M9 5l7 7-7 7"
							/>
						</svg>
						Filters (Header Mutation)
					</button>

					{#if showFilters}
						<div class="mt-4 pl-6">
							<HeaderMutationPerRouteForm
								config={headerMutationConfig}
								onUpdate={(config) =>
									(headerMutationConfig = config)}
							/>
						</div>
					{/if}
				</div>
			</div>

			<!-- Footer -->
			<div
				class="px-6 py-4 border-t border-gray-200 flex justify-end gap-3"
			>
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
					disabled={actionType === "redirect" &&
						!hostRedirect &&
						!pathRedirect}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
				>
					{route ? "Save Changes" : "Add Route"}
				</button>
			</div>
		</div>
	</div>
{/if}
