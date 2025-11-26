<script lang="ts" generics="T extends Record<string, any>">
	import Badge from './Badge.svelte';

	interface Column<T> {
		key: string;
		label: string;
		format?: (value: any, row: T) => string | { type: 'badge'; text: string; variant?: 'blue' | 'green' | 'purple' | 'orange' | 'red' | 'gray' } | { type: 'link'; text: string; href: string };
	}

	interface Props {
		columns: Column<T>[];
		data: T[];
		emptyMessage?: string;
	}

	let { columns, data, emptyMessage = 'No data found' }: Props = $props();

	function getCellValue(row: T, column: Column<T>) {
		const keys = column.key.split('.');
		let value: any = row;

		for (const key of keys) {
			value = value?.[key];
			if (value === undefined || value === null) break;
		}

		if (column.format) {
			return column.format(value, row);
		}

		return value;
	}

	function renderCell(row: T, column: Column<T>) {
		const value = getCellValue(row, column);

		if (typeof value === 'object' && value !== null && 'type' in value) {
			if (value.type === 'badge') {
				return { badge: true, text: value.text, variant: value.variant || 'blue' };
			}
			if (value.type === 'link') {
				return { link: true, text: value.text, href: value.href };
			}
		}

		return { text: value !== undefined && value !== null ? String(value) : 'N/A' };
	}
</script>

<div class="overflow-x-auto">
	{#if data.length === 0}
		<p class="text-center text-gray-500 py-12">{emptyMessage}</p>
	{:else}
		<table class="min-w-full divide-y divide-gray-200">
			<thead class="bg-gray-50">
				<tr>
					{#each columns as column}
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							{column.label}
						</th>
					{/each}
				</tr>
			</thead>
			<tbody class="bg-white divide-y divide-gray-200">
				{#each data as row}
					<tr class="hover:bg-gray-50">
						{#each columns as column}
							{@const cell = renderCell(row, column)}
							<td class="px-6 py-4 whitespace-nowrap">
								{#if cell.badge}
									<Badge variant={cell.variant}>{cell.text}</Badge>
								{:else if cell.link}
									<a
										href={cell.href}
										class="text-sm font-medium text-blue-600 hover:text-blue-800"
									>
										{cell.text}
									</a>
								{:else}
									<span class="text-sm text-gray-600">{cell.text}</span>
								{/if}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>
