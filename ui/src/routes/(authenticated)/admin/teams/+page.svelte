<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { TeamResponse, TeamStatus } from '$lib/api/types';

	let teams = $state<TeamResponse[]>([]);
	let total = $state(0);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	// Pagination
	let currentPage = $state(1);
	let pageSize = $state(20);

	// Filters
	let searchQuery = $state('');
	let statusFilter = $state<string>('all');

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}
			await loadTeams();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadTeams() {
		isLoading = true;
		error = null;

		try {
			const offset = (currentPage - 1) * pageSize;
			const response = await apiClient.adminListTeams(pageSize, offset);
			teams = response.teams;
			total = response.total;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load teams';
		} finally {
			isLoading = false;
		}
	}

	let filteredTeams = $derived.by(() => {
		let filtered = teams;

		// Apply search filter
		if (searchQuery.trim()) {
			const query = searchQuery.toLowerCase();
			filtered = filtered.filter(
				(team) =>
					team.name.toLowerCase().includes(query) ||
					team.displayName.toLowerCase().includes(query) ||
					(team.description && team.description.toLowerCase().includes(query))
			);
		}

		// Apply status filter
		if (statusFilter !== 'all') {
			filtered = filtered.filter((team) => team.status === statusFilter);
		}

		return filtered;
	});

	let totalPages = $derived.by(() => Math.ceil(total / pageSize));

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	function getStatusColor(status: TeamStatus): string {
		switch (status) {
			case 'active':
				return 'bg-green-100 text-green-800';
			case 'suspended':
				return 'bg-yellow-100 text-yellow-800';
			case 'archived':
				return 'bg-gray-100 text-gray-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	function handleNextPage() {
		if (currentPage < totalPages) {
			currentPage++;
			loadTeams();
		}
	}

	function handlePreviousPage() {
		if (currentPage > 1) {
			currentPage--;
			loadTeams();
		}
	}

	function handleCreateTeam() {
		goto('/admin/teams/create');
	}

	function handleViewTeam(teamId: string) {
		goto(`/admin/teams/${teamId}`);
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/dashboard"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to dashboard"
					>
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Team Management</h1>
				</div>
				<button
					onclick={handleCreateTeam}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					Create Team
				</button>
			</div>
		</div>
	</nav>

	<main class="w-full px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<!-- Filters Section -->
		<div class="bg-white rounded-lg shadow-md p-6 mb-6">
			<div class="grid grid-cols-1 md:grid-cols-3 gap-4">
				<!-- Search -->
				<div class="md:col-span-2">
					<label for="search" class="block text-sm font-medium text-gray-700 mb-2">
						Search
					</label>
					<input
						id="search"
						type="text"
						bind:value={searchQuery}
						placeholder="Search by name or description..."
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>

				<!-- Status Filter -->
				<div>
					<label for="status" class="block text-sm font-medium text-gray-700 mb-2">
						Status
					</label>
					<select
						id="status"
						bind:value={statusFilter}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						<option value="all">All Statuses</option>
						<option value="active">Active</option>
						<option value="suspended">Suspended</option>
						<option value="archived">Archived</option>
					</select>
				</div>
			</div>
		</div>

		<!-- Teams Table -->
		<div class="bg-white rounded-lg shadow-md overflow-hidden">
			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if filteredTeams.length === 0}
				<div class="text-center py-12">
					<p class="text-gray-500">No teams found</p>
				</div>
			{:else}
				<div class="overflow-x-auto">
					<table class="min-w-full divide-y divide-gray-200">
						<thead class="bg-gray-50">
							<tr>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Name
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Display Name
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Description
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Organization
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Status
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Admin Port
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Created
								</th>
								<th
									class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Actions
								</th>
							</tr>
						</thead>
						<tbody class="bg-white divide-y divide-gray-200">
							{#each filteredTeams as team (team.id)}
								<tr class="hover:bg-gray-50">
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm font-medium text-gray-900">{team.name}</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm text-gray-900">{team.displayName}</div>
									</td>
									<td class="px-6 py-4">
										<div class="text-sm text-gray-600 max-w-xs truncate">
											{team.description || '-'}
										</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										{#if team.orgId}
											<span class="text-sm text-indigo-600 font-medium">{team.orgId}</span>
										{:else}
											<span class="text-sm text-gray-400">-</span>
										{/if}
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<span
											class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getStatusColor(
												team.status
											)}"
										>
											{team.status.charAt(0).toUpperCase() + team.status.slice(1)}
										</span>
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600 font-mono">
										{team.envoyAdminPort ?? '-'}
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
										{formatDate(team.createdAt)}
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
										<button
											onclick={() => handleViewTeam(team.id)}
											class="text-blue-600 hover:text-blue-900"
										>
											View
										</button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>

				<!-- Pagination -->
				<div
					class="bg-gray-50 px-6 py-4 flex items-center justify-between border-t border-gray-200"
				>
					<div class="text-sm text-gray-700">
						Showing {(currentPage - 1) * pageSize + 1} to {Math.min(currentPage * pageSize, total)}
						of {total} teams
					</div>
					<div class="flex gap-2">
						<button
							onclick={handlePreviousPage}
							disabled={currentPage === 1}
							class="px-3 py-1 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
						>
							Previous
						</button>
						<span class="px-3 py-1 text-sm text-gray-700">
							Page {currentPage} of {totalPages}
						</span>
						<button
							onclick={handleNextPage}
							disabled={currentPage >= totalPages}
							class="px-3 py-1 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
						>
							Next
						</button>
					</div>
				</div>
			{/if}
		</div>
	</main>
</div>
