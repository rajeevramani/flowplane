<script lang="ts">
	interface Column {
		key: string;
		label: string;
		sortable?: boolean;
		render?: (value: any, row: any) => string;
	}

	interface Props {
		columns: Column[];
		data: any[];
		emptyMessage?: string;
		onRowClick?: (row: any) => void;
	}

	let { columns, data, emptyMessage = 'No data available', onRowClick }: Props = $props();

	let sortKey = $state<string | null>(null);
	let sortDirection = $state<'asc' | 'desc'>('asc');

	function handleSort(key: string) {
		if (sortKey === key) {
			sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = key;
			sortDirection = 'asc';
		}
	}

	let sortedData = $derived(() => {
		if (!sortKey) return data;

		return [...data].sort((a, b) => {
			const aVal = a[sortKey];
			const bVal = b[sortKey];

			if (aVal === bVal) return 0;

			const comparison = aVal < bVal ? -1 : 1;
			return sortDirection === 'asc' ? comparison : -comparison;
		});
	});

	function getCellValue(row: any, column: Column): string {
		const value = row[column.key];
		return column.render ? column.render(value, row) : String(value ?? '');
	}
</script>

{#if data.length === 0}
	<p class="text-center text-gray-500 py-12">{emptyMessage}</p>
{:else}
	<div class="overflow-x-auto">
		<table class="min-w-full divide-y divide-gray-200">
			<thead class="bg-gray-50">
				<tr>
					{#each columns as column}
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							{#if column.sortable}
								<button
									onclick={() => handleSort(column.key)}
									class="flex items-center gap-1 hover:text-gray-700"
								>
									{column.label}
									{#if sortKey === column.key}
										<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
											{#if sortDirection === 'asc'}
												<path
													stroke-linecap="round"
													stroke-linejoin="round"
													stroke-width="2"
													d="M5 15l7-7 7 7"
												/>
											{:else}
												<path
													stroke-linecap="round"
													stroke-linejoin="round"
													stroke-width="2"
													d="M19 9l-7 7-7-7"
												/>
											{/if}
										</svg>
									{/if}
								</button>
							{:else}
								{column.label}
							{/if}
						</th>
					{/each}
				</tr>
			</thead>
			<tbody class="bg-white divide-y divide-gray-200">
				{#each sortedData() as row}
					<tr
						class="hover:bg-gray-50 {onRowClick ? 'cursor-pointer' : ''}"
						onclick={() => onRowClick && onRowClick(row)}
					>
						{#each columns as column}
							<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
								{getCellValue(row, column)}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}
