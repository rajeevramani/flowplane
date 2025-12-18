<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { UserResponse } from '$lib/api/types';

	let users = $state<UserResponse[]>([]);
	let total = $state(0);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	// Pagination
	let currentPage = $state(1);
	let pageSize = $state(20);

	// Filters
	let searchQuery = $state('');
	let statusFilter = $state<string>('all');
	let roleFilter = $state<string>('all');

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}
			await loadUsers();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadUsers() {
		isLoading = true;
		error = null;

		try {
			const offset = (currentPage - 1) * pageSize;
			const response = await apiClient.listUsers(pageSize, offset);
			users = response.users;
			total = response.total;
		} catch (err: any) {
			error = err.message || 'Failed to load users';
		} finally {
			isLoading = false;
		}
	}


	let filteredUsers = $derived.by(() => {
		let filtered = users;

		// Apply search filter
		if (searchQuery.trim()) {
			const query = searchQuery.toLowerCase();
			filtered = filtered.filter(
				(user) =>
					user.name.toLowerCase().includes(query) || user.email.toLowerCase().includes(query)
			);
		}

		// Apply status filter
		if (statusFilter !== 'all') {
			filtered = filtered.filter((user) => user.status.toLowerCase() === statusFilter);
		}

		// Apply role filter
		if (roleFilter !== 'all') {
			const isAdmin = roleFilter === 'admin';
			filtered = filtered.filter((user) => user.isAdmin === isAdmin);
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

	function getStatusColor(status: string): string {
		switch (status.toLowerCase()) {
			case 'active':
				return 'bg-green-100 text-green-800';
			case 'suspended':
				return 'bg-red-100 text-red-800';
			case 'inactive':
				return 'bg-gray-100 text-gray-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	function handleNextPage() {
		if (currentPage < totalPages) {
			currentPage++;
			loadUsers();
		}
	}

	function handlePreviousPage() {
		if (currentPage > 1) {
			currentPage--;
			loadUsers();
		}
	}

	function handleCreateUser() {
		goto('/admin/users/create');
	}

	function handleViewUser(userId: string) {
		goto(`/admin/users/${userId}`);
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
					<h1 class="text-xl font-bold text-gray-900">User Management</h1>
				</div>
				<button
					onclick={handleCreateUser}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					Create User
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
			<div class="grid grid-cols-1 md:grid-cols-4 gap-4">
				<!-- Search -->
				<div class="md:col-span-2">
					<label for="search" class="block text-sm font-medium text-gray-700 mb-2">
						Search
					</label>
					<input
						id="search"
						type="text"
						bind:value={searchQuery}
						placeholder="Search by name or email..."
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
						<option value="inactive">Inactive</option>
						<option value="suspended">Suspended</option>
					</select>
				</div>

				<!-- Role Filter -->
				<div>
					<label for="role" class="block text-sm font-medium text-gray-700 mb-2">Role</label>
					<select
						id="role"
						bind:value={roleFilter}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						<option value="all">All Roles</option>
						<option value="admin">Admins</option>
						<option value="developer">Developers</option>
					</select>
				</div>
			</div>
		</div>

		<!-- Users Table -->
		<div class="bg-white rounded-lg shadow-md overflow-hidden">
			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if filteredUsers.length === 0}
				<div class="text-center py-12">
					<p class="text-gray-500">No users found</p>
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
									Email
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Role
								</th>
								<th
									class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
								>
									Status
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
							{#each filteredUsers as user (user.id)}
								<tr class="hover:bg-gray-50">
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm font-medium text-gray-900">{user.name}</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<div class="text-sm text-gray-600">{user.email}</div>
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										{#if user.isAdmin}
											<span
												class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
											>
												Admin
											</span>
										{:else}
											<span
												class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
											>
												Developer
											</span>
										{/if}
									</td>
									<td class="px-6 py-4 whitespace-nowrap">
										<span
											class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getStatusColor(
												user.status
											)}"
										>
											{user.status}
										</span>
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
										{formatDate(user.createdAt)}
									</td>
									<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
										<button
											onclick={() => handleViewUser(user.id)}
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
				<div class="bg-gray-50 px-6 py-4 flex items-center justify-between border-t border-gray-200">
					<div class="text-sm text-gray-700">
						Showing {(currentPage - 1) * pageSize + 1} to {Math.min(
							currentPage * pageSize,
							total
						)} of {total} users
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
