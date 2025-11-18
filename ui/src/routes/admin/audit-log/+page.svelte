<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { AuditLogEntry, ListAuditLogsQuery } from '$lib/api/types';

	let entries = $state<AuditLogEntry[]>([]);
	let total = $state(0);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	// Pagination
	let currentPage = $state(1);
	let pageSize = $state(50);

	// Filters
	let resourceTypeFilter = $state('all');
	let actionFilter = $state('all');
	let userIdFilter = $state('');
	let startDateFilter = $state('');
	let endDateFilter = $state('');

	// Detail modal
	let selectedEntry = $state<AuditLogEntry | null>(null);
	let showDetailModal = $state(false);

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}
			await loadAuditLogs();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadAuditLogs() {
		isLoading = true;
		error = null;

		try {
			const query: ListAuditLogsQuery = {
				limit: pageSize,
				offset: (currentPage - 1) * pageSize
			};

			if (resourceTypeFilter !== 'all') {
				query.resource_type = resourceTypeFilter;
			}
			if (actionFilter !== 'all') {
				query.action = actionFilter;
			}
			if (userIdFilter.trim()) {
				query.user_id = userIdFilter.trim();
			}
			if (startDateFilter) {
				query.start_date = new Date(startDateFilter).toISOString();
			}
			if (endDateFilter) {
				query.end_date = new Date(endDateFilter).toISOString();
			}

			const response = await apiClient.listAuditLogs(query);
			entries = response.entries;
			total = response.total;
		} catch (err: any) {
			error = err.message || 'Failed to load audit logs';
		} finally {
			isLoading = false;
		}
	}

	let totalPages = $derived.by(() => Math.ceil(total / pageSize));

	function formatDate(dateString: string): string {
		const date = new Date(dateString);
		return date.toLocaleString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit'
		});
	}

	function formatDateForInput(dateString: string): string {
		if (!dateString) return '';
		const date = new Date(dateString);
		return date.toISOString().slice(0, 16);
	}

	function getActionColor(action: string): string {
		const lowerAction = action.toLowerCase();
		if (lowerAction.includes('create')) return 'bg-green-100 text-green-800';
		if (lowerAction.includes('update') || lowerAction.includes('modify'))
			return 'bg-blue-100 text-blue-800';
		if (lowerAction.includes('delete') || lowerAction.includes('revoke'))
			return 'bg-red-100 text-red-800';
		if (lowerAction.includes('login') || lowerAction.includes('auth'))
			return 'bg-purple-100 text-purple-800';
		return 'bg-gray-100 text-gray-800';
	}

	function getResourceTypeColor(resourceType: string): string {
		if (resourceType.includes('auth')) return 'bg-purple-100 text-purple-800';
		if (resourceType.includes('platform')) return 'bg-blue-100 text-blue-800';
		if (resourceType.includes('secrets')) return 'bg-yellow-100 text-yellow-800';
		return 'bg-gray-100 text-gray-800';
	}

	function openDetailModal(entry: AuditLogEntry) {
		selectedEntry = entry;
		showDetailModal = true;
	}

	function closeDetailModal() {
		showDetailModal = false;
		selectedEntry = null;
	}

	function tryParseJSON(jsonString: string | null): any {
		if (!jsonString) return null;
		try {
			return JSON.parse(jsonString);
		} catch {
			return jsonString;
		}
	}

	function exportToCSV() {
		// Build CSV header
		const header = [
			'Timestamp',
			'Resource Type',
			'Resource ID',
			'Resource Name',
			'Action',
			'User ID',
			'Client IP',
			'User Agent'
		];

		// Build CSV rows
		const rows = entries.map((entry) => [
			entry.created_at,
			entry.resource_type,
			entry.resource_id || '',
			entry.resource_name || '',
			entry.action,
			entry.user_id || '',
			entry.client_ip || '',
			entry.user_agent || ''
		]);

		// Convert to CSV string
		const csvContent = [
			header.join(','),
			...rows.map((row) => row.map((cell) => `"${cell}"`).join(','))
		].join('\n');

		// Create download
		const blob = new Blob([csvContent], { type: 'text/csv' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `audit-logs-${new Date().toISOString()}.csv`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	async function applyFilters() {
		currentPage = 1;
		await loadAuditLogs();
	}

	async function resetFilters() {
		resourceTypeFilter = 'all';
		actionFilter = 'all';
		userIdFilter = '';
		startDateFilter = '';
		endDateFilter = '';
		currentPage = 1;
		await loadAuditLogs();
	}

	async function changePage(page: number) {
		currentPage = page;
		await loadAuditLogs();
	}
</script>

<svelte:head>
	<title>Audit Log - Flowplane</title>
</svelte:head>

<div class="container mx-auto px-4 py-8">
	<div class="mb-6">
		<h1 class="text-3xl font-bold text-gray-900">Audit Log</h1>
		<p class="mt-2 text-gray-600">
			View system-wide audit logs including authentication events, resource changes, and user
			activity.
		</p>
	</div>

	<!-- Filters -->
	<div class="mb-6 bg-white rounded-lg shadow p-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Filters</h2>

		<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
			<!-- Resource Type Filter -->
			<div>
				<label for="resourceType" class="block text-sm font-medium text-gray-700 mb-1"
					>Resource Type</label
				>
				<select
					id="resourceType"
					bind:value={resourceTypeFilter}
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:ring-blue-500 focus:border-blue-500"
				>
					<option value="all">All Types</option>
					<option value="auth.token">Auth/Token</option>
					<option value="platform.api">Platform API</option>
					<option value="secrets">Secrets</option>
				</select>
			</div>

			<!-- Action Filter -->
			<div>
				<label for="action" class="block text-sm font-medium text-gray-700 mb-1">Action</label>
				<input
					type="text"
					id="action"
					bind:value={actionFilter}
					placeholder="e.g., CREATE, UPDATE, DELETE"
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:ring-blue-500 focus:border-blue-500"
				/>
			</div>

			<!-- User ID Filter -->
			<div>
				<label for="userId" class="block text-sm font-medium text-gray-700 mb-1">User ID</label>
				<input
					type="text"
					id="userId"
					bind:value={userIdFilter}
					placeholder="Filter by user ID"
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:ring-blue-500 focus:border-blue-500"
				/>
			</div>

			<!-- Start Date Filter -->
			<div>
				<label for="startDate" class="block text-sm font-medium text-gray-700 mb-1"
					>Start Date</label
				>
				<input
					type="datetime-local"
					id="startDate"
					bind:value={startDateFilter}
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:ring-blue-500 focus:border-blue-500"
				/>
			</div>

			<!-- End Date Filter -->
			<div>
				<label for="endDate" class="block text-sm font-medium text-gray-700 mb-1">End Date</label>
				<input
					type="datetime-local"
					id="endDate"
					bind:value={endDateFilter}
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:ring-blue-500 focus:border-blue-500"
				/>
			</div>
		</div>

		<!-- Filter Actions -->
		<div class="mt-4 flex gap-2">
			<button
				onclick={applyFilters}
				class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2"
			>
				Apply Filters
			</button>
			<button
				onclick={resetFilters}
				class="px-4 py-2 bg-gray-200 text-gray-700 rounded-md hover:bg-gray-300 focus:outline-none focus:ring-2 focus:ring-gray-500 focus:ring-offset-2"
			>
				Reset
			</button>
			<button
				onclick={exportToCSV}
				disabled={entries.length === 0}
				class="ml-auto px-4 py-2 bg-green-600 text-white rounded-md hover:bg-green-700 focus:outline-none focus:ring-2 focus:ring-green-500 focus:ring-offset-2 disabled:bg-gray-300 disabled:cursor-not-allowed"
			>
				Export to CSV
			</button>
		</div>
	</div>

	<!-- Results Summary -->
	<div class="mb-4 text-sm text-gray-600">
		Showing {entries.length > 0 ? (currentPage - 1) * pageSize + 1 : 0} -
		{Math.min(currentPage * pageSize, total)} of {total} entries
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="text-center py-12">
			<div class="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			<p class="mt-2 text-gray-600">Loading audit logs...</p>
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-lg p-4">
			<p class="text-red-800">{error}</p>
		</div>
	{:else if entries.length === 0}
		<div class="bg-gray-50 border border-gray-200 rounded-lg p-8 text-center">
			<p class="text-gray-600">No audit log entries found.</p>
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow overflow-hidden">
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Timestamp
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Resource Type
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Resource
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Action
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								User
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Client IP
							</th>
							<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
								Actions
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each entries as entry (entry.id)}
							<tr class="hover:bg-gray-50 cursor-pointer" onclick={() => openDetailModal(entry)}>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
									{formatDate(entry.created_at)}
								</td>
								<td class="px-6 py-4 whitespace-nowrap">
									<span
										class="px-2 py-1 text-xs font-semibold rounded-full {getResourceTypeColor(
											entry.resource_type
										)}"
									>
										{entry.resource_type}
									</span>
								</td>
								<td class="px-6 py-4 text-sm text-gray-900">
									<div class="max-w-xs truncate">{entry.resource_name || entry.resource_id || '-'}</div>
								</td>
								<td class="px-6 py-4 whitespace-nowrap">
									<span
										class="px-2 py-1 text-xs font-semibold rounded-full {getActionColor(entry.action)}"
									>
										{entry.action}
									</span>
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
									{entry.user_id || '-'}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									{entry.client_ip || '-'}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
									<button
										onclick={(e) => {
											e.stopPropagation();
											openDetailModal(entry);
										}}
										class="text-blue-600 hover:text-blue-900"
									>
										View Details
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>

		<!-- Pagination -->
		<div class="mt-6 flex items-center justify-between">
			<button
				onclick={() => changePage(currentPage - 1)}
				disabled={currentPage === 1}
				class="px-4 py-2 bg-white border border-gray-300 rounded-md text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:bg-gray-100 disabled:cursor-not-allowed"
			>
				Previous
			</button>

			<div class="flex items-center gap-2">
				<span class="text-sm text-gray-700">
					Page {currentPage} of {totalPages}
				</span>
			</div>

			<button
				onclick={() => changePage(currentPage + 1)}
				disabled={currentPage === totalPages}
				class="px-4 py-2 bg-white border border-gray-300 rounded-md text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:bg-gray-100 disabled:cursor-not-allowed"
			>
				Next
			</button>
		</div>
	{/if}
