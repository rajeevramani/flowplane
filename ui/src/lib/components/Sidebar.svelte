<script lang="ts">
	import { page } from '$app/stores';
	import {
		LayoutDashboard,
		Layers,
		Server,
		Radio,
		FileUp,
		Filter,
		Users,
		Building2,
		Key,
		FileText,
		ChevronDown,
		List,
		Link,
		BarChart3,
		Lock
	} from 'lucide-svelte';
	import type { SessionInfoResponse } from '$lib/api/types';

	interface ResourceCounts {
		routeConfigs: number;
		clusters: number;
		listeners: number;
		imports: number;
		filters: number;
		secrets?: number;
	}

	interface Props {
		sessionInfo: SessionInfoResponse;
		resourceCounts?: ResourceCounts;
		statsEnabled?: boolean;
	}

	let { sessionInfo, resourceCounts, statsEnabled = false }: Props = $props();

	// Track HTTP Filters menu expansion state
	let filtersMenuOpen = $state(true);

	// Resources navigation items (without filters - handled separately)
	const resourceItems = [
		{ id: 'clusters', label: 'Clusters', href: '/clusters', icon: Server },
		{ id: 'route-configs', label: 'Route Configurations', href: '/route-configs', icon: Layers },
		{ id: 'listeners', label: 'Listeners', href: '/listeners', icon: Radio },
		{ id: 'secrets', label: 'Secrets', href: '/secrets', icon: Lock },
		{ id: 'imports', label: 'Imports', href: '/imports', icon: FileUp }
	];

	// HTTP Filters submenu items
	const filtersSubmenu = [
		{ id: 'manage-filters', label: 'Manage Filters', href: '/filters', icon: List },
		{ id: 'attach-filters', label: 'Attach Filters', href: '/filters/attach', icon: Link }
	];

	// Admin navigation items
	const adminItems = [
		{ id: 'users', label: 'Users', href: '/admin/users', icon: Users },
		{ id: 'teams', label: 'Teams', href: '/admin/teams', icon: Building2 },
		{ id: 'audit', label: 'Audit Log', href: '/admin/audit-log', icon: FileText }
	];

	// Check if a path is active
	function isActive(href: string): boolean {
		const currentPath = $page.url.pathname;
		if (href === '/dashboard') {
			return currentPath === '/dashboard' || currentPath === '/';
		}
		// Special handling for /filters to not match /filters/attach
		if (href === '/filters') {
			return currentPath === '/filters' || currentPath === '/filters/create' || currentPath.startsWith('/filters/') && !currentPath.startsWith('/filters/attach');
		}
		return currentPath.startsWith(href);
	}

	// Check if any filter submenu item is active
	function isFiltersActive(): boolean {
		const currentPath = $page.url.pathname;
		return currentPath.startsWith('/filters');
	}

	// Toggle filters submenu
	function toggleFiltersMenu() {
		filtersMenuOpen = !filtersMenuOpen;
	}

	function getCount(id: string): number | undefined {
		if (!resourceCounts) return undefined;
		switch (id) {
			case 'route-configs':
				return resourceCounts.routeConfigs;
			case 'clusters':
				return resourceCounts.clusters;
			case 'listeners':
				return resourceCounts.listeners;
			case 'filters':
				return resourceCounts.filters;
			case 'imports':
				return resourceCounts.imports;
			case 'secrets':
				return resourceCounts.secrets;
			default:
				return undefined;
		}
	}
</script>

