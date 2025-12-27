<script lang="ts">
	import { page } from '$app/stores';
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import {
		ArrowLeft,
		RefreshCw,
		XCircle,
		Clock,
		Play,
		CheckCircle,
		AlertTriangle,
		Calendar,
		Target,
		FileCode
	} from 'lucide-svelte';
	import type { LearningSessionResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import SessionStatusBadge from '$lib/components/learning/SessionStatusBadge.svelte';
	import SessionProgressBar from '$lib/components/learning/SessionProgressBar.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let session = $state<LearningSessionResponse | null>(null);
	let pollingInterval: ReturnType<typeof setInterval> | null = null;

	const sessionId = $page.params.id ?? '';

	onMount(async () => {
		if (!sessionId) {
			error = 'Session ID is required';
			isLoading = false;
			return;
		}
		await loadSession();
	});

	onDestroy(() => {
		if (pollingInterval) {
			clearInterval(pollingInterval);
		}
	});

	async function loadSession() {
		isLoading = true;
		error = null;

		try {
			session = await apiClient.getLearningSession(sessionId);

			// Auto-refresh for active sessions
			if (session.status === 'active' || session.status === 'completing') {
				startPolling();
			} else {
				stopPolling();
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load session';
			console.error('Failed to load session:', e);
		} finally {
			isLoading = false;
		}
	}

	function startPolling() {
		if (pollingInterval) return;
		pollingInterval = setInterval(async () => {
			try {
				session = await apiClient.getLearningSession(sessionId);
				if (session.status !== 'active' && session.status !== 'completing') {
					stopPolling();
				}
			} catch (e) {
				console.error('Polling failed:', e);
			}
		}, 3000);
	}

	function stopPolling() {
		if (pollingInterval) {
			clearInterval(pollingInterval);
			pollingInterval = null;
		}
	}

	async function handleCancel() {
		if (!session) return;
		if (
			!confirm(
				'Are you sure you want to cancel this session? This will stop traffic capture.'
			)
		) {
			return;
		}

		actionError = null;
		try {
			await apiClient.cancelLearningSession(session.id);
			await loadSession();
		} catch (err) {
			actionError = err instanceof Error ? err.message : 'Failed to cancel session';
		}
	}

	function formatDate(dateStr: string | null): string {
		if (!dateStr) return '-';
		const date = new Date(dateStr);
		return date.toLocaleString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit'
		});
	}

	function formatDuration(startStr: string | null, endStr: string | null): string {
		if (!startStr) return '-';
		const start = new Date(startStr);
		const end = endStr ? new Date(endStr) : new Date();
		const diff = end.getTime() - start.getTime();
		const seconds = Math.floor(diff / 1000);
		const minutes = Math.floor(seconds / 60);
		const hours = Math.floor(minutes / 60);

		if (hours > 0) {
			return `${hours}h ${minutes % 60}m`;
		}
		if (minutes > 0) {
			return `${minutes}m ${seconds % 60}s`;
		}
		return `${seconds}s`;
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Back Button -->
	<button
		onclick={() => goto('/learning')}
		class="flex items-center gap-2 text-gray-600 hover:text-gray-900 mb-6 transition-colors"
	>
		<ArrowLeft class="h-4 w-4" />
		Back to Sessions
	</button>

	{#if isLoading}
		<div class="flex justify-center items-center py-12">
			<RefreshCw class="h-8 w-8 animate-spin text-gray-400" />
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
			{error}
		</div>
	{:else if session}
		<!-- Header -->
		<div class="mb-8 flex items-start justify-between">
			<div>
				<div class="flex items-center gap-3">
					<h1 class="text-3xl font-bold text-gray-900">Learning Session</h1>
					<SessionStatusBadge status={session.status} size="md" />
				</div>
				<p class="mt-2 text-sm text-gray-500 font-mono">{session.id}</p>
			</div>

			{#if session.status === 'active' || session.status === 'pending'}
				<Button onclick={handleCancel} variant="danger">
					<XCircle class="h-4 w-4 mr-2" />
					Cancel Session
				</Button>
			{/if}
		</div>

		<!-- Action Error -->
		{#if actionError}
			<div class="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
				{actionError}
			</div>
		{/if}

		<!-- Auto-refresh indicator -->
		{#if session.status === 'active' || session.status === 'completing'}
			<div class="mb-6 bg-blue-50 border border-blue-200 rounded-lg p-3 flex items-center gap-2 text-blue-700">
				<RefreshCw class="h-4 w-4 animate-spin" />
				<span class="text-sm">Auto-refreshing every 3 seconds...</span>
			</div>
		{/if}

		<!-- Progress Card -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
				<Target class="h-5 w-5 text-gray-500" />
				Progress
			</h2>
			<SessionProgressBar
				current={session.currentSampleCount}
				target={session.targetSampleCount}
				size="lg"
				animated={session.status === 'active'}
			/>

			{#if session.errorMessage}
				<div
					class="mt-4 bg-red-50 border border-red-200 rounded-lg p-3 flex items-start gap-2"
				>
					<AlertTriangle class="h-5 w-5 text-red-500 flex-shrink-0 mt-0.5" />
					<div>
						<p class="font-medium text-red-800">Session Failed</p>
						<p class="text-sm text-red-700">{session.errorMessage}</p>
					</div>
				</div>
			{/if}
		</div>

		<!-- Details Grid -->
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
			<!-- Traffic Matching -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
					<FileCode class="h-5 w-5 text-gray-500" />
					Traffic Matching
				</h2>

				<dl class="space-y-4">
					<div>
						<dt class="text-sm font-medium text-gray-500">Route Pattern</dt>
						<dd class="mt-1 text-sm font-mono bg-gray-50 p-2 rounded">
							{session.routePattern}
						</dd>
					</div>

					{#if session.clusterName}
						<div>
							<dt class="text-sm font-medium text-gray-500">Cluster</dt>
							<dd class="mt-1 text-sm text-gray-900">{session.clusterName}</dd>
						</div>
					{/if}

					{#if session.httpMethods && session.httpMethods.length > 0}
						<div>
							<dt class="text-sm font-medium text-gray-500">HTTP Methods</dt>
							<dd class="mt-1 flex gap-2">
								{#each session.httpMethods as method}
									<span
										class="px-2 py-1 text-xs font-medium bg-gray-100 text-gray-700 rounded"
									>
										{method}
									</span>
								{/each}
							</dd>
						</div>
					{:else}
						<div>
							<dt class="text-sm font-medium text-gray-500">HTTP Methods</dt>
							<dd class="mt-1 text-sm text-gray-500">All methods</dd>
						</div>
					{/if}
				</dl>
			</div>

			<!-- Timeline -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
					<Clock class="h-5 w-5 text-gray-500" />
					Timeline
				</h2>

				<div class="space-y-4">
					<!-- Created -->
					<div class="flex items-start gap-3">
						<div class="flex-shrink-0">
							<div class="w-8 h-8 rounded-full bg-gray-100 flex items-center justify-center">
								<Calendar class="h-4 w-4 text-gray-500" />
							</div>
						</div>
						<div>
							<p class="text-sm font-medium text-gray-900">Created</p>
							<p class="text-sm text-gray-500">{formatDate(session.createdAt)}</p>
						</div>
					</div>

					<!-- Started -->
					{#if session.startedAt}
						<div class="flex items-start gap-3">
							<div class="flex-shrink-0">
								<div class="w-8 h-8 rounded-full bg-blue-100 flex items-center justify-center">
									<Play class="h-4 w-4 text-blue-600" />
								</div>
							</div>
							<div>
								<p class="text-sm font-medium text-gray-900">Started</p>
								<p class="text-sm text-gray-500">{formatDate(session.startedAt)}</p>
							</div>
						</div>
					{/if}

					<!-- Duration -->
					{#if session.startedAt}
						<div class="flex items-start gap-3">
							<div class="flex-shrink-0">
								<div class="w-8 h-8 rounded-full bg-gray-100 flex items-center justify-center">
									<Clock class="h-4 w-4 text-gray-500" />
								</div>
							</div>
							<div>
								<p class="text-sm font-medium text-gray-900">Duration</p>
								<p class="text-sm text-gray-500">
									{formatDuration(session.startedAt, session.completedAt)}
								</p>
							</div>
						</div>
					{/if}

					<!-- Ends At -->
					{#if session.endsAt && session.status === 'active'}
						<div class="flex items-start gap-3">
							<div class="flex-shrink-0">
								<div class="w-8 h-8 rounded-full bg-yellow-100 flex items-center justify-center">
									<AlertTriangle class="h-4 w-4 text-yellow-600" />
								</div>
							</div>
							<div>
								<p class="text-sm font-medium text-gray-900">Timeout</p>
								<p class="text-sm text-gray-500">{formatDate(session.endsAt)}</p>
							</div>
						</div>
					{/if}

					<!-- Completed -->
					{#if session.completedAt}
						<div class="flex items-start gap-3">
							<div class="flex-shrink-0">
								<div
									class="w-8 h-8 rounded-full flex items-center justify-center {session.status ===
									'completed'
										? 'bg-green-100'
										: 'bg-gray-100'}"
								>
									<CheckCircle
										class="h-4 w-4 {session.status === 'completed'
											? 'text-green-600'
											: 'text-gray-500'}"
									/>
								</div>
							</div>
							<div>
								<p class="text-sm font-medium text-gray-900">
									{session.status === 'completed' ? 'Completed' : 'Ended'}
								</p>
								<p class="text-sm text-gray-500">{formatDate(session.completedAt)}</p>
							</div>
						</div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Metadata -->
		{#if session.triggeredBy || session.deploymentVersion}
			<div class="mt-6 bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata</h2>
				<dl class="grid grid-cols-1 sm:grid-cols-2 gap-4">
					{#if session.triggeredBy}
						<div>
							<dt class="text-sm font-medium text-gray-500">Triggered By</dt>
							<dd class="mt-1 text-sm text-gray-900">{session.triggeredBy}</dd>
						</div>
					{/if}
					{#if session.deploymentVersion}
						<div>
							<dt class="text-sm font-medium text-gray-500">Deployment Version</dt>
							<dd class="mt-1 text-sm text-gray-900">{session.deploymentVersion}</dd>
						</div>
					{/if}
				</dl>
			</div>
		{/if}
	{/if}
</div>