</div>

<!-- Detail Modal -->
{#if showDetailModal && selectedEntry}
	<div
		class="fixed inset-0 z-50 overflow-y-auto"
		aria-labelledby="modal-title"
		role="dialog"
		aria-modal="true"
	>
		<div class="flex items-center justify-center min-h-screen px-4 pt-4 pb-20 text-center sm:block sm:p-0">
			<!-- Background overlay -->
			<div
				class="fixed inset-0 bg-gray-500 bg-opacity-75 transition-opacity"
				onclick={closeDetailModal}
			></div>

			<!-- Modal panel -->
			<div
				class="inline-block align-bottom bg-white rounded-lg text-left overflow-hidden shadow-xl transform transition-all sm:my-8 sm:align-middle sm:max-w-4xl sm:w-full"
			>
				<div class="bg-white px-4 pt-5 pb-4 sm:p-6 sm:pb-4">
					<div class="sm:flex sm:items-start">
						<div class="mt-3 text-center sm:mt-0 sm:text-left w-full">
							<h3 class="text-lg leading-6 font-medium text-gray-900 mb-4" id="modal-title">
								Audit Log Entry Details
							</h3>

							<div class="space-y-4">
								<!-- Metadata -->
								<div class="grid grid-cols-2 gap-4 pb-4 border-b border-gray-200">
									<div>
										<p class="text-sm font-medium text-gray-500">Timestamp</p>
										<p class="mt-1 text-sm text-gray-900">{formatDate(selectedEntry.created_at)}</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">Resource Type</p>
										<p class="mt-1">
											<span
												class="px-2 py-1 text-xs font-semibold rounded-full {getResourceTypeColor(
													selectedEntry.resource_type
												)}"
											>
												{selectedEntry.resource_type}
											</span>
										</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">Action</p>
										<p class="mt-1">
											<span
												class="px-2 py-1 text-xs font-semibold rounded-full {getActionColor(
													selectedEntry.action
												)}"
											>
												{selectedEntry.action}
											</span>
										</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">Resource ID</p>
										<p class="mt-1 text-sm text-gray-900">{selectedEntry.resource_id || '-'}</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">Resource Name</p>
										<p class="mt-1 text-sm text-gray-900">{selectedEntry.resource_name || '-'}</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">User ID</p>
										<p class="mt-1 text-sm text-gray-900">{selectedEntry.user_id || '-'}</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">Client IP</p>
										<p class="mt-1 text-sm text-gray-900">{selectedEntry.client_ip || '-'}</p>
									</div>
									<div>
										<p class="text-sm font-medium text-gray-500">User Agent</p>
										<p class="mt-1 text-sm text-gray-900 truncate">
											{selectedEntry.user_agent || '-'}
										</p>
									</div>
								</div>

								<!-- Configuration Changes -->
								{#if selectedEntry.old_configuration || selectedEntry.new_configuration}
									<div class="space-y-4">
										{#if selectedEntry.old_configuration}
											<div>
												<p class="text-sm font-medium text-gray-500 mb-2">Old Configuration</p>
												<pre
													class="bg-gray-50 border border-gray-200 rounded p-3 text-xs overflow-x-auto max-h-64"><code>{JSON.stringify(tryParseJSON(selectedEntry.old_configuration), null, 2)}</code></pre>
											</div>
										{/if}
										{#if selectedEntry.new_configuration}
											<div>
												<p class="text-sm font-medium text-gray-500 mb-2">New Configuration</p>
												<pre
													class="bg-gray-50 border border-gray-200 rounded p-3 text-xs overflow-x-auto max-h-64"><code>{JSON.stringify(tryParseJSON(selectedEntry.new_configuration), null, 2)}</code></pre>
											</div>
										{/if}
									</div>
								{/if}
							</div>
						</div>
					</div>
				</div>
				<div class="bg-gray-50 px-4 py-3 sm:px-6 sm:flex sm:flex-row-reverse">
					<button
						type="button"
						onclick={closeDetailModal}
						class="w-full inline-flex justify-center rounded-md border border-gray-300 shadow-sm px-4 py-2 bg-white text-base font-medium text-gray-700 hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 sm:ml-3 sm:w-auto sm:text-sm"
					>
						Close
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
