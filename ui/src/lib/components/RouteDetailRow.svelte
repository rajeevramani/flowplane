<script lang="ts">
	import Badge from './Badge.svelte';
	import Tooltip from './Tooltip.svelte';

	export interface RetryPolicy {
		numRetries?: number;
		retryOn?: string;
		perTryTimeout?: string;
		retryBackOff?: {
			baseInterval?: string;
			maxInterval?: string;
		};
	}

	export interface RouteDetail {
		name: string;
		method: string;
		path: string;
		matchType: 'exact' | 'prefix' | 'template' | 'regex';
		cluster: string;
		timeout?: number;
		prefixRewrite?: string;
		templateRewrite?: string;
		retryPolicy?: RetryPolicy;
	}

	interface Props {
		routes: RouteDetail[];
	}

	let { routes }: Props = $props();

	function getMethodBadgeVariant(method: string): 'green' | 'blue' | 'yellow' | 'red' | 'gray' {
		switch (method.toUpperCase()) {
			case 'GET':
				return 'green';
			case 'POST':
				return 'blue';
			case 'PUT':
			case 'PATCH':
				return 'yellow';
			case 'DELETE':
				return 'red';
			default:
				return 'gray';
		}
	}

	function getMatchTypeLabel(matchType: string): string {
		switch (matchType) {
			case 'exact':
				return 'Exact';
			case 'prefix':
				return 'Prefix';
			case 'template':
				return 'Template';
			case 'regex':
				return 'Regex';
			default:
				return matchType;
		}
	}

	function truncateCluster(cluster: string, maxLength: number = 30): string {
		if (cluster.length <= maxLength) return cluster;
		return cluster.substring(0, maxLength) + '...';
	}

	function formatRewrite(route: RouteDetail): string | null {
		if (route.prefixRewrite) return route.prefixRewrite;
		if (route.templateRewrite) return route.templateRewrite;
		return null;
	}

	function formatRetryOn(retryOn: string | undefined): string {
		if (!retryOn) return '-';
		// Format common retry conditions to be more readable
		return retryOn
			.split(',')
			.map((s) => s.trim())
			.join(', ');
	}

	function formatDuration(duration: string | undefined): string {
		if (!duration) return '-';
		// Parse proto duration format (e.g., "10s", "100ms")
		return duration;
	}

	function formatBackoff(policy: RetryPolicy | undefined): string {
		if (!policy?.retryBackOff) return '-';
		const base = policy.retryBackOff.baseInterval || '25ms';
		const max = policy.retryBackOff.maxInterval || '250ms';
		return `${base} - ${max}`;
	}
</script>

<div class="bg-gray-50 px-6 py-4 border-t border-gray-100">
	<table class="min-w-full">
		<thead>
			<tr>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-20">Method</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2">Path</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-20">Match</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2">Target Cluster</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-40">Rewrite</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-24">Retries</th>
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-20">Timeout</th>
			</tr>
		</thead>
		<tbody class="divide-y divide-gray-200">
			{#each routes as route}
				{@const rewrite = formatRewrite(route)}
				<tr class="hover:bg-gray-100">
					<td class="py-2 pr-4">
						<Badge variant={getMethodBadgeVariant(route.method)} size="sm">
							{route.method === '*' ? 'ANY' : route.method.toUpperCase()}
						</Badge>
					</td>
					<td class="py-2 pr-4">
						<code class="text-sm text-gray-800 bg-white px-2 py-0.5 rounded border border-gray-200 font-mono">
							{route.path || '/'}
						</code>
					</td>
					<td class="py-2 pr-4">
						<span class="text-xs text-gray-600 bg-blue-100 text-blue-700 px-2 py-0.5 rounded font-medium">
							{getMatchTypeLabel(route.matchType)}
						</span>
					</td>
					<td class="py-2 pr-4">
						<span class="text-sm text-gray-600" title={route.cluster}>
							{truncateCluster(route.cluster)}
						</span>
					</td>
					<td class="py-2 pr-4">
						{#if rewrite}
							<span class="text-sm text-gray-600 flex items-center gap-1">
								<span class="text-gray-400">&rarr;</span>
								<code class="font-mono text-xs bg-gray-100 px-1.5 py-0.5 rounded">{rewrite}</code>
							</span>
						{:else}
							<span class="text-gray-400">-</span>
						{/if}
					</td>
					<td class="py-2 pr-4">
						{#if route.retryPolicy && route.retryPolicy.numRetries}
							<Tooltip position="bottom">
								<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-orange-100 text-orange-800 cursor-help">
									{route.retryPolicy.numRetries} retries
								</span>
								{#snippet tooltip()}
									<div class="space-y-2 min-w-[200px]">
										<div class="flex justify-between gap-4">
											<span class="text-gray-500">Max Retries:</span>
											<span class="font-medium text-gray-900">{route.retryPolicy?.numRetries}</span>
										</div>
										<div class="flex justify-between gap-4">
											<span class="text-gray-500">Retry On:</span>
											<span class="font-medium text-gray-900">{formatRetryOn(route.retryPolicy?.retryOn)}</span>
										</div>
										<div class="flex justify-between gap-4">
											<span class="text-gray-500">Per Try Timeout:</span>
											<span class="font-medium text-gray-900">{formatDuration(route.retryPolicy?.perTryTimeout)}</span>
										</div>
										<div class="flex justify-between gap-4">
											<span class="text-gray-500">Backoff:</span>
											<span class="font-medium text-gray-900">{formatBackoff(route.retryPolicy)}</span>
										</div>
									</div>
								{/snippet}
							</Tooltip>
						{:else}
							<span class="text-gray-400">-</span>
						{/if}
					</td>
					<td class="py-2">
						<span class="text-sm text-gray-500">
							{route.timeout ? `${route.timeout}s` : '-'}
						</span>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
