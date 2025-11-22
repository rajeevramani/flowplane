<script lang="ts">
	import Badge from './Badge.svelte';

	export interface RouteDetail {
		name: string;
		method: string;
		path: string;
		matchType: 'exact' | 'prefix' | 'template' | 'regex';
		cluster: string;
		timeout?: number;
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
				return 'exact';
			case 'prefix':
				return 'prefix';
			case 'template':
				return 'template';
			case 'regex':
				return 'regex';
			default:
				return matchType;
		}
	}

	function truncateCluster(cluster: string, maxLength: number = 40): string {
		if (cluster.length <= maxLength) return cluster;
		return cluster.substring(0, maxLength) + '...';
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
				<th class="text-left text-xs font-medium text-gray-500 uppercase tracking-wider pb-2 w-20">Timeout</th>
			</tr>
		</thead>
		<tbody class="divide-y divide-gray-200">
			{#each routes as route}
				<tr class="hover:bg-gray-100">
					<td class="py-2 pr-4">
						<Badge variant={getMethodBadgeVariant(route.method)} size="sm">
							{route.method.toUpperCase()}
						</Badge>
					</td>
					<td class="py-2 pr-4">
						<code class="text-sm text-gray-800 bg-white px-2 py-0.5 rounded border border-gray-200 font-mono">
							{route.path || '/'}
						</code>
					</td>
					<td class="py-2 pr-4">
						<span class="text-xs text-gray-500 bg-gray-200 px-2 py-0.5 rounded">
							{getMatchTypeLabel(route.matchType)}
						</span>
					</td>
					<td class="py-2 pr-4">
						<span class="text-sm text-gray-600" title={route.cluster}>
							{truncateCluster(route.cluster)}
						</span>
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
