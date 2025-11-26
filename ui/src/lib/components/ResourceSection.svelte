<script lang="ts">
	import ResourceTable from './ResourceTable.svelte';

	interface Props {
		title: string;
		count: number;
		columns: any[];
		data: any[];
		emptyMessage?: string;
		actionButton?: {
			text: string;
			href: string;
		};
	}

	let { title, count, columns, data, emptyMessage, actionButton }: Props = $props();
</script>

<div class="mb-8">
	<div class="mb-4 flex justify-between items-center">
		<h2 class="text-xl font-semibold text-gray-900">
			{title} ({count})
		</h2>
		{#if actionButton}
			<a
				href={actionButton.href}
				class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded hover:bg-blue-700"
			>
				{actionButton.text}
			</a>
		{/if}
	</div>
	<div class="bg-white rounded-lg shadow-md p-6">
		<ResourceTable {columns} {data} {emptyMessage} />
	</div>
</div>
