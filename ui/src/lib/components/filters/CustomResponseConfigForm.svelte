<script lang="ts">
	import type { CustomResponseConfig, ResponseMatcherRule, StatusCodeMatcher, LocalResponsePolicy } from '$lib/api/types';
	import { Info, Plus, Trash2, ChevronDown, ChevronUp } from 'lucide-svelte';

	interface Props {
		config: CustomResponseConfig;
		onConfigChange: (config: CustomResponseConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Initialize state from config
	let matchers = $state<ResponseMatcherRule[]>(config.matchers.length > 0 ? [...config.matchers] : []);

	// Track which rules are expanded
	let expandedRules = $state<Set<number>>(new Set([0]));

	// Update parent when values change
	function updateParent() {
		onConfigChange({
			matchers: matchers
		});
	}

	// Add a new matcher rule
	function addRule() {
		const newRule: ResponseMatcherRule = {
			status_code: { type: 'exact', code: 500 },
			response: {
				status_code: undefined,
				body: '{"error": "Internal Server Error"}',
				headers: { 'content-type': 'application/json' }
			}
		};
		matchers = [...matchers, newRule];
		expandedRules.add(matchers.length - 1);
		updateParent();
	}

	// Remove a matcher rule
	function removeRule(index: number) {
		matchers = matchers.filter((_, i) => i !== index);
		expandedRules.delete(index);
		updateParent();
	}

	// Toggle rule expansion
	function toggleRule(index: number) {
		if (expandedRules.has(index)) {
			expandedRules.delete(index);
		} else {
			expandedRules.add(index);
		}
		expandedRules = new Set(expandedRules);
	}

	// Update a specific rule
	function updateRule(index: number, rule: ResponseMatcherRule) {
		matchers[index] = rule;
		matchers = [...matchers];
		updateParent();
	}

	// Update match type for a rule
	function updateMatchType(index: number, type: StatusCodeMatcher['type']) {
		const rule = matchers[index];
		let newMatcher: StatusCodeMatcher;

		switch (type) {
			case 'exact':
				newMatcher = { type: 'exact', code: 500 };
				break;
			case 'range':
				newMatcher = { type: 'range', min: 500, max: 599 };
				break;
			case 'list':
				newMatcher = { type: 'list', codes: [500, 502, 503] };
				break;
		}

		updateRule(index, { ...rule, status_code: newMatcher });
	}

	// Get display text for a matcher
	function getMatcherDisplay(matcher: StatusCodeMatcher): string {
		switch (matcher.type) {
			case 'exact':
				return `Status ${matcher.code}`;
			case 'range':
				return `Status ${matcher.min}-${matcher.max}`;
			case 'list':
				return `Status [${matcher.codes.join(', ')}]`;
		}
	}

	// Parse comma-separated codes
	function parseCodes(value: string): number[] {
		return value
			.split(',')
			.map(s => parseInt(s.trim(), 10))
			.filter(n => !isNaN(n));
	}

	// Add a header to response
	function addHeader(index: number) {
		const rule = matchers[index];
		const headers = { ...(rule.response.headers || {}), '': '' };
		updateRule(index, { ...rule, response: { ...rule.response, headers } });
	}

	// Update a header key/value
	function updateHeader(ruleIndex: number, oldKey: string, newKey: string, value: string) {
		const rule = matchers[ruleIndex];
		const headers = { ...(rule.response.headers || {}) };

		if (oldKey !== newKey) {
			delete headers[oldKey];
		}
		if (newKey) {
			headers[newKey] = value;
		}

		updateRule(ruleIndex, { ...rule, response: { ...rule.response, headers } });
	}

	// Remove a header
	function removeHeader(ruleIndex: number, key: string) {
		const rule = matchers[ruleIndex];
		const headers = { ...(rule.response.headers || {}) };
		delete headers[key];
		updateRule(ruleIndex, { ...rule, response: { ...rule.response, headers } });
	}
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Custom Response Filter</p>
				<p class="mt-1">
					Define custom responses based on upstream status codes. Use this to transform
					error responses into user-friendly messages or standardized API error formats.
				</p>
			</div>
		</div>
	</div>

	<!-- Matcher Rules List -->
	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<h3 class="text-sm font-medium text-gray-900">Response Matcher Rules</h3>
			<button
				type="button"
				onclick={addRule}
				class="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-blue-600 hover:text-blue-700 hover:bg-blue-50 rounded-md transition-colors"
			>
				<Plus class="h-4 w-4" />
				Add Rule
			</button>
		</div>

		{#if matchers.length === 0}
			<div class="text-center py-8 bg-gray-50 rounded-lg border border-dashed border-gray-300">
				<p class="text-sm text-gray-500">No matcher rules configured.</p>
				<button
					type="button"
					onclick={addRule}
					class="mt-2 text-sm text-blue-600 hover:text-blue-700"
				>
					Add your first rule
				</button>
			</div>
		{:else}
			<div class="space-y-3">
				{#each matchers as rule, index}
					<div class="border border-gray-200 rounded-lg overflow-hidden">
						<!-- Rule Header -->
						<div class="flex items-center justify-between bg-gray-50 px-4 py-2">
							<button
								type="button"
								onclick={() => toggleRule(index)}
								class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
							>
								{#if expandedRules.has(index)}
									<ChevronUp class="h-4 w-4" />
								{:else}
									<ChevronDown class="h-4 w-4" />
								{/if}
								<span>Rule {index + 1}: {getMatcherDisplay(rule.status_code)}</span>
							</button>
							<button
								type="button"
								onclick={() => removeRule(index)}
								class="p-1 text-red-500 hover:text-red-700 hover:bg-red-50 rounded transition-colors"
								title="Remove rule"
							>
								<Trash2 class="h-4 w-4" />
							</button>
						</div>

						<!-- Rule Content (Collapsible) -->
						{#if expandedRules.has(index)}
							<div class="p-4 space-y-4">
								<!-- Status Code Matcher -->
								<div class="space-y-3">
									<label class="block text-sm font-medium text-gray-700">
										Match Status Code
									</label>

									<div class="flex gap-3">
										<select
											value={rule.status_code.type}
											onchange={(e) => updateMatchType(index, (e.target as HTMLSelectElement).value as StatusCodeMatcher['type'])}
											class="px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
										>
											<option value="exact">Exact Code</option>
											<option value="range">Code Range</option>
											<option value="list">Code List</option>
										</select>

										{#if rule.status_code.type === 'exact'}
											<input
												type="number"
												min="100"
												max="599"
												value={rule.status_code.code}
												oninput={(e) => {
													const code = parseInt((e.target as HTMLInputElement).value, 10);
													if (!isNaN(code)) {
														updateRule(index, { ...rule, status_code: { type: 'exact', code } });
													}
												}}
												class="w-24 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
												placeholder="500"
											/>
										{:else if rule.status_code.type === 'range'}
											<div class="flex items-center gap-2">
												<input
													type="number"
													min="100"
													max="599"
													value={rule.status_code.min}
													oninput={(e) => {
														const min = parseInt((e.target as HTMLInputElement).value, 10);
														if (!isNaN(min) && rule.status_code.type === 'range') {
															updateRule(index, { ...rule, status_code: { type: 'range', min, max: rule.status_code.max } });
														}
													}}
													class="w-20 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
													placeholder="500"
												/>
												<span class="text-gray-500">to</span>
												<input
													type="number"
													min="100"
													max="599"
													value={rule.status_code.max}
													oninput={(e) => {
														const max = parseInt((e.target as HTMLInputElement).value, 10);
														if (!isNaN(max) && rule.status_code.type === 'range') {
															updateRule(index, { ...rule, status_code: { type: 'range', min: rule.status_code.min, max } });
														}
													}}
													class="w-20 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
													placeholder="599"
												/>
											</div>
										{:else if rule.status_code.type === 'list'}
											<input
												type="text"
												value={rule.status_code.codes.join(', ')}
												oninput={(e) => {
													const codes = parseCodes((e.target as HTMLInputElement).value);
													updateRule(index, { ...rule, status_code: { type: 'list', codes } });
												}}
												class="flex-1 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
												placeholder="500, 502, 503, 504"
											/>
										{/if}
									</div>
									<p class="text-xs text-gray-500">
										{#if rule.status_code.type === 'exact'}
											Match a single specific status code
										{:else if rule.status_code.type === 'range'}
											Match any status code in the inclusive range
										{:else}
											Match any of the specified status codes (comma-separated)
										{/if}
									</p>
								</div>

								<!-- Response Configuration -->
								<div class="border-t border-gray-200 pt-4 space-y-4">
									<h4 class="text-sm font-medium text-gray-700">Response Configuration</h4>

									<!-- Override Status Code -->
									<div>
										<label class="block text-sm font-medium text-gray-600 mb-1">
											Override Status Code (optional)
										</label>
										<input
											type="number"
											min="100"
											max="599"
											value={rule.response.status_code ?? ''}
											oninput={(e) => {
												const value = (e.target as HTMLInputElement).value;
												const code = value ? parseInt(value, 10) : undefined;
												updateRule(index, { ...rule, response: { ...rule.response, status_code: code } });
											}}
											class="w-24 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
											placeholder="Keep original"
										/>
										<p class="text-xs text-gray-500 mt-1">
											Leave empty to keep the original status code
										</p>
									</div>

									<!-- Response Body -->
									<div>
										<label class="block text-sm font-medium text-gray-600 mb-1">
											Response Body
										</label>
										<textarea
											value={rule.response.body ?? ''}
											oninput={(e) => {
												const body = (e.target as HTMLTextAreaElement).value || undefined;
												updateRule(index, { ...rule, response: { ...rule.response, body } });
											}}
											rows="4"
											class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm font-mono focus:outline-none focus:ring-2 focus:ring-blue-500"
											placeholder={'{"error": "Something went wrong", "message": "Please try again later"}'}
										></textarea>
										<p class="text-xs text-gray-500 mt-1">
											Custom response body (typically JSON for API responses)
										</p>
									</div>

									<!-- Response Headers -->
									<div>
										<div class="flex items-center justify-between mb-2">
											<label class="block text-sm font-medium text-gray-600">
												Response Headers
											</label>
											<button
												type="button"
												onclick={() => addHeader(index)}
												class="text-xs text-blue-600 hover:text-blue-700"
											>
												+ Add Header
											</button>
										</div>

										{#if rule.response.headers && Object.keys(rule.response.headers).length > 0}
											<div class="space-y-2">
												{#each Object.entries(rule.response.headers) as [key, value]}
													<div class="flex gap-2 items-center">
														<input
															type="text"
															value={key}
															oninput={(e) => updateHeader(index, key, (e.target as HTMLInputElement).value, value)}
															class="flex-1 px-2 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
															placeholder="Header name"
														/>
														<span class="text-gray-400">:</span>
														<input
															type="text"
															value={value}
															oninput={(e) => updateHeader(index, key, key, (e.target as HTMLInputElement).value)}
															class="flex-1 px-2 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
															placeholder="Header value"
														/>
														<button
															type="button"
															onclick={() => removeHeader(index, key)}
															class="p-1 text-gray-400 hover:text-red-500 transition-colors"
														>
															<Trash2 class="h-4 w-4" />
														</button>
													</div>
												{/each}
											</div>
										{:else}
											<p class="text-xs text-gray-500 italic">No headers configured</p>
										{/if}
									</div>
								</div>
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Preview of configured rules -->
	{#if matchers.length > 0}
		<div class="p-4 bg-gray-50 rounded-lg border border-gray-200">
			<h4 class="text-sm font-medium text-gray-700 mb-2">Configuration Summary</h4>
			<ul class="text-xs text-gray-600 space-y-1">
				{#each matchers as rule, index}
					<li class="flex items-center gap-2">
						<span class="w-4 h-4 rounded-full bg-blue-100 text-blue-600 flex items-center justify-center text-[10px] font-medium">
							{index + 1}
						</span>
						<span>
							When status is <strong>{getMatcherDisplay(rule.status_code)}</strong>
							{#if rule.response.status_code}
								&rarr; respond with <strong>{rule.response.status_code}</strong>
							{:else}
								&rarr; replace response body
							{/if}
						</span>
					</li>
				{/each}
			</ul>
		</div>
	{/if}
</div>
