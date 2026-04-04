<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import type { GrantResponse, CreateGrantRequest } from '$lib/api/types';

	interface Props {
		principalId: string;
		orgName: string;
		teamId: string;
		teamName: string;
		existingGrants: GrantResponse[];
		onGrantCreated: () => void;
		onGrantDeleted: () => void;
	}

	let {
		principalId,
		orgName,
		teamId,
		teamName,
		existingGrants,
		onGrantCreated,
		onGrantDeleted
	}: Props = $props();

	const VALID_RESOURCE_PAIRS: Record<string, string[]> = {
		clusters: ['read', 'create', 'update', 'delete'],
		listeners: ['read', 'create', 'update', 'delete'],
		routes: ['read', 'create', 'update', 'delete'],
		filters: ['read', 'create', 'update', 'delete'],
		secrets: ['read', 'create', 'update', 'delete'],
		dataplanes: ['read', 'create', 'update', 'delete'],
		'custom-wasm-filters': ['read', 'create', 'update', 'delete'],
		'learning-sessions': ['read', 'create', 'execute', 'delete'],
		'aggregated-schemas': ['read', 'execute'],
		'proxy-certificates': ['read', 'create', 'delete'],
		reports: ['read'],
		audit: ['read'],
		stats: ['read'],
		agents: ['read', 'create', 'update', 'delete']
	};

	const RESOURCES = Object.keys(VALID_RESOURCE_PAIRS);
	const ACTIONS = ['read', 'create', 'update', 'delete', 'execute'] as const;

	let cellLoading = $state<Record<string, boolean>>({});
	let error = $state<string | null>(null);

	function isValidPair(resource: string, action: string): boolean {
		return VALID_RESOURCE_PAIRS[resource]?.includes(action) ?? false;
	}

	function cellKey(resource: string, action: string): string {
		return `${resource}:${action}`;
	}

	function findGrant(resource: string, action: string): GrantResponse | undefined {
		return existingGrants.find(
			(g) =>
				g.grantType === 'resource' &&
				g.resourceType === resource &&
				g.action === action &&
				g.team === teamId
		);
	}

	function isGranted(resource: string, action: string): boolean {
		return findGrant(resource, action) !== undefined;
	}

	async function toggleCell(resource: string, action: string) {
		if (!isValidPair(resource, action)) return;
		const key = cellKey(resource, action);
		if (cellLoading[key]) return;

		cellLoading = { ...cellLoading, [key]: true };
		error = null;

		try {
			const existing = findGrant(resource, action);
			if (existing) {
				await apiClient.deletePrincipalGrant(orgName, principalId, existing.id);
				onGrantDeleted();
			} else {
				const request: CreateGrantRequest = {
					grantType: 'resource',
					team: teamId,
					resourceType: resource,
					action
				};
				await apiClient.createPrincipalGrant(orgName, principalId, request);
				onGrantCreated();
			}
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to update permission';
		} finally {
			cellLoading = { ...cellLoading, [key]: false };
		}
	}
</script>

<div>
	<div class="px-4 py-2 text-xs text-gray-500 bg-gray-50 border-b border-gray-200">
		Team: <span class="font-mono text-gray-700">{teamName}</span>
		— Check to grant, uncheck to revoke.
	</div>

	{#if error}
		<div class="mx-4 mt-3 bg-red-50 border-l-4 border-red-500 p-3">
			<p class="text-red-800 text-sm">{error}</p>
		</div>
	{/if}

	<div class="overflow-x-auto">
		<table class="min-w-full">
			<thead>
				<tr class="bg-gray-50 border-b border-gray-200">
					<th
						class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-48"
					>
						Resource
					</th>
					{#each ACTIONS as action}
						<th
							class="px-3 py-3 text-center text-xs font-medium text-gray-500 uppercase tracking-wider w-20"
						>
							{action}
						</th>
					{/each}
				</tr>
			</thead>
			<tbody class="divide-y divide-gray-100">
				{#each RESOURCES as resource}
					<tr class="hover:bg-gray-50/50">
						<td class="px-4 py-2.5 text-sm font-mono text-gray-900">{resource}</td>
						{#each ACTIONS as action}
							<td class="px-3 py-2.5 text-center">
								{#if isValidPair(resource, action)}
									{@const key = cellKey(resource, action)}
									{@const loading = cellLoading[key] ?? false}
									{@const granted = isGranted(resource, action)}
									<input
										type="checkbox"
										checked={granted}
										disabled={loading}
										onchange={() => toggleCell(resource, action)}
										class="h-4 w-4 rounded border-gray-300 {granted
											? 'text-green-600'
											: 'text-blue-600'} {loading
											? 'opacity-50 cursor-wait'
											: 'cursor-pointer'}"
										title="{resource}:{action}"
									/>
								{:else}
									<span class="text-gray-200">—</span>
								{/if}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
