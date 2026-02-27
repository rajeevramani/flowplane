<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import {
		Plus,
		BookOpen,
		Play,
		CheckCircle,
		XCircle,
		RefreshCw,
		Search,
		Eye
	} from 'lucide-svelte';
	import type { LearningSessionResponse, LearningSessionStatus, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import SessionStatusBadge from '$lib/components/learning/SessionStatusBadge.svelte';
	import SessionProgressBar from '$lib/components/learning/SessionProgressBar.svelte';
	import { canWriteLearningSessions, canDeleteLearningSessions } from '$lib/utils/permissions';
	import { handleApiError } from '$lib/utils/errorHandling';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let statusFilter = $state<LearningSessionStatus | ''>('');
	let currentTeam = $state<string>('');
	let pollingEnabled = $state(false);
	let pollingInterval: ReturnType<typeof setInterval> | null = null;
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	// Data
	let sessions = $state<LearningSessionResponse[]>([]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
		} catch (e) {
			console.error('Failed to load session info:', e);
		}
		await loadData();
	});

	onDestroy(() => {
		if (pollingInterval) {
			clearInterval(pollingInterval);
		}
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const query: import('$lib/api/types').ListLearningSessionsQuery = {
				team: currentTeam || undefined,
				status: statusFilter || undefined,
			};
			sessions = await apiClient.listLearningSessions(query);

			// Auto-enable polling if there are active sessions
			const hasActiveSessions = sessions.some(
				(s) => s.status === 'active' || s.status === 'completing'
			);
			if (hasActiveSessions && !pollingEnabled) {
				startPolling();
			} else if (!hasActiveSessions && pollingEnabled) {
				stopPolling();
			}
		} catch (e) {
			const apiError = handleApiError(e, 'load learning sessions');
			error = apiError.userMessage;
			console.error('Failed to load learning sessions:', e);
		} finally {
			isLoading = false;
		}
	}

	function startPolling() {
		pollingEnabled = true;
		pollingInterval = setInterval(async () => {
			try {
				const query: import('$lib/api/types').ListLearningSessionsQuery = {
					team: currentTeam || undefined,
					status: statusFilter || undefined,
				};
				sessions = await apiClient.listLearningSessions(query);

				// Stop polling if no more active sessions
				const hasActiveSessions = sessions.some(
					(s) => s.status === 'active' || s.status === 'completing'
				);
				if (!hasActiveSessions) {
					stopPolling();
				}
			} catch (e) {
				console.error('Polling failed:', e);
			}
		}, 5000);
	}

	function stopPolling() {
		pollingEnabled = false;
		if (pollingInterval) {
			clearInterval(pollingInterval);
			pollingInterval = null;
		}
	}

	// Calculate stats
	let stats = $derived.by(() => {
		const active = sessions.filter((s) => s.status === 'active').length;
		const completed = sessions.filter((s) => s.status === 'completed').length;
		const failed = sessions.filter((s) => s.status === 'failed' || s.status === 'cancelled')
			.length;
		return { total: sessions.length, active, completed, failed };
	});

	// Filter sessions by search
	let filteredSessions = $derived(
		sessions.filter(
			(session) =>
				!searchQuery ||
				session.routePattern.toLowerCase().includes(searchQuery.toLowerCase()) ||
				session.id.toLowerCase().includes(searchQuery.toLowerCase()) ||
				(session.clusterName &&
					session.clusterName.toLowerCase().includes(searchQuery.toLowerCase()))
		)
	);

	// Navigate to create page
	function handleCreate() {
		goto('/learning/create');
	}

	// Navigate to details page
	function handleView(sessionId: string) {
		goto(`/learning/${encodeURIComponent(sessionId)}`);
	}

	// Cancel session
	async function handleCancel(session: LearningSessionResponse) {
		// Permission check
		if (sessionInfo && !canDeleteLearningSessions(sessionInfo)) {
			actionError = "You don't have permission to cancel learning sessions. Contact your administrator.";
			return;
		}

		if (
			!confirm(
				`Are you sure you want to cancel the session for "${session.routePattern}"? This will stop traffic capture.`
			)
		) {
			return;
		}

		actionError = null;
		try {
			await apiClient.cancelLearningSession(session.id);
			await loadData();
		} catch (err) {
			const apiError = handleApiError(err, 'cancel learning session');
			actionError = apiError.userMessage;
		}
	}

	// Format date
	function formatDate(dateStr: string | null): string {
		if (!dateStr) return '-';
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Format relative time
	function formatRelativeTime(dateStr: string | null): string {
		if (!dateStr) return '-';
		const date = new Date(dateStr);
		const now = new Date();
		const diff = now.getTime() - date.getTime();
		const minutes = Math.floor(diff / 60000);
		const hours = Math.floor(minutes / 60);
		const days = Math.floor(hours / 24);

		if (days > 0) return `${days}d ago`;
		if (hours > 0) return `${hours}h ago`;
		if (minutes > 0) return `${minutes}m ago`;
		return 'Just now';
	}

	// Handle status filter change
	function handleStatusFilter(status: LearningSessionStatus | '') {
		statusFilter = status;
		loadData();
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Learning Sessions</h1>
		<p class="mt-2 text-sm text-gray-600">
			Capture and analyze API traffic to discover schemas for the <span class="font-medium"
				>{currentTeam}</span
			> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6 flex items-center gap-4">
		{#if sessionInfo && canWriteLearningSessions(sessionInfo)}
			<Button onclick={handleCreate} variant="primary">
				<Plus class="h-4 w-4 mr-2" />
				Create Session
			</Button>
		{/if}
		{#if pollingEnabled}
			<span class="text-sm text-gray-500 flex items-center gap-2">
				<RefreshCw class="h-4 w-4 animate-spin" />
				Auto-refreshing...
			</span>
		{/if}
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Sessions</p>
					<p class="text-2xl font-bold text-gray-900">{stats.total}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<BookOpen class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Active</p>
					<p class="text-2xl font-bold text-blue-600">{stats.active}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Play class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Completed</p>
					<p class="text-2xl font-bold text-green-600">{stats.completed}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<CheckCircle class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Failed / Cancelled</p>
					<p class="text-2xl font-bold text-gray-600">{stats.failed}</p>
				</div>
				<div class="p-3 bg-gray-100 rounded-lg">
					<XCircle class="h-6 w-6 text-gray-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Filters Row -->
	<div class="mb-6 flex flex-col sm:flex-row gap-4">
		<!-- Search -->
		<div class="relative flex-1">
			<Search class="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-gray-400" />
			<input
				type="text"
				bind:value={searchQuery}
				placeholder="Search by route pattern, ID, or cluster..."
				class="w-full pl-10 pr-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
			/>
		</div>

		<!-- Status Filter -->
		<select
			value={statusFilter}
			onchange={(e) => handleStatusFilter(e.currentTarget.value as LearningSessionStatus | '')}
			class="px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 bg-white"
		>
			<option value="">All Statuses</option>
			<option value="pending">Pending</option>
			<option value="active">Active</option>
			<option value="completing">Completing</option>
			<option value="completed">Completed</option>
			<option value="cancelled">Cancelled</option>
			<option value="failed">Failed</option>
		</select>
	</div>

	<!-- Error Messages -->
	{#if error}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
			{error}
		</div>
	{/if}

	{#if actionError}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
			{actionError}
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex justify-center items-center py-12">
			<RefreshCw class="h-8 w-8 animate-spin text-gray-400" />
		</div>
	{:else if filteredSessions.length === 0}
		<div class="text-center py-12 bg-white rounded-lg border border-gray-200">
			<BookOpen class="h-12 w-12 mx-auto text-gray-400 mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">No learning sessions found</h3>
			<p class="text-gray-500 mb-4">
				{searchQuery || statusFilter
					? 'No sessions match your filters.'
					: 'Create a learning session to start capturing API traffic.'}
			</p>
			{#if !searchQuery && !statusFilter && sessionInfo && canWriteLearningSessions(sessionInfo)}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Session
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Sessions Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Route Pattern
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Status
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Progress
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Created
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredSessions as session}
						<tr class="hover:bg-gray-50">
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<code class="text-sm font-mono text-gray-900">{session.routePattern}</code>
									{#if session.clusterName}
										<span class="text-xs text-gray-500 mt-1">Cluster: {session.clusterName}</span>
									{/if}
									{#if session.httpMethods && session.httpMethods.length > 0}
										<span class="text-xs text-gray-500 mt-1">
											Methods: {session.httpMethods.join(', ')}
										</span>
									{/if}
								</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<SessionStatusBadge status={session.status} />
							</td>
							<td class="px-6 py-4">
								<div class="w-48">
									<SessionProgressBar
										current={session.currentSampleCount}
										target={session.targetSampleCount}
										size="sm"
										animated={session.status === 'active'}
									/>
								</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
								{formatRelativeTime(session.createdAt)}
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleView(session.id)}
										class="p-2 text-gray-500 hover:text-blue-600 hover:bg-blue-50 rounded-lg transition-colors"
										title="View details"
									>
										<Eye class="h-4 w-4" />
									</button>
									{#if (session.status === 'active' || session.status === 'pending') && sessionInfo && canDeleteLearningSessions(sessionInfo)}
										<button
											onclick={() => handleCancel(session)}
											class="p-2 text-gray-500 hover:text-red-600 hover:bg-red-50 rounded-lg transition-colors"
											title="Cancel session"
										>
											<XCircle class="h-4 w-4" />
										</button>
									{/if}
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
