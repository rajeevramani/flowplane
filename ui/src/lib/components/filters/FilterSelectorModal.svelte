<script lang="ts">
	import { X, Filter, AlertCircle } from 'lucide-svelte';
	import type { FilterResponse, AttachmentPoint, FilterType, HierarchicalFilterContext, HierarchyLevel } from '$lib/api/types';
	import { canAttachTo, getAttachmentErrorMessage } from '$lib/utils/filter-attachment';
	import AttachmentPointBadge from './AttachmentPointBadge.svelte';
	import Button from '$lib/components/Button.svelte';

	interface Props {
		isOpen: boolean;
		filters: FilterResponse[];
		attachmentPoint: AttachmentPoint;
		alreadyAttachedIds: string[];
		onSelect: (filterId: string, order?: number) => void;
		onClose: () => void;
		isLoading?: boolean;
		// Hierarchical context for modal title
		hierarchyContext?: HierarchicalFilterContext;
	}

	let {
		isOpen,
		filters,
		attachmentPoint,
		alreadyAttachedIds,
		onSelect,
		onClose,
		isLoading = false,
		hierarchyContext
	}: Props = $props();

	let selectedFilterId = $state<string | null>(null);
	let filterOrder = $state<number>(1);
	let searchQuery = $state('');

	// Reset state when modal opens
	$effect(() => {
		if (isOpen) {
			selectedFilterId = null;
			filterOrder = alreadyAttachedIds.length + 1;
			searchQuery = '';
		}
	});

	// Get modal title based on hierarchy context
	function getModalTitle(): string {
		if (!hierarchyContext) {
			return 'Attach Filter';
		}

		switch (hierarchyContext.level) {
			case 'route_config':
				return `Attach Filter to Configuration`;
			case 'virtual_host':
				return `Attach Filter to Virtual Host: ${hierarchyContext.virtualHostName}`;
			case 'route':
				return `Attach Filter to Route: ${hierarchyContext.routeName}`;
			default:
				return 'Attach Filter';
		}
	}

	// Filter and categorize available filters
	let categorizedFilters = $derived(() => {
		const searchLower = searchQuery.toLowerCase();
		const compatible: FilterResponse[] = [];
		const incompatible: FilterResponse[] = [];
		const alreadyAttached: FilterResponse[] = [];

		for (const filter of filters) {
			// Filter by search query
			if (
				searchQuery &&
				!filter.name.toLowerCase().includes(searchLower) &&
				!(filter.description?.toLowerCase().includes(searchLower))
			) {
				continue;
			}

			if (alreadyAttachedIds.includes(filter.id)) {
				alreadyAttached.push(filter);
			} else if (canAttachTo(filter.filterType as FilterType, attachmentPoint)) {
				compatible.push(filter);
			} else {
				incompatible.push(filter);
			}
		}

		// Debug logging to help diagnose categorization issues
		if (filters.length > 0) {
			console.debug('Filter categorization:', {
				attachmentPoint,
				totalFilters: filters.length,
				compatible: compatible.length,
				incompatible: incompatible.length,
				alreadyAttached: alreadyAttached.length,
				filterTypes: filters.map(f => f.filterType)
			});
		}

		return { compatible, incompatible, alreadyAttached };
	});

	function getFilterTypeLabel(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'Header Mutation';
			case 'jwt_auth':
			case 'jwt_authn':
				return 'JWT Auth';
			case 'cors':
				return 'CORS';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'Rate Limit';
			case 'ext_authz':
				return 'External Auth';
			default:
				return filterType;
		}
	}

	function getFilterTypeBadgeColor(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'bg-blue-100 text-blue-800';
			case 'jwt_auth':
			case 'jwt_authn':
				return 'bg-green-100 text-green-800';
			case 'cors':
				return 'bg-purple-100 text-purple-800';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'bg-orange-100 text-orange-800';
			case 'ext_authz':
				return 'bg-red-100 text-red-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	// Get header color based on hierarchy level
	function getHeaderColor(): string {
		if (!hierarchyContext) return 'border-gray-200';
		switch (hierarchyContext.level) {
			case 'virtual_host':
				return 'border-emerald-200 bg-emerald-50';
			case 'route':
				return 'border-amber-200 bg-amber-50';
			default:
				return 'border-gray-200';
		}
	}

	function handleAttach() {
		if (selectedFilterId) {
			onSelect(selectedFilterId, filterOrder);
			onClose();
		}
	}

	function handleBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget) {
			onClose();
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			onClose();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#if isOpen}
	<!-- Backdrop -->
	<!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
	<div
		class="fixed inset-0 bg-black/50 z-40 flex items-center justify-center"
		onclick={handleBackdropClick}
		role="presentation"
	>
		<!-- Modal -->
		<div
			class="bg-white rounded-lg shadow-xl w-full max-w-lg mx-4 max-h-[80vh] flex flex-col"
			role="dialog"
			aria-modal="true"
			aria-labelledby="modal-title"
		>
			<!-- Header -->
			<div class="flex items-center justify-between px-6 py-4 border-b {getHeaderColor()}">
				<h2 id="modal-title" class="text-lg font-semibold text-gray-900">{getModalTitle()}</h2>
				<button
					onclick={onClose}
					class="text-gray-400 hover:text-gray-600 transition-colors"
					aria-label="Close"
				>
					<X class="h-5 w-5" />
				</button>
			</div>

			<!-- Search -->
			<div class="px-6 py-3 border-b border-gray-200">
				<input
					type="text"
					bind:value={searchQuery}
					placeholder="Search filters..."
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>

			<!-- Filter List -->
			<div class="flex-1 overflow-y-auto px-6 py-4">
				{#if isLoading}
					<div class="flex items-center justify-center py-8">
						<div class="animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600"></div>
						<span class="ml-2 text-sm text-gray-600">Loading filters...</span>
					</div>
				{:else}
					{@const { compatible, incompatible, alreadyAttached } = categorizedFilters()}

					{#if compatible.length === 0 && incompatible.length === 0 && alreadyAttached.length === 0}
						<div class="text-center py-8">
							<Filter class="h-12 w-12 text-gray-400 mx-auto mb-3" />
							<p class="text-sm text-gray-600">
								{searchQuery ? 'No filters match your search' : 'No filters available'}
							</p>
						</div>
					{:else}
						<!-- Compatible filters -->
						{#if compatible.length > 0}
							<div class="mb-4">
								<h3 class="text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">
									Available Filters ({compatible.length})
								</h3>
								<div class="space-y-2">
									{#each compatible as filter}
										<button
											onclick={() => (selectedFilterId = filter.id)}
											class="w-full text-left p-3 rounded-lg border transition-colors {selectedFilterId ===
											filter.id
												? 'border-blue-500 bg-blue-50'
												: 'border-gray-200 hover:border-gray-300'}"
										>
											<div class="flex items-center justify-between">
												<div>
													<div class="flex items-center gap-2">
														<span class="text-sm font-medium text-gray-900">{filter.name}</span>
														<span
															class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded {getFilterTypeBadgeColor(
																filter.filterType
															)}"
														>
															{getFilterTypeLabel(filter.filterType)}
														</span>
													</div>
													{#if filter.description}
														<p class="text-xs text-gray-500 mt-1">{filter.description}</p>
													{/if}
												</div>
												{#if selectedFilterId === filter.id}
													<div class="w-5 h-5 rounded-full bg-blue-500 flex items-center justify-center">
														<svg
															class="w-3 h-3 text-white"
															fill="currentColor"
															viewBox="0 0 20 20"
														>
															<path
																fill-rule="evenodd"
																d="M16.707 5.293a1 1 0 010 1.414l-8 8a1 1 0 01-1.414 0l-4-4a1 1 0 011.414-1.414L8 12.586l7.293-7.293a1 1 0 011.414 0z"
																clip-rule="evenodd"
															/>
														</svg>
													</div>
												{/if}
											</div>
										</button>
									{/each}
								</div>
							</div>
						{/if}

						<!-- Already attached filters -->
						{#if alreadyAttached.length > 0}
							<div class="mb-4">
								<h3 class="text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">
									Already Attached ({alreadyAttached.length})
								</h3>
								<div class="space-y-2">
									{#each alreadyAttached as filter}
										<div class="p-3 rounded-lg border border-gray-200 bg-gray-50 opacity-60">
											<div class="flex items-center gap-2">
												<span class="text-sm font-medium text-gray-600">{filter.name}</span>
												<span
													class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded {getFilterTypeBadgeColor(
														filter.filterType
													)}"
												>
													{getFilterTypeLabel(filter.filterType)}
												</span>
												<span class="text-xs text-gray-400">(attached)</span>
											</div>
										</div>
									{/each}
								</div>
							</div>
						{/if}

						<!-- Incompatible filters -->
						{#if incompatible.length > 0}
							<div>
								<h3 class="text-xs font-medium text-gray-500 uppercase tracking-wide mb-2">
									Incompatible ({incompatible.length})
								</h3>
								<div class="space-y-2">
									{#each incompatible as filter}
										<div
											class="p-3 rounded-lg border border-gray-200 bg-gray-50 opacity-60"
											title={getAttachmentErrorMessage(
												filter.filterType as FilterType,
												attachmentPoint
											) || ''}
										>
											<div class="flex items-center justify-between">
												<div>
													<div class="flex items-center gap-2">
														<span class="text-sm font-medium text-gray-500">{filter.name}</span>
														<span
															class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-gray-100 text-gray-500"
														>
															{getFilterTypeLabel(filter.filterType)}
														</span>
													</div>
													<div class="flex items-center gap-1 mt-1">
														<AlertCircle class="h-3 w-3 text-amber-500" />
														<span class="text-xs text-amber-600">
															{attachmentPoint === 'route'
																? 'Cannot attach to routes'
																: 'Cannot attach to listeners'}
														</span>
													</div>
												</div>
												<AttachmentPointBadge
													filterType={filter.filterType as FilterType}
													variant="compact"
												/>
											</div>
										</div>
									{/each}
								</div>
							</div>
						{/if}
					{/if}
				{/if}
			</div>

			<!-- Order input (when a filter is selected) -->
			{#if selectedFilterId}
				<div class="px-6 py-3 border-t border-gray-200 bg-gray-50">
					<label class="flex items-center gap-3">
						<span class="text-sm font-medium text-gray-700">Execution order:</span>
						<input
							type="number"
							bind:value={filterOrder}
							min="1"
							class="w-20 px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<span class="text-xs text-gray-500">Lower numbers execute first</span>
					</label>
				</div>
			{/if}

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-6 py-4 border-t border-gray-200">
				<Button onclick={onClose} variant="secondary">Cancel</Button>
				<Button onclick={handleAttach} variant="primary" disabled={!selectedFilterId}>
					Attach Filter
				</Button>
			</div>
		</div>
	</div>
{/if}
