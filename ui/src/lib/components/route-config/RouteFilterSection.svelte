<!--
	RouteFilterSection.svelte

	Section component for displaying and managing filters at the route level.
	Displays filters grouped by direct attachments vs inherited from parent levels.

	Features:
	- Groups filters by direct (route-level) vs inherited (from route config or virtual host)
	- Shows filter count badge in header
	- "Attach Filter" button to add new filters
	- Empty state when no filters are present
	- Manages filter attachment/detachment at route level

	Usage example:
	```svelte
	<RouteFilterSection
		routeConfigName="my-routes"
		virtualHostName="example.com"
		routeName="api-route"
		directFilters={routeFilters}
		inheritedFilters={inheritedFromParents}
		onAddFilter={() => showAttachModal()}
		onRemoveFilter={(id) => handleDetach(id)}
		onFiltersChanged={() => refreshFilters()}
	/>
	```
-->
<script lang="ts">
	import { Plus, Filter } from 'lucide-svelte';
	import type { FilterResponse } from '$lib/api/types';
	import RouteFilterCard from '../filters/RouteFilterCard.svelte';

	interface Props {
		/** Route config name for scope ID */
		routeConfigName: string;
		/** Virtual host name for scope ID */
		virtualHostName: string;
		/** Route name for scope ID */
		routeName: string;
		/** Filters directly attached to this route */
		directFilters: FilterResponse[];
		/** Filters inherited from route config or virtual host */
		inheritedFilters: FilterResponse[];
		/** Called when user wants to add a new filter */
		onAddFilter?: () => void;
		/** Called when user wants to remove a filter */
		onRemoveFilter?: (filterId: string) => void;
		/** Called after any filter changes (for refreshing data) */
		onFiltersChanged?: () => void;
	}

	let {
		routeConfigName,
		virtualHostName,
		routeName,
		directFilters,
		inheritedFilters,
		onAddFilter,
		onRemoveFilter,
		onFiltersChanged
	}: Props = $props();

	// Calculate total filter count
	const totalFilterCount = $derived(directFilters.length + inheritedFilters.length);
</script>

<div class="space-y-4">
	<!-- Section Header -->
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-2">
			<h3 class="text-lg font-semibold text-gray-900">Filters</h3>
			{#if totalFilterCount > 0}
				<span
					class="inline-flex items-center px-2.5 py-0.5 text-sm font-medium rounded-full bg-blue-100 text-blue-800"
				>
					{totalFilterCount}
				</span>
			{/if}
		</div>

		{#if onAddFilter}
			<button
				onclick={onAddFilter}
				class="inline-flex items-center gap-2 px-3 py-1.5 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-md transition-colors"
			>
				<Plus class="h-4 w-4" />
				Attach Filter
			</button>
		{/if}
	</div>

	<!-- Empty State -->
	{#if totalFilterCount === 0}
		<div
			class="flex items-center justify-center py-8 border-2 border-dashed border-gray-300 rounded-lg bg-gray-50"
		>
			<div class="text-center">
				<Filter class="h-10 w-10 text-gray-400 mx-auto mb-3" />
				<p class="text-sm font-medium text-gray-700">No filters configured</p>
				<p class="text-xs text-gray-500 mt-1">
					Filters can be inherited from the route config or virtual host
				</p>
				{#if onAddFilter}
					<button
						onclick={onAddFilter}
						class="mt-3 inline-flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-blue-600 hover:text-blue-700 hover:bg-blue-50 rounded-md transition-colors"
					>
						<Plus class="h-4 w-4" />
						Attach your first filter
					</button>
				{/if}
			</div>
		</div>
	{:else}
		<!-- Direct Filters Section -->
		{#if directFilters.length > 0}
			<div class="space-y-2">
				<div class="flex items-center gap-2">
					<h4 class="text-sm font-medium text-gray-700">Configured for this route</h4>
					<span
						class="inline-flex items-center px-1.5 py-0.5 text-xs font-medium rounded bg-green-100 text-green-700"
					>
						{directFilters.length}
					</span>
				</div>
				<div class="space-y-2">
					{#each directFilters as filter (filter.id)}
						<RouteFilterCard
							{filter}
							{routeConfigName}
							{virtualHostName}
							{routeName}
							isInherited={false}
							onRemove={onRemoveFilter ? () => onRemoveFilter(filter.id) : undefined}
							onSettingsUpdate={onFiltersChanged}
						/>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Inherited Filters Section -->
		{#if inheritedFilters.length > 0}
			<div class="space-y-2">
				<div class="flex items-center gap-2">
					<h4 class="text-sm font-medium text-gray-700">
						Inherited from route config or virtual host
					</h4>
					<span
						class="inline-flex items-center px-1.5 py-0.5 text-xs font-medium rounded bg-gray-100 text-gray-600"
					>
						{inheritedFilters.length}
					</span>
				</div>
				<p class="text-xs text-gray-500">
					These filters are configured at a higher level and can be overridden for this route.
				</p>
				<div class="space-y-2">
					{#each inheritedFilters as filter (filter.id)}
						<RouteFilterCard
							{filter}
							{routeConfigName}
							{virtualHostName}
							{routeName}
							isInherited={true}
							onSettingsUpdate={onFiltersChanged}
						/>
					{/each}
				</div>
			</div>
		{/if}
	{/if}
</div>
