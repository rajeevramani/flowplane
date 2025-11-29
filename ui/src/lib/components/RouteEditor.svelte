<script lang="ts">
	import type {
		PathMatchType,
		HeaderMatchDefinition,
		QueryParameterMatchDefinition,
		ClusterResponse,
		EndpointRequest
	} from '$lib/api/types';
	import type { RouteRule, RouteActionType, WeightedCluster, RetryPolicy } from './EditableRoutesTable.svelte';
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
	const actionTypes: { value: RouteActionType; label: string }[] = [
		{ value: 'forward', label: 'Forward' },
		{ value: 'weighted', label: 'Weighted' },
		{ value: 'redirect', label: 'Redirect' }
	];
	const redirectCodes = [
		{ value: 301, label: '301 - Permanent' },
		{ value: 302, label: '302 - Found' },
		{ value: 303, label: '303 - See Other' },
		{ value: 307, label: '307 - Temporary' },
		{ value: 308, label: '308 - Permanent Redirect' }
	];

	// Retry presets
	type RetryPreset = '5xx' | 'connection' | 'gateway' | 'all' | 'custom';
	const retryPresets: { value: RetryPreset; label: string; conditions: string[] }[] = [
		{ value: '5xx', label: '5xx Server Errors', conditions: ['5xx'] },
		{ value: 'connection', label: 'Connection Failures', conditions: ['reset', 'connect-failure'] },
		{ value: 'gateway', label: 'Gateway Errors', conditions: ['gateway-error'] },
		{ value: 'all', label: 'All Retriable', conditions: ['5xx', 'reset', 'connect-failure', 'retriable-4xx', 'refused-stream', 'gateway-error'] },
		{ value: 'custom', label: 'Custom...', conditions: [] }
	];
	const retryConditions = [
		{ value: '5xx', label: '5xx Server Errors' },
		{ value: 'reset', label: 'Connection Reset' },
		{ value: 'connect-failure', label: 'Connect Failure' },
		{ value: 'retriable-4xx', label: 'Retriable 4xx' },
		{ value: 'refused-stream', label: 'Refused Stream' },
		{ value: 'gateway-error', label: 'Gateway Error' }
	];
	type ForwardSubTab = 'target' | 'resilience';

	// Form state
	let method = $state('GET');
	let path = $state('/');
	let pathType = $state<PathMatchType>('prefix');
	let actionType = $state<RouteActionType>('forward');
	let headers = $state<HeaderMatchDefinition[]>([]);
	let queryParams = $state<QueryParameterMatchDefinition[]>([]);
	// Forward action state
	let timeoutSeconds = $state(15);
	let clusterConfig = $state<ClusterConfig>({ mode: 'existing', existingClusterName: null });
	let prefixRewrite = $state('');
	let templateRewrite = $state('');
	// Weighted action state
	let weightedClusters = $state<WeightedCluster[]>([{ name: '', weight: 100 }]);
	// Redirect action state
	let hostRedirect = $state('');
	let pathRedirect = $state('');
	let responseCode = $state(302);
	// Retry policy state (Forward action only)
	let retryEnabled = $state(false);
	let retryMaxRetries = $state(3);
	let retryPreset = $state<RetryPreset>('5xx');
	let retryOnCustom = $state<string[]>([]);
	let retryPerTryTimeout = $state<number | null>(null);
	let showBackoff = $state(false);
	let backoffBaseInterval = $state(100);
	let backoffMaxInterval = $state(1000);
	// UI state
	let showAdvanced = $state(false);
	let forwardSubTab = $state<ForwardSubTab>('target');

	// Derived: check if rewrite is allowed based on path type
	let canUsePrefixRewrite = $derived(pathType === 'prefix' || pathType === 'exact');
	let canUseTemplateRewrite = $derived(pathType === 'template');

	// Derived: calculate total weight
	let totalWeight = $derived(weightedClusters.reduce((sum, c) => sum + (c.weight || 0), 0));

	// Derived: get actual retry conditions based on preset or custom
	let actualRetryConditions = $derived(() => {
		if (retryPreset === 'custom') {
			return retryOnCustom;
		}
		const preset = retryPresets.find(p => p.value === retryPreset);
		return preset?.conditions || [];
	});

	// Helper: detect preset from conditions array
	function detectPresetFromConditions(conditions: string[]): RetryPreset {
		const sorted = [...conditions].sort();
		for (const preset of retryPresets) {
			if (preset.value === 'custom') continue;
			const presetSorted = [...preset.conditions].sort();
			if (sorted.length === presetSorted.length &&
				sorted.every((c, i) => c === presetSorted[i])) {
				return preset.value;
			}
		}
		return 'custom';
	}

	// Helper: toggle retry condition in custom mode
	function toggleRetryCondition(condition: string) {
		if (retryOnCustom.includes(condition)) {
			retryOnCustom = retryOnCustom.filter(c => c !== condition);
		} else {
			retryOnCustom = [...retryOnCustom, condition];
		}
	}

	// Reset form when route changes
	$effect(() => {
		if (show) {
			if (route) {
				// Editing existing route
				method = route.method;
				path = route.path;
				pathType = route.pathType;
				actionType = route.actionType || 'forward';
				headers = [...(route.headers || [])];
				queryParams = [...(route.queryParams || [])];
				timeoutSeconds = route.timeoutSeconds || 15;
				clusterConfig = { mode: 'existing', existingClusterName: route.cluster || null };
				prefixRewrite = route.prefixRewrite || '';
				templateRewrite = route.templateRewrite || '';
				weightedClusters = route.weightedClusters?.length
					? [...route.weightedClusters]
					: [{ name: '', weight: 100 }];
				hostRedirect = route.hostRedirect || '';
				pathRedirect = route.pathRedirect || '';
				responseCode = route.responseCode || 302;
				showAdvanced = (route.headers?.length || 0) > 0 || (route.queryParams?.length || 0) > 0;
				// Load retry policy
				if (route.retryPolicy) {
					retryEnabled = true;
					retryMaxRetries = route.retryPolicy.maxRetries;
					const detectedPreset = detectPresetFromConditions(route.retryPolicy.retryOn);
					retryPreset = detectedPreset;
					if (detectedPreset === 'custom') {
						retryOnCustom = [...route.retryPolicy.retryOn];
					}
					retryPerTryTimeout = route.retryPolicy.perTryTimeoutSeconds || null;
					if (route.retryPolicy.backoff) {
						showBackoff = true;
						backoffBaseInterval = route.retryPolicy.backoff.baseIntervalMs || 100;
						backoffMaxInterval = route.retryPolicy.backoff.maxIntervalMs || 1000;
					} else {
						showBackoff = false;
						backoffBaseInterval = 100;
						backoffMaxInterval = 1000;
					}
				} else {
					retryEnabled = false;
					retryMaxRetries = 3;
					retryPreset = '5xx';
					retryOnCustom = [];
					retryPerTryTimeout = null;
					showBackoff = false;
					backoffBaseInterval = 100;
					backoffMaxInterval = 1000;
				}
				forwardSubTab = 'target';
			} else {
				// Creating new route
				method = 'GET';
				path = '/';
				pathType = 'prefix';
				actionType = 'forward';
				headers = [];
				queryParams = [];
				timeoutSeconds = 15;
				clusterConfig = { mode: 'existing', existingClusterName: null };
				prefixRewrite = '';
				templateRewrite = '';
				weightedClusters = [{ name: '', weight: 100 }];
				hostRedirect = '';
				pathRedirect = '';
				responseCode = 302;
				showAdvanced = false;
				// Reset retry policy
				retryEnabled = false;
				retryMaxRetries = 3;
				retryPreset = '5xx';
				retryOnCustom = [];
				retryPerTryTimeout = null;
				showBackoff = false;
				backoffBaseInterval = 100;
				backoffMaxInterval = 1000;
				forwardSubTab = 'target';
			}
		}
	});

	// Clear invalid rewrite when path type changes
	$effect(() => {
		if (!canUsePrefixRewrite) {
			prefixRewrite = '';
		}
		if (!canUseTemplateRewrite) {
			templateRewrite = '';
		}
	});

	function addWeightedCluster() {
		weightedClusters = [...weightedClusters, { name: '', weight: 0 }];
	}

	function removeWeightedCluster(index: number) {
		if (weightedClusters.length > 1) {
			weightedClusters = weightedClusters.filter((_, i) => i !== index);
		}
	}

	function updateWeightedCluster(index: number, field: 'name' | 'weight', value: string | number) {
		weightedClusters = weightedClusters.map((c, i) => {
			if (i === index) {
				return { ...c, [field]: field === 'weight' ? Number(value) : value };
			}
			return c;
		});
	}

	function handleSave() {
		let savedRoute: RouteRule;

		if (actionType === 'forward') {
			const targetCluster =
				clusterConfig.mode === 'existing'
					? clusterConfig.existingClusterName || ''
					: clusterConfig.newClusterConfig?.name || '';

			// Build retry policy if enabled
			let retryPolicy: RetryPolicy | undefined = undefined;
			if (retryEnabled) {
				const conditions = retryPreset === 'custom'
					? retryOnCustom
					: retryPresets.find(p => p.value === retryPreset)?.conditions || [];

				retryPolicy = {
					maxRetries: retryMaxRetries,
					retryOn: conditions,
					perTryTimeoutSeconds: retryPerTryTimeout || undefined,
					backoff: showBackoff ? {
						baseIntervalMs: backoffBaseInterval,
						maxIntervalMs: backoffMaxInterval
					} : undefined
				};
			}

			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: 'forward',
				cluster: targetCluster,
				prefixRewrite: prefixRewrite || undefined,
				templateRewrite: templateRewrite || undefined,
				timeoutSeconds,
				retryPolicy,
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined
			};
		} else if (actionType === 'weighted') {
			const validClusters = weightedClusters.filter(c => c.name && c.weight > 0);
			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: 'weighted',
				weightedClusters: validClusters,
				totalWeight: validClusters.reduce((sum, c) => sum + c.weight, 0),
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined
			};
		} else {
			savedRoute = {
				id: route?.id || crypto.randomUUID(),
				method,
				path,
				pathType,
				actionType: 'redirect',
				hostRedirect: hostRedirect || undefined,
				pathRedirect: pathRedirect || undefined,
				responseCode,
				headers: headers.length > 0 ? headers : undefined,
				queryParams: queryParams.length > 0 ? queryParams : undefined
			};
		}

		const newCluster = actionType === 'forward' && clusterConfig.mode === 'new' ? clusterConfig : null;
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

				<!-- Action Type Tabs -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">Route Action</label>
					<div class="flex border-b border-gray-200">
						{#each actionTypes as at}
							<button
								type="button"
								onclick={() => actionType = at.value}
								class="px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors {actionType === at.value
									? 'text-blue-600 border-blue-600'
									: 'text-gray-500 border-transparent hover:text-gray-700 hover:border-gray-300'}"
							>
								{at.label}
							</button>
						{/each}
					</div>

					<!-- Tab Content -->
					<div class="pt-4 space-y-4">
						{#if actionType === 'forward'}
							<!-- Forward Sub-tabs -->
							<div class="flex gap-4 border-b border-gray-100 mb-4">
								<button
									type="button"
									onclick={() => forwardSubTab = 'target'}
									class="pb-2 text-sm font-medium transition-colors {forwardSubTab === 'target'
										? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
										: 'text-gray-500 hover:text-gray-700'}"
								>
									Target
								</button>
								<button
									type="button"
									onclick={() => forwardSubTab = 'resilience'}
									class="pb-2 text-sm font-medium transition-colors {forwardSubTab === 'resilience'
										? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
										: 'text-gray-500 hover:text-gray-700'}"
								>
									Resilience
									{#if retryEnabled}
										<span class="ml-1 inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-700">
											Retry
										</span>
									{/if}
								</button>
							</div>

							{#if forwardSubTab === 'target'}
								<!-- Target Sub-tab Content -->
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-2">Target Cluster</label>
									<ClusterSelector
										{clusters}
										config={clusterConfig}
										onConfigChange={(c) => (clusterConfig = c)}
									/>
								</div>

								<!-- Timeout -->
								<div>
									<label for="route-timeout" class="block text-sm font-medium text-gray-700 mb-1"
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
								{#key pathType}
									{#if pathType === 'prefix' || pathType === 'exact'}
										<div class="border-t border-gray-200 pt-4">
											<label class="block text-sm font-medium text-gray-700 mb-2">Path Rewrite (Optional)</label>
											<div>
												<label for="prefix-rewrite" class="block text-sm text-gray-600 mb-1">
													Prefix Rewrite
													<span class="text-xs text-gray-400">(replaces matched prefix)</span>
												</label>
												<input
													id="prefix-rewrite"
													type="text"
													bind:value={prefixRewrite}
													placeholder="/new-prefix"
													class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
												/>
												<p class="mt-1 text-xs text-gray-500">
													E.g., path "/api/v1" with prefix rewrite "/internal" turns "/api/v1/users" into "/internal/users"
												</p>
											</div>
										</div>
									{:else if pathType === 'template'}
										<div class="border-t border-gray-200 pt-4">
											<label class="block text-sm font-medium text-gray-700 mb-2">Path Rewrite (Optional)</label>
											<div>
												<label for="template-rewrite" class="block text-sm text-gray-600 mb-1">
													Template Rewrite
													<span class="text-xs text-gray-400">(uses captured variables)</span>
												</label>
												<input
													id="template-rewrite"
													type="text"
													bind:value={templateRewrite}
													placeholder="/users/{'{'}id{'}'}/profile"
													class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
												/>
												<p class="mt-1 text-xs text-gray-500">
													E.g., template "/users/{'{'}user_id{'}'}" with rewrite "/v2/users/{'{'}user_id{'}'}"
												</p>
											</div>
										</div>
									{:else if pathType === 'regex'}
										<p class="text-sm text-gray-500 italic">
											Path rewrites are not available for regex path matching.
										</p>
									{/if}
								{/key}
							{:else}
								<!-- Resilience Sub-tab Content -->
								<div class="space-y-4">
									<!-- Retry Policy Toggle -->
									<div class="flex items-center gap-3">
										<input
											type="checkbox"
											id="retry-enabled"
											bind:checked={retryEnabled}
											class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
										/>
										<label for="retry-enabled" class="text-sm font-medium text-gray-700">
											Enable Retry Policy
										</label>
									</div>

									{#if retryEnabled}
										<div class="pl-7 space-y-4">
											<!-- Max Retries -->
											<div class="flex items-center gap-4">
												<div>
													<label for="max-retries" class="block text-sm font-medium text-gray-700 mb-1">
														Max Retries
													</label>
													<select
														id="max-retries"
														bind:value={retryMaxRetries}
														class="w-20 rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
													>
														{#each [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] as n}
															<option value={n}>{n}</option>
														{/each}
													</select>
												</div>
											</div>

											<!-- Retry Preset -->
											<div>
												<label for="retry-preset" class="block text-sm font-medium text-gray-700 mb-1">
													Retry On
												</label>
												<select
													id="retry-preset"
													bind:value={retryPreset}
													class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
												>
													{#each retryPresets as preset}
														<option value={preset.value}>{preset.label}</option>
													{/each}
												</select>
											</div>

											<!-- Custom Conditions -->
											{#if retryPreset === 'custom'}
												<div class="bg-gray-50 p-3 rounded-md">
													<label class="block text-sm font-medium text-gray-700 mb-2">
														Select Retry Conditions
													</label>
													<div class="grid grid-cols-2 gap-2">
														{#each retryConditions as condition}
															<label class="flex items-center gap-2 text-sm">
																<input
																	type="checkbox"
																	checked={retryOnCustom.includes(condition.value)}
																	onchange={() => toggleRetryCondition(condition.value)}
																	class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
																/>
																{condition.label}
															</label>
														{/each}
													</div>
													{#if retryOnCustom.length === 0}
														<p class="mt-2 text-xs text-amber-600">
															Select at least one retry condition
														</p>
													{/if}
												</div>
											{/if}

											<!-- Per-Try Timeout -->
											<div>
												<label for="per-try-timeout" class="block text-sm font-medium text-gray-700 mb-1">
													Per-Try Timeout
													<span class="text-xs text-gray-400">(optional, seconds)</span>
												</label>
												<input
													id="per-try-timeout"
													type="number"
													min="1"
													max="300"
													bind:value={retryPerTryTimeout}
													placeholder="Use route timeout"
													class="w-32 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
												/>
											</div>

											<!-- Backoff Settings (Collapsible) -->
											<div class="border-t border-gray-200 pt-4">
												<button
													type="button"
													onclick={() => showBackoff = !showBackoff}
													class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
												>
													<svg
														class="h-4 w-4 transition-transform {showBackoff ? 'rotate-90' : ''}"
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
													Backoff Settings
													<span class="text-xs text-gray-400">(optional)</span>
												</button>

												{#if showBackoff}
													<div class="mt-3 pl-6 space-y-3">
														<div>
															<label for="backoff-base" class="block text-sm text-gray-600 mb-1">
																Base Interval (ms)
															</label>
															<input
																id="backoff-base"
																type="number"
																min="10"
																max="10000"
																bind:value={backoffBaseInterval}
																class="w-28 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
															/>
														</div>
														<div>
															<label for="backoff-max" class="block text-sm text-gray-600 mb-1">
																Max Interval (ms)
															</label>
															<input
																id="backoff-max"
																type="number"
																min="100"
																max="60000"
																bind:value={backoffMaxInterval}
																class="w-28 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
															/>
														</div>
														{#if backoffBaseInterval >= backoffMaxInterval}
															<p class="text-xs text-amber-600">
																Base interval should be less than max interval
															</p>
														{/if}
													</div>
												{/if}
											</div>
										</div>
									{:else}
										<p class="pl-7 text-sm text-gray-500">
											Enable retry policy to configure automatic request retries on failures.
										</p>
									{/if}
								</div>
							{/if}

						{:else if actionType === 'weighted'}
							<!-- Weighted Tab Content -->
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-2">Traffic Distribution</label>
								<div class="space-y-2">
									{#each weightedClusters as wc, index}
										<div class="flex items-center gap-2">
											<select
												bind:value={wc.name}
												onchange={(e) => updateWeightedCluster(index, 'name', (e.target as HTMLSelectElement).value)}
												class="flex-1 rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
											>
												<option value="">Select cluster...</option>
												{#each clusters as c}
													<option value={c.name}>{c.name}</option>
												{/each}
											</select>
											<div class="flex items-center gap-1 w-24">
												<input
													type="number"
													min="0"
													max="100"
													bind:value={wc.weight}
													oninput={(e) => updateWeightedCluster(index, 'weight', (e.target as HTMLInputElement).value)}
													class="w-16 rounded-md border border-gray-300 px-2 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
												/>
												<span class="text-sm text-gray-500">%</span>
											</div>
											{#if weightedClusters.length > 1}
												<button
													type="button"
													onclick={() => removeWeightedCluster(index)}
													class="p-1 text-gray-400 hover:text-red-600 rounded"
													title="Remove cluster"
												>
													<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
														<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
													</svg>
												</button>
											{/if}
										</div>
									{/each}
								</div>
								<div class="flex items-center justify-between mt-3">
									<button
										type="button"
										onclick={addWeightedCluster}
										class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800"
									>
										<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
										</svg>
										Add Cluster
									</button>
									<span class="text-sm {totalWeight === 100 ? 'text-green-600' : 'text-amber-600'}">
										Total: {totalWeight}%
										{#if totalWeight !== 100}
											<span class="text-xs">(should be 100%)</span>
										{/if}
									</span>
								</div>
							</div>

						{:else if actionType === 'redirect'}
							<!-- Redirect Tab Content -->
							<div class="space-y-4">
								<div>
									<label for="host-redirect" class="block text-sm font-medium text-gray-700 mb-1">
										Host Redirect
										<span class="text-xs text-gray-400">(optional)</span>
									</label>
									<input
										id="host-redirect"
										type="text"
										bind:value={hostRedirect}
										placeholder="example.com"
										class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
									/>
								</div>
								<div>
									<label for="path-redirect" class="block text-sm font-medium text-gray-700 mb-1">
										Path Redirect
										<span class="text-xs text-gray-400">(optional)</span>
									</label>
									<input
										id="path-redirect"
										type="text"
										bind:value={pathRedirect}
										placeholder="/new-path"
										class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
									/>
								</div>
								<div>
									<label for="response-code" class="block text-sm font-medium text-gray-700 mb-1">
										Response Code
									</label>
									<select
										id="response-code"
										bind:value={responseCode}
										class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
									>
										{#each redirectCodes as rc}
											<option value={rc.value}>{rc.label}</option>
										{/each}
									</select>
								</div>
								{#if !hostRedirect && !pathRedirect}
									<p class="text-sm text-amber-600">
										Please specify at least a host or path redirect.
									</p>
								{/if}
							</div>
						{/if}
					</div>
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
					disabled={actionType === 'redirect' && !hostRedirect && !pathRedirect}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
				>
					{route ? 'Save Changes' : 'Add Route'}
				</button>
			</div>
		</div>
	</div>
{/if}
