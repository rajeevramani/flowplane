<script lang="ts">
	import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight } from 'lucide-svelte';

	interface Props {
		currentPage: number;
		totalPages: number;
		totalItems: number;
		pageSize: number;
		onPageChange: (page: number) => void;
	}

	let { currentPage, totalPages, totalItems, pageSize, onPageChange }: Props = $props();

	// Calculate visible page numbers
	function getVisiblePages(): (number | 'ellipsis')[] {
		const pages: (number | 'ellipsis')[] = [];
		const maxVisible = 5;

		if (totalPages <= maxVisible) {
			for (let i = 1; i <= totalPages; i++) {
				pages.push(i);
			}
		} else {
			// Always show first page
			pages.push(1);

			if (currentPage > 3) {
				pages.push('ellipsis');
			}

			// Show pages around current
			const start = Math.max(2, currentPage - 1);
			const end = Math.min(totalPages - 1, currentPage + 1);

			for (let i = start; i <= end; i++) {
				pages.push(i);
			}

			if (currentPage < totalPages - 2) {
				pages.push('ellipsis');
			}

			// Always show last page
			if (totalPages > 1) {
				pages.push(totalPages);
			}
		}

		return pages;
	}

	let visiblePages = $derived(getVisiblePages());
	let startItem = $derived((currentPage - 1) * pageSize + 1);
	let endItem = $derived(Math.min(currentPage * pageSize, totalItems));
</script>

{#if totalPages > 1}
	<div class="flex items-center justify-between px-4 py-3 bg-white border-t border-gray-200">
		<!-- Item count -->
		<div class="text-sm text-gray-700">
			Showing <span class="font-medium">{startItem}</span> to
			<span class="font-medium">{endItem}</span> of
			<span class="font-medium">{totalItems}</span> results
		</div>

		<!-- Pagination controls -->
		<nav class="flex items-center gap-1" aria-label="Pagination">
			<!-- First page -->
			<button
				onclick={() => onPageChange(1)}
				disabled={currentPage === 1}
				class="p-1 rounded hover:bg-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
				title="First page"
			>
				<ChevronsLeft class="h-5 w-5 text-gray-500" />
			</button>

			<!-- Previous page -->
			<button
				onclick={() => onPageChange(currentPage - 1)}
				disabled={currentPage === 1}
				class="p-1 rounded hover:bg-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
				title="Previous page"
			>
				<ChevronLeft class="h-5 w-5 text-gray-500" />
			</button>

			<!-- Page numbers -->
			{#each visiblePages as page}
				{#if page === 'ellipsis'}
					<span class="px-2 text-gray-400">...</span>
				{:else}
					<button
						onclick={() => onPageChange(page)}
						class="min-w-[32px] h-8 px-3 rounded text-sm font-medium transition-colors
							{currentPage === page
							? 'bg-blue-600 text-white'
							: 'text-gray-700 hover:bg-gray-100'}"
					>
						{page}
					</button>
				{/if}
			{/each}

			<!-- Next page -->
			<button
				onclick={() => onPageChange(currentPage + 1)}
				disabled={currentPage === totalPages}
				class="p-1 rounded hover:bg-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
				title="Next page"
			>
				<ChevronRight class="h-5 w-5 text-gray-500" />
			</button>

			<!-- Last page -->
			<button
				onclick={() => onPageChange(totalPages)}
				disabled={currentPage === totalPages}
				class="p-1 rounded hover:bg-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
				title="Last page"
			>
				<ChevronsRight class="h-5 w-5 text-gray-500" />
			</button>
		</nav>
	</div>
{/if}