<aside class="w-56 bg-gray-900 text-white flex flex-col h-full">
	<!-- Logo -->
	<div class="p-4 border-b border-gray-800">
		<a href="/dashboard" class="flex items-center gap-2">
			<span class="text-lg font-bold tracking-tight">FLOWPLANE</span>
		</a>
	</div>

	<!-- Navigation -->
	<nav class="flex-1 overflow-y-auto py-4">
		<!-- Dashboard -->
		<div class="px-3 mb-4">
			<a
				href="/dashboard"
				class="flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors
					{isActive('/dashboard')
					? 'bg-blue-600 text-white'
					: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
			>
				<LayoutDashboard class="h-5 w-5" />
				Dashboard
			</a>
		</div>

		<!-- Envoy Stats Dashboard (only when enabled) -->
		{#if statsEnabled}
			<div class="px-3 mb-4">
				<a
					href="/stats"
					class="flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors
						{isActive('/stats')
						? 'bg-blue-600 text-white'
						: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
				>
					<BarChart3 class="h-5 w-5" />
					Envoy Stats
				</a>
			</div>
		{/if}

		<!-- Resources Section -->
		<div class="px-3 mb-4">
			<h3 class="px-3 mb-2 text-xs font-semibold text-gray-500 uppercase tracking-wider">
				Resources
			</h3>
			<div class="space-y-1">
				{#each resourceItems as item}
					{@const count = getCount(item.id)}
					<a
						href={item.href}
						class="flex items-center justify-between px-3 py-2 rounded-md text-sm font-medium transition-colors
							{isActive(item.href)
							? 'bg-blue-600 text-white'
							: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
					>
						<div class="flex items-center gap-3">
							<item.icon class="h-5 w-5" />
							{item.label}
						</div>
						{#if count !== undefined && count > 0}
							<span
								class="px-2 py-0.5 text-xs rounded-full
								{isActive(item.href) ? 'bg-blue-500 text-white' : 'bg-gray-700 text-gray-300'}"
							>
								{count}
							</span>
						{/if}
					</a>
				{/each}

				<!-- HTTP Filters - Expandable with submenu -->
				<div class="space-y-1">
					<button
						onclick={toggleFiltersMenu}
						class="w-full flex items-center justify-between px-3 py-2 rounded-md text-sm font-medium transition-colors
							{isFiltersActive()
							? 'bg-blue-600 text-white'
							: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
					>
						<div class="flex items-center gap-3">
							<Filter class="h-5 w-5" />
							HTTP Filters
						</div>
						<div class="flex items-center gap-2">
							{#if resourceCounts?.filters !== undefined && resourceCounts.filters > 0}
								<span
									class="px-2 py-0.5 text-xs rounded-full
									{isFiltersActive() ? 'bg-blue-500 text-white' : 'bg-gray-700 text-gray-300'}"
								>
									{resourceCounts.filters}
								</span>
							{/if}
							<ChevronDown
								class="h-4 w-4 transition-transform {filtersMenuOpen ? 'rotate-180' : ''}"
							/>
						</div>
					</button>

					<!-- Submenu -->
					{#if filtersMenuOpen}
						<div class="ml-4 pl-4 border-l border-gray-700 space-y-1">
							{#each filtersSubmenu as subItem}
								<a
									href={subItem.href}
									class="flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors
										{isActive(subItem.href)
										? 'bg-gray-800 text-white'
										: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
								>
									<subItem.icon class="h-4 w-4" />
									{subItem.label}
								</a>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Admin Section (only for admins) -->
		{#if sessionInfo.isAdmin}
			<div class="px-3 mb-4">
				<h3 class="px-3 mb-2 text-xs font-semibold text-gray-500 uppercase tracking-wider">
					Admin
				</h3>
				<div class="space-y-1">
					{#each adminItems as item}
						<a
							href={item.href}
							class="flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors
								{isActive(item.href)
								? 'bg-blue-600 text-white'
								: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
						>
							<item.icon class="h-5 w-5" />
							{item.label}
						</a>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Tokens (accessible to all) -->
		<div class="px-3">
			<a
				href="/tokens"
				class="flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors
					{isActive('/tokens')
					? 'bg-blue-600 text-white'
					: 'text-gray-300 hover:bg-gray-800 hover:text-white'}"
			>
				<Key class="h-5 w-5" />
				Access Tokens
			</a>
		</div>
	</nav>

	<!-- Version Footer -->
	<div class="px-4 py-3 border-t border-gray-800">
		<span class="text-xs text-gray-500">v{sessionInfo.version}</span>
	</div>
</aside>
