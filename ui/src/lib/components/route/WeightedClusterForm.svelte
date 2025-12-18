<script lang="ts">
	import type { ClusterResponse } from "$lib/api/types";
	import type { WeightedCluster } from "../EditableRoutesTable.svelte";

	interface Props {
		clusters: ClusterResponse[];
		weightedClusters: WeightedCluster[];
	}

	let { clusters, weightedClusters = $bindable() }: Props = $props();

	// Derived: calculate total weight
	let totalWeight = $derived(
		weightedClusters.reduce((sum, c) => sum + (c.weight || 0), 0),
	);

	function addWeightedCluster() {
		weightedClusters = [...weightedClusters, { name: "", weight: 0 }];
	}

	function removeWeightedCluster(index: number) {
		if (weightedClusters.length > 1) {
			weightedClusters = weightedClusters.filter((_, i) => i !== index);
		}
	}

	function updateWeightedCluster(
		index: number,
		field: "name" | "weight",
		value: string | number,
	) {
		weightedClusters = weightedClusters.map((c, i) => {
			if (i === index) {
				return {
					...c,
					[field]: field === "weight" ? Number(value) : value,
				};
			}
			return c;
		});
	}
</script>

<div>
	<label class="block text-sm font-medium text-gray-700 mb-2"
		>Traffic Distribution</label
	>
	<div class="space-y-2">
		{#each weightedClusters as wc, index}
			<div class="flex items-center gap-2">
				<select
					bind:value={wc.name}
					onchange={(e) =>
						updateWeightedCluster(
							index,
							"name",
							(e.target as HTMLSelectElement).value,
						)}
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
						oninput={(e) =>
							updateWeightedCluster(
								index,
								"weight",
								(e.target as HTMLInputElement).value,
							)}
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
						<svg
							class="h-5 w-5"
							fill="none"
							stroke="currentColor"
							viewBox="0 0 24 24"
						>
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M6 18L18 6M6 6l12 12"
							/>
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
			<svg
				class="h-4 w-4"
				fill="none"
				stroke="currentColor"
				viewBox="0 0 24 24"
			>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M12 4v16m8-8H4"
				/>
			</svg>
			Add Cluster
		</button>
		<span
			class="text-sm {totalWeight === 100
				? 'text-green-600'
				: 'text-amber-600'}"
		>
			Total: {totalWeight}%
			{#if totalWeight !== 100}
				<span class="text-xs">(should be 100%)</span>
			{/if}
		</span>
	</div>
</div>
