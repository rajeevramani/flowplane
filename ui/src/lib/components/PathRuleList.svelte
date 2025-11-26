<script lang="ts">
	import type { PathMatchType, HeaderMatchDefinition, QueryParameterMatchDefinition } from '$lib/api/types';
	import HeaderMatcherList from './HeaderMatcherList.svelte';
	import QueryParamMatcherList from './QueryParamMatcherList.svelte';

	export interface PathRule {
		id: string;
		method: string;
		path: string;
		pathType: PathMatchType;
		headers: HeaderMatchDefinition[];
		queryParams: QueryParameterMatchDefinition[];
	}

	interface Props {
		rules: PathRule[];
		onRulesChange: (rules: PathRule[]) => void;
	}

	let { rules, onRulesChange }: Props = $props();

	let expandedRules = $state<Set<string>>(new Set());

	const httpMethods = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', '*'];
	const pathTypes: { value: PathMatchType; label: string }[] = [
		{ value: 'prefix', label: 'Prefix' },
		{ value: 'exact', label: 'Exact' },
		{ value: 'regex', label: 'Regex' },
		{ value: 'template', label: 'Template' }
	];

	function addRule() {
		const newRule: PathRule = {
			id: crypto.randomUUID(),
			method: 'GET',
			path: '/',
			pathType: 'prefix',
			headers: [],
			queryParams: []
		};
		onRulesChange([...rules, newRule]);
	}

	function removeRule(id: string) {
		if (rules.length > 1) {
			onRulesChange(rules.filter((r) => r.id !== id));
			expandedRules.delete(id);
		}
	}

	function updateRule(id: string, field: keyof PathRule, value: unknown) {
		onRulesChange(
			rules.map((rule) => {
				if (rule.id === id) {
					return { ...rule, [field]: value };
				}
				return rule;
			})
		);
	}

	function toggleExpand(id: string) {
		if (expandedRules.has(id)) {
			expandedRules.delete(id);
		} else {
			expandedRules.add(id);
		}
		expandedRules = new Set(expandedRules);
	}

	function handleHeadersChange(id: string, headers: HeaderMatchDefinition[]) {
		updateRule(id, 'headers', headers);
	}

	function handleQueryParamsChange(id: string, queryParams: QueryParameterMatchDefinition[]) {
		updateRule(id, 'queryParams', queryParams);
	}
</script>

<div class="space-y-3">
	<label class="block text-sm font-medium text-gray-700">Path Rules</label>

	{#each rules as rule}
		<div class="rounded-lg border border-gray-200 bg-gray-50">
			<div class="flex items-center gap-2 p-3">
				<select
					value={rule.method}
					onchange={(e) => updateRule(rule.id, 'method', (e.target as HTMLSelectElement).value)}
					class="w-28 rounded-md border border-gray-300 bg-white px-2 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				>
					{#each httpMethods as method}
						<option value={method}>{method === '*' ? 'ANY' : method}</option>
					{/each}
				</select>

				<input
					type="text"
					placeholder="/path"
					value={rule.path}
					oninput={(e) => updateRule(rule.id, 'path', (e.target as HTMLInputElement).value)}
					class="flex-1 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>

				<select
					value={rule.pathType}
					onchange={(e) =>
						updateRule(rule.id, 'pathType', (e.target as HTMLSelectElement).value as PathMatchType)}
					class="w-28 rounded-md border border-gray-300 bg-white px-2 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				>
					{#each pathTypes as pt}
						<option value={pt.value}>{pt.label}</option>
					{/each}
				</select>

				<button
					type="button"
					onclick={() => toggleExpand(rule.id)}
					class="rounded-md p-2 text-gray-500 hover:bg-gray-200"
					title={expandedRules.has(rule.id) ? 'Collapse' : 'Expand advanced options'}
				>
					<svg
						class="h-5 w-5 transition-transform {expandedRules.has(rule.id)
							? 'rotate-180'
							: ''}"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
					>
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M19 9l-7 7-7-7"
						/>
					</svg>
				</button>

				<button
					type="button"
					onclick={() => removeRule(rule.id)}
					disabled={rules.length <= 1}
					class="rounded-md p-2 text-gray-400 hover:bg-gray-100 hover:text-red-500 disabled:cursor-not-allowed disabled:opacity-50"
					title="Remove rule"
				>
					<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M6 18L18 6M6 6l12 12"
						/>
					</svg>
				</button>
			</div>

			{#if expandedRules.has(rule.id)}
				<div class="space-y-4 border-t border-gray-200 bg-white p-4">
					<div>
						<HeaderMatcherList
							headers={rule.headers}
							onHeadersChange={(headers) => handleHeadersChange(rule.id, headers)}
						/>
					</div>
					<div>
						<QueryParamMatcherList
							params={rule.queryParams}
							onParamsChange={(params) => handleQueryParamsChange(rule.id, params)}
						/>
					</div>
				</div>
			{/if}
		</div>
	{/each}

	<button
		type="button"
		onclick={addRule}
		class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
	>
		<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
		</svg>
		Add Path Rule
	</button>
</div>
