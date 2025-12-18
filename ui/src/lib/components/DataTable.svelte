<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Column {
		key: string;
		label: string;
		width?: string;
		sortable?: boolean;
		align?: 'left' | 'center' | 'right';
	}

	interface Props {
		columns: Column[];
		data: Record<string, unknown>[];
		loading?: boolean;
		emptyMessage?: string;
		emptyIcon?: Snippet;
		onRowClick?: (row: Record<string, unknown>) => void;
		rowKey?: string;
		cell?: Snippet<[{ row: Record<string, unknown>; column: Column; value: unknown }]>;
		actions?: Snippet<[{ row: Record<string, unknown> }]>;
	}

	let {
		columns,
		data,
		loading = false,
		emptyMessage = 'No data available',
		emptyIcon,
		onRowClick,
		rowKey = 'id',
		cell,
		actions
	}: Props = $props();

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

	let sortedData = $derived.by(() => {
		if (!sortKey) return data;
		const key = sortKey; // Narrow type to non-null

		return [...data].sort((a, b) => {
			const aVal = a[key] as string | number | null;
			const bVal = b[key] as string | number | null;

			if (aVal === bVal) return 0;
			if (aVal === null || aVal === undefined) return 1;
			if (bVal === null || bVal === undefined) return -1;

			const comparison = aVal < bVal ? -1 : 1;
			return sortDirection === 'asc' ? comparison : -comparison;
		});
	});

	function getCellValue(row: Record<string, unknown>, column: Column): unknown {
		return row[column.key];
	}

	function getAlignClass(align: 'left' | 'center' | 'right' | undefined): string {
		switch (align) {
			case 'center':
				return 'text-center';
			case 'right':
				return 'text-right';
			default:
				return 'text-left';
		}
	}

	// Add actions column if actions snippet is provided
	let displayColumns = $derived(
		actions ? [...columns, { key: '_actions', label: '', width: 'w-24', align: 'right' as const }] : columns
	);
</script>

<div class="bg-white rounded-lg shadow-sm border border-gray-200">
	{#if loading}
		<!-- Loading skeleton -->
		<div class="animate-pulse">
			<div class="bg-gray-50 border-b border-gray-200 px-6 py-3">
				<div class="flex gap-6">
					{#each columns as column}
						<div class="h-4 bg-gray-200 rounded {column.width || 'flex-1'}"></div>
					{/each}
				</div>
			</div>
			{#each Array(5) as _}
				<div class="border-b border-gray-100 px-6 py-4">
					<div class="flex gap-6">
						{#each columns as column}
							<div class="h-4 bg-gray-100 rounded {column.width || 'flex-1'}"></div>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	{:else if data.length === 0}
		<!-- Empty state -->
		<div class="text-center py-12">
			{#if emptyIcon}
				{@render emptyIcon()}
			{:else}
				<svg
					class="mx-auto h-12 w-12 text-gray-400"
					fill="none"
					stroke="currentColor"
					viewBox="0 0 24 24"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="1.5"
						d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"
					/>
				</svg>
			{/if}
			<p class="mt-4 text-gray-500">{emptyMessage}</p>
		</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						{#each displayColumns as column}
							<th
								class="px-6 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider {getAlignClass(
									column.align
								)} {column.width || ''}"
							>
								{#if column.sortable}
									<button
										onclick={() => handleSort(column.key)}
										class="flex items-center gap-1 hover:text-gray-700 {column.align === 'right'
											? 'ml-auto'
											: ''}"
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
					{#each sortedData as row (row[rowKey])}
						<tr
							class="hover:bg-gray-50 transition-colors {onRowClick ? 'cursor-pointer' : ''}"
							onclick={() => onRowClick && onRowClick(row)}
						>
							{#each columns as column}
								<td
									class="px-6 py-4 whitespace-nowrap text-sm text-gray-900 {getAlignClass(
										column.align
									)} {column.width || ''}"
								>
									{#if cell}
										{@render cell({ row, column, value: getCellValue(row, column) })}
									{:else}
										{String(getCellValue(row, column) ?? '')}
									{/if}
								</td>
							{/each}
							{#if actions}
								<td class="px-6 py-4 whitespace-nowrap text-sm text-right">
									{@render actions({ row })}
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
