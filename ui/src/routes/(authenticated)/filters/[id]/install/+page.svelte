<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { ArrowLeft, Server, Check, X, Info, AlertTriangle } from 'lucide-svelte';
	import type { FilterResponse, ListenerResponse, FilterInstallationItem } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let error = $state<string | null>(null);
	let saveError = $state<string | null>(null);
	let saveSuccess = $state<string | null>(null);

	// Data
	let filter = $state<FilterResponse | null>(null);
	let listeners = $state<ListenerResponse[]>([]);
	let currentInstallations = $state<FilterInstallationItem[]>([]);

	// Track which listeners are selected for installation
	let selectedListeners = $state<Set<string>>(new Set());
	// Track execution order for each listener
	let listenerOrders = $state<Map<string, number>>(new Map());

	const filterId = $derived($page.params.id ?? '');

	onMount(async () => {
		if (filterId) {
			await loadData();
		}
	});

	async function loadData() {
		if (!filterId) {
			error = 'Filter ID is required';
			return;
		}

		isLoading = true;
		error = null;

		try {
			// Load filter details
			filter = await apiClient.getFilter(filterId);

			// Load all listeners
			listeners = await apiClient.listListeners();

			// Load current installations
			const installationsResponse = await apiClient.listFilterInstallations(filterId);
			currentInstallations = installationsResponse.installations;

			// Initialize selection state from current installations
			const selected = new Set<string>();
			const orders = new Map<string, number>();
			currentInstallations.forEach((inst) => {
				selected.add(inst.listenerName);
				orders.set(inst.listenerName, inst.order);
			});
			selectedListeners = selected;
			listenerOrders = orders;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
			console.error('Failed to load data:', e);
		} finally {
			isLoading = false;
		}
	}

	function toggleListener(listenerName: string) {
		const newSelected = new Set(selectedListeners);
		if (newSelected.has(listenerName)) {
			newSelected.delete(listenerName);
			const newOrders = new Map(listenerOrders);
			newOrders.delete(listenerName);
			listenerOrders = newOrders;
		} else {
			newSelected.add(listenerName);
			// Set default order
			if (!listenerOrders.has(listenerName)) {
				const newOrders = new Map(listenerOrders);
				newOrders.set(listenerName, 1);
				listenerOrders = newOrders;
			}
		}
		selectedListeners = newSelected;
	}

	function setOrder(listenerName: string, order: number) {
		const newOrders = new Map(listenerOrders);
		newOrders.set(listenerName, order);
		listenerOrders = newOrders;
	}

	function isCurrentlyInstalled(listenerName: string): boolean {
		return currentInstallations.some((inst) => inst.listenerName === listenerName);
	}

	async function handleSave() {
		isSaving = true;
		saveError = null;
		saveSuccess = null;

		try {
			// Determine what changed
			const currentlyInstalled = new Set(currentInstallations.map((inst) => inst.listenerName));
			const toInstall = [...selectedListeners].filter((name) => !currentlyInstalled.has(name));
			const toUninstall = [...currentlyInstalled].filter((name) => !selectedListeners.has(name));

			// Perform uninstalls
			for (const listenerName of toUninstall) {
				await apiClient.uninstallFilter(filterId, listenerName);
			}

			// Perform installs
			for (const listenerName of toInstall) {
				const order = listenerOrders.get(listenerName) || 1;
				await apiClient.installFilter(filterId, {
					listenerName,
					order
				});
			}

			saveSuccess = `Successfully updated installations. ${toInstall.length} installed, ${toUninstall.length} uninstalled.`;

			// Reload to show updated state
			await loadData();
		} catch (e) {
			saveError = e instanceof Error ? e.message : 'Failed to save changes';
		} finally {
			isSaving = false;
		}
	}

	function handleBack() {
		goto('/filters');
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-6">
		<button
			onclick={handleBack}
			class="inline-flex items-center text-sm text-gray-500 hover:text-gray-700 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Filters
		</button>

		{#if filter}
			<h1 class="text-3xl font-bold text-gray-900">Install Filter on Listeners</h1>
			<p class="mt-2 text-sm text-gray-600">
				Select which listeners should have the <span class="font-semibold">{filter.name}</span> filter
				installed in their HTTP connection manager chain.
			</p>
		{:else}
			<h1 class="text-3xl font-bold text-gray-900">Install Filter</h1>
		{/if}
	</div>

	<!-- Info Banner -->
	<div class="bg-blue-50 border border-blue-200 rounded-lg p-4 mb-6">
		<div class="flex">
			<Info class="h-5 w-5 text-blue-400 mr-3 flex-shrink-0 mt-0.5" />
			<div class="text-sm text-blue-700">
				<p class="font-medium mb-1">What does "Install" mean?</p>
				<p>
					Installing a filter on a listener adds it to the HTTP connection manager (HCM) filter chain.
					This is where the filter code actually executes. Once installed, the filter will process
					all traffic on that listener unless further configured at the route level.
				</p>
			</div>
		</div>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading...</span>
			</div>
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filter}
		<!-- Filter Info Card -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4 mb-6">
			<div class="flex items-center gap-4">
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-gray-900">{filter.name}</h2>
					<div class="flex items-center gap-2 mt-1">
						<Badge variant="blue">{filter.filterType.split('_').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')}</Badge>
						<span class="text-sm text-gray-500">Team: {filter.team}</span>
					</div>
				</div>
			</div>
		</div>

		<!-- Save Messages -->
		{#if saveError}
			<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-4">
				<p class="text-sm text-red-800">{saveError}</p>
			</div>
		{/if}

		{#if saveSuccess}
			<div class="bg-green-50 border border-green-200 rounded-md p-4 mb-4">
				<p class="text-sm text-green-800">{saveSuccess}</p>
			</div>
		{/if}

		<!-- Listener Selection -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<div class="px-6 py-4 border-b border-gray-200 bg-gray-50">
				<h3 class="text-lg font-medium text-gray-900">Available Listeners</h3>
				<p class="text-sm text-gray-500 mt-1">
					Select listeners to install this filter on. Checked listeners will have the filter in their HCM chain.
				</p>
			</div>

			{#if listeners.length === 0}
				<div class="px-6 py-12 text-center">
					<Server class="h-12 w-12 text-gray-400 mx-auto mb-4" />
					<h4 class="text-lg font-medium text-gray-900 mb-2">No Listeners Available</h4>
					<p class="text-sm text-gray-600">
						Create a listener first before installing filters.
					</p>
				</div>
			{:else}
				<div class="divide-y divide-gray-200">
					{#each listeners as listener}
						{@const isSelected = selectedListeners.has(listener.name)}
						{@const wasInstalled = isCurrentlyInstalled(listener.name)}
						<div
							class="px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
							class:bg-green-50={isSelected}
						>
							<div class="flex items-center gap-4">
								<button
									onclick={() => toggleListener(listener.name)}
									class="p-1 rounded-md border-2 transition-colors"
									class:border-green-500={isSelected}
									class:bg-green-500={isSelected}
									class:border-gray-300={!isSelected}
								>
									{#if isSelected}
										<Check class="h-4 w-4 text-white" />
									{:else}
										<div class="h-4 w-4"></div>
									{/if}
								</button>

								<div>
									<div class="flex items-center gap-2">
										<span class="text-sm font-medium text-gray-900">{listener.name}</span>
										{#if wasInstalled}
											<Badge variant="green">Currently Installed</Badge>
										{/if}
									</div>
									<div class="text-xs text-gray-500 mt-0.5">
										{listener.address}:{listener.port} - {listener.team}
									</div>
								</div>
							</div>

							{#if isSelected}
								<div class="flex items-center gap-2">
									<label class="text-sm text-gray-600">Order:</label>
									<input
										type="number"
										min="1"
										max="100"
										value={listenerOrders.get(listener.name) || 1}
										onchange={(e) => setOrder(listener.name, parseInt(e.currentTarget.value) || 1)}
										class="w-16 px-2 py-1 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Action Buttons -->
		<div class="mt-6 flex items-center gap-4">
			<Button onclick={handleSave} variant="primary" disabled={isSaving}>
				{#if isSaving}
					<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-white mr-2"></div>
					Saving...
				{:else}
					Apply Changes
				{/if}
			</Button>
			<Button onclick={handleBack} variant="secondary">
				Cancel
			</Button>
		</div>
	{/if}
</div>
