<script lang="ts">
	import { Filter, Search, Check, Clock, Settings, Shield, ChevronDown } from 'lucide-svelte';
	import type { FilterResponse } from '$lib/api/types';

	interface Props {
		filters: FilterResponse[];
		selectedFilterId: string | null;
		onSelect: (filter: FilterResponse) => void;
		placeholder?: string;
	}

	let { filters, selectedFilterId, onSelect, placeholder = 'Select a filter...' }: Props = $props();

	let isOpen = $state(false);
	let searchQuery = $state('');
	let searchInputRef: HTMLInputElement | undefined = $state();

	// Find the currently selected filter
	let selectedFilter = $derived(filters.find((f) => f.id === selectedFilterId));

	// Filter the list based on search query
	let filteredFilters = $derived(
		filters.filter((filter) => {
			if (!searchQuery) return true;
			const query = searchQuery.toLowerCase();
			return (
				filter.name.toLowerCase().includes(query) ||
				filter.filterType.toLowerCase().includes(query) ||
				(filter.description && filter.description.toLowerCase().includes(query))
			);
		})
	);

	function toggleDropdown() {
		isOpen = !isOpen;
		if (isOpen) {
			searchQuery = '';
			// Focus the search input after the dropdown opens
			setTimeout(() => {
				searchInputRef?.focus();
			}, 10);
		}
	}

	function closeDropdown() {
		isOpen = false;
	}

	function handleSelect(filter: FilterResponse) {
		onSelect(filter);
		closeDropdown();
	}

	function handleClickOutside(event: MouseEvent) {
		const target = event.target as HTMLElement;
		if (!target.closest('.filter-dropdown-container')) {
			closeDropdown();
		}
	}

	// Format filter type for display
	function formatFilterType(type: string): string {
		return type
			.split('_')
			.map((word) => word.charAt(0).toUpperCase() + word.slice(1))
			.join(' ');
	}

	// Get color classes based on filter type
	function getFilterColors(type: string): {
		bg: string;
		text: string;
		badge: string;
	} {
		switch (type) {
			case 'header_mutation':
				return {
					bg: 'bg-green-100',
					text: 'text-green-600',
					badge: 'bg-green-100 text-green-800'
				};
			case 'jwt_auth':
			case 'jwt_authn':
				return {
					bg: 'bg-purple-100',
					text: 'text-purple-600',
					badge: 'bg-purple-100 text-purple-800'
				};
			case 'local_rate_limit':
			case 'rate_limit':
				return {
					bg: 'bg-orange-100',
					text: 'text-orange-600',
					badge: 'bg-orange-100 text-orange-800'
				};
			case 'cors':
				return {
					bg: 'bg-blue-100',
					text: 'text-blue-600',
					badge: 'bg-blue-100 text-blue-800'
				};
			default:
				return {
					bg: 'bg-gray-100',
					text: 'text-gray-600',
					badge: 'bg-gray-100 text-gray-800'
				};
		}
	}

	// Get icon component based on filter type
	function getFilterIcon(type: string) {
		switch (type) {
			case 'local_rate_limit':
			case 'rate_limit':
				return Clock;
			case 'jwt_auth':
			case 'jwt_authn':
				return Shield;
			case 'header_mutation':
			case 'cors':
			default:
				return Settings;
		}
	}

	$effect(() => {
		if (isOpen) {
			document.addEventListener('click', handleClickOutside);
		} else {
			document.removeEventListener('click', handleClickOutside);
		}
		return () => {
			document.removeEventListener('click', handleClickOutside);
		};
	});
</script>

<div class="relative filter-dropdown-container">
	<!-- Dropdown Trigger -->
	<button
		type="button"
		onclick={toggleDropdown}
		class="w-full flex items-center justify-between px-4 py-3 text-left bg-white border border-gray-300 rounded-lg hover:border-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500 transition-colors"
	>
		{#if selectedFilter}
			{@const colors = getFilterColors(selectedFilter.filterType)}
			{@const IconComponent = getFilterIcon(selectedFilter.filterType)}
			<div class="flex items-center gap-3">
				<div class="p-1.5 rounded {colors.bg}">
					<IconComponent class="w-4 h-4 {colors.text}" />
				</div>
				<div>
					<div class="flex items-center gap-2">
						<span class="text-sm font-medium text-gray-900">{selectedFilter.name}</span>
						<span class="px-2 py-0.5 text-xs rounded-full {colors.badge}">
							{formatFilterType(selectedFilter.filterType)}
						</span>
					</div>
					{#if selectedFilter.description}
						<p class="text-xs text-gray-500">{selectedFilter.description}</p>
					{/if}
				</div>
			</div>
		{:else}
			<span class="text-gray-500">{placeholder}</span>
		{/if}
		<ChevronDown class="w-5 h-5 text-gray-400 flex-shrink-0 transition-transform {isOpen ? 'rotate-180' : ''}" />
	</button>

	<!-- Dropdown Panel -->
	{#if isOpen}
		<div class="absolute z-10 w-full mt-1 bg-white border border-gray-200 rounded-lg shadow-lg">
			<!-- Search Input -->
			<div class="p-3 border-b border-gray-100">
				<div class="relative">
					<input
						type="text"
						bind:this={searchInputRef}
						bind:value={searchQuery}
						placeholder="Search filters..."
						class="w-full pl-9 pr-4 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<Search class="absolute left-3 top-2.5 w-4 h-4 text-gray-400" />
				</div>
			</div>

			<!-- Options List -->
			<div class="max-h-64 overflow-y-auto py-1">
				{#if filteredFilters.length === 0}
					<div class="px-4 py-6 text-center text-sm text-gray-500">
						No filters found matching your search.
					</div>
				{:else}
					{#each filteredFilters as filter}
						{@const colors = getFilterColors(filter.filterType)}
						{@const IconComponent = getFilterIcon(filter.filterType)}
						{@const isSelected = filter.id === selectedFilterId}
						<button
							type="button"
							onclick={() => handleSelect(filter)}
							class="w-full flex items-center gap-3 px-4 py-3 hover:bg-gray-50 transition-colors {isSelected ? 'bg-blue-50' : ''}"
						>
							<div class="p-1.5 rounded {colors.bg}">
								<IconComponent class="w-4 h-4 {colors.text}" />
							</div>
							<div class="flex-1 text-left">
								<div class="flex items-center gap-2">
									<span class="text-sm font-medium text-gray-900">{filter.name}</span>
									<span class="px-2 py-0.5 text-xs rounded-full {colors.badge}">
										{formatFilterType(filter.filterType)}
									</span>
								</div>
								{#if filter.description}
									<p class="text-xs text-gray-500">{filter.description}</p>
								{/if}
							</div>
							{#if isSelected}
								<Check class="w-4 h-4 text-blue-600" />
							{/if}
						</button>
					{/each}
				{/if}
			</div>
		</div>
	{/if}
</div>
