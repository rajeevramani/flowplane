<script lang="ts">
	/**
	 * Reusable scope selector component with grouped checkboxes.
	 * Fetches available scopes dynamically from the API.
	 * Used in PAT creation and team membership management.
	 */
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import type { ScopeDefinition } from '$lib/api/types';

	interface ScopeGroup {
		category: string;
		scopes: ScopeDefinition[];
	}

	interface Props {
		selectedScopes: string[];
		onScopeToggle: (scope: string) => void;
		required?: boolean;
	}

	let { selectedScopes = $bindable([]), onScopeToggle, required = false }: Props = $props();

	let scopeGroups: ScopeGroup[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	onMount(async () => {
		try {
			const response = await apiClient.listScopes();

			// Group scopes by category
			const groupMap = new Map<string, ScopeDefinition[]>();
			for (const scope of response.scopes) {
				const existing = groupMap.get(scope.category) || [];
				existing.push(scope);
				groupMap.set(scope.category, existing);
			}

			// Convert map to array and sort categories
			scopeGroups = Array.from(groupMap.entries())
				.map(([category, scopes]) => ({ category, scopes }))
				.sort((a, b) => a.category.localeCompare(b.category));
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load scopes';
			console.error('Failed to load scopes:', e);
		} finally {
			loading = false;
		}
	});
</script>

<div>
	<div class="block text-sm font-medium text-gray-700 mb-2">
		Scopes {#if required}<span class="text-red-500">*</span>{/if}
	</div>

	{#if loading}
		<div class="text-sm text-gray-500 py-2">Loading scopes...</div>
	{:else if error}
		<div class="text-sm text-red-600 py-2">{error}</div>
	{:else if scopeGroups.length === 0}
		<div class="text-sm text-gray-500 py-2">No scopes available</div>
	{:else}
		<div class="space-y-4">
			{#each scopeGroups as group}
				<div>
					<h4 class="text-sm font-medium text-gray-900 mb-2">{group.category}</h4>
					<div class="space-y-2 pl-4">
						{#each group.scopes as scope}
							<label class="flex items-center">
								<input
									type="checkbox"
									checked={selectedScopes.includes(scope.value)}
									onchange={() => onScopeToggle(scope.value)}
									class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
								/>
								<span class="ml-2 text-sm text-gray-700">{scope.label}</span>
								<span class="ml-2 text-xs text-gray-500">({scope.value})</span>
							</label>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>
