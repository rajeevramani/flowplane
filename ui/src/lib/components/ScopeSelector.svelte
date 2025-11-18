<script lang="ts">
	/**
	 * Reusable scope selector component with grouped checkboxes.
	 * Used in PAT creation and team membership management.
	 */

	interface ScopeOption {
		value: string;
		label: string;
	}

	interface ScopeGroup {
		category: string;
		scopes: ScopeOption[];
	}

	interface Props {
		selectedScopes: string[];
		onScopeToggle: (scope: string) => void;
		required?: boolean;
	}

	let { selectedScopes = $bindable([]), onScopeToggle, required = false }: Props = $props();

	// Available scopes grouped by category
	const scopeGroups: ScopeGroup[] = [
		{
			category: 'Tokens',
			scopes: [
				{ value: 'tokens:read', label: 'Read tokens' },
				{ value: 'tokens:write', label: 'Create/update tokens' }
			]
		},
		{
			category: 'Clusters',
			scopes: [
				{ value: 'clusters:read', label: 'Read clusters' },
				{ value: 'clusters:write', label: 'Create/update clusters' }
			]
		},
		{
			category: 'Routes',
			scopes: [
				{ value: 'routes:read', label: 'Read routes' },
				{ value: 'routes:write', label: 'Create/update routes' }
			]
		},
		{
			category: 'Listeners',
			scopes: [
				{ value: 'listeners:read', label: 'Read listeners' },
				{ value: 'listeners:write', label: 'Create/update listeners' }
			]
		},
		{
			category: 'API Definitions',
			scopes: [
				{ value: 'api-definitions:read', label: 'Read API definitions' },
				{ value: 'api-definitions:write', label: 'Create/update API definitions' }
			]
		}
	];
</script>

<div>
	<div class="block text-sm font-medium text-gray-700 mb-2">
		Scopes {#if required}<span class="text-red-500">*</span>{/if}
	</div>
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
</div>
