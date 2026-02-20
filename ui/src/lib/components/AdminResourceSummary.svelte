<script lang="ts">
	import type { AdminResourceSummary } from '$lib/api/types';

	interface Props {
		summary: AdminResourceSummary;
		highlightResource?: string;
	}

	let { summary, highlightResource }: Props = $props();

	interface StatCardData {
		title: string;
		value: number;
		highlighted: boolean;
	}

	let statCards = $derived<StatCardData[]>([
		{ title: 'Organizations', value: summary.orgs.length, highlighted: highlightResource === 'organizations' },
		{ title: 'Teams', value: summary.totals.teams, highlighted: highlightResource === 'teams' },
		{ title: 'Clusters', value: summary.totals.clusters, highlighted: highlightResource === 'clusters' },
		{ title: 'Listeners', value: summary.totals.listeners, highlighted: highlightResource === 'listeners' },
		{ title: 'Route Configs', value: summary.totals.routeConfigs, highlighted: highlightResource === 'routeConfigs' },
		{ title: 'Filters', value: summary.totals.filters, highlighted: highlightResource === 'filters' },
		{ title: 'Dataplanes', value: summary.totals.dataplanes, highlighted: highlightResource === 'dataplanes' },
		{ title: 'Secrets', value: summary.totals.secrets, highlighted: highlightResource === 'secrets' },
		{ title: 'Imports', value: summary.totals.imports, highlighted: highlightResource === 'imports' }
	]);
</script>

<div class="space-y-6">
	<!-- Totals Grid -->
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4">
		{#each statCards as card}
			<div
				class="bg-white rounded-lg border p-4 {card.highlighted
					? 'border-blue-500 ring-2 ring-blue-200'
					: 'border-gray-200'}"
			>
				<p class="text-sm font-medium text-gray-500">{card.title}</p>
				<p class="mt-1 text-2xl font-semibold text-gray-900">{card.value}</p>
			</div>
		{/each}
	</div>

	<!-- Org → Team Breakdown Table -->
	<div class="bg-white rounded-lg border border-gray-200 overflow-x-auto">
		<table class="min-w-full divide-y divide-gray-200">
			<thead class="bg-gray-50">
				<tr>
					<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Organization</th>
					<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Team</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Clusters</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Listeners</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Routes</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Filters</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Dataplanes</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Secrets</th>
					<th class="px-4 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Imports</th>
				</tr>
			</thead>
			<tbody class="bg-white divide-y divide-gray-200">
				{#each summary.orgs as org}
					{#each org.teams as team, teamIdx}
						<tr class="hover:bg-gray-50">
							<td class="px-4 py-3 text-sm text-gray-900">
								{#if teamIdx === 0}
									<span class="font-medium">{org.orgName ?? 'No Org'}</span>
								{/if}
							</td>
							<td class="px-4 py-3 text-sm text-gray-700">{team.teamDisplayName}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.clusters}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.listeners}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.routeConfigs}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.filters}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.dataplanes}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.secrets}</td>
							<td class="px-4 py-3 text-sm text-gray-700 text-right">{team.imports}</td>
						</tr>
					{/each}
				{/each}
				{#if summary.orgs.length === 0}
					<tr>
						<td colspan="9" class="px-4 py-8 text-center text-sm text-gray-500">
							No organizations or teams found. Create an organization to get started.
						</td>
					</tr>
				{/if}
			</tbody>
		</table>
	</div>
</div>
