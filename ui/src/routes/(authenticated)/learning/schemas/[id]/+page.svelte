<script lang="ts">
	import { page } from '$app/stores';
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		ArrowLeft,
		RefreshCw,
		Download,
		AlertTriangle,
		Calendar,
		BarChart3,
		FileCode,
		GitCompare,
		CheckCircle
	} from 'lucide-svelte';
	import type { AggregatedSchemaResponse, SchemaComparisonResponse, SessionInfoResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import { canReadSchemas } from '$lib/utils/permissions';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let schema = $state<AggregatedSchemaResponse | null>(null);
	let comparison = $state<SchemaComparisonResponse | null>(null);
	let isLoadingComparison = $state(false);
	let isExporting = $state(false);
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	// Tab state
	let activeTab = $state<'request' | 'response' | 'comparison'>('request');

	const schemaId = Number($page.params.id);

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
		} catch (e) {
			console.error('Failed to load session info:', e);
		}
		await loadSchema();
	});

	async function loadSchema() {
		isLoading = true;
		error = null;

		try {
			schema = await apiClient.getAggregatedSchema(schemaId);

			// Auto-load comparison if there's a previous version
			if (schema.previousVersionId) {
				await loadComparison();
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load schema';
			console.error('Failed to load schema:', e);
		} finally {
			isLoading = false;
		}
	}

	async function loadComparison() {
		if (!schema?.previousVersionId) return;

		isLoadingComparison = true;
		try {
			comparison = await apiClient.compareSchemaVersions(schemaId, schema.previousVersionId);
		} catch (e) {
			console.error('Failed to load comparison:', e);
		} finally {
			isLoadingComparison = false;
		}
	}

	async function handleExport(includeExamples: boolean = false) {
		if (!schema) return;

		// Permission check
		if (sessionInfo && !canReadSchemas(sessionInfo)) {
			error = "You don't have permission to export schemas. Contact your administrator.";
			return;
		}

		isExporting = true;
		try {
			const openapi = await apiClient.exportSchemaAsOpenApi(schema.id, includeExamples);
			const blob = new Blob([JSON.stringify(openapi, null, 2)], { type: 'application/json' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${schema.path.replace(/\//g, '_')}_${schema.httpMethod}.openapi.json`;
			a.click();
			URL.revokeObjectURL(url);
		} catch (e) {
			console.error('Failed to export schema:', e);
		} finally {
			isExporting = false;
		}
	}

	// Format confidence as percentage
	function formatConfidence(score: number): string {
		return `${(score * 100).toFixed(0)}%`;
	}

	// Get confidence color
	function getConfidenceVariant(score: number): 'green' | 'yellow' | 'red' {
		if (score >= 0.9) return 'green';
		if (score >= 0.7) return 'yellow';
		return 'red';
	}

	// HTTP method colors
	const methodColors: Record<string, 'blue' | 'green' | 'yellow' | 'red' | 'purple' | 'gray'> = {
		GET: 'blue',
		POST: 'green',
		PUT: 'yellow',
		DELETE: 'red',
		PATCH: 'purple',
		HEAD: 'gray',
		OPTIONS: 'gray'
	};

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Format JSON for display
	function formatJson(obj: unknown): string {
		if (!obj) return 'null';
		return JSON.stringify(obj, null, 2);
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Back Button -->
	<button
		onclick={() => goto('/learning/schemas')}
		class="flex items-center gap-2 text-gray-600 hover:text-gray-900 mb-6 transition-colors"
	>
		<ArrowLeft class="h-4 w-4" />
		Back to Schemas
	</button>

	{#if isLoading}
		<div class="flex justify-center items-center py-12">
			<RefreshCw class="h-8 w-8 animate-spin text-gray-400" />
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-lg p-4 text-red-700">
			{error}
		</div>
	{:else if schema}
		<!-- Header -->
		<div class="mb-8 flex items-start justify-between">
			<div>
				<div class="flex items-center gap-3 flex-wrap">
					<Badge variant={methodColors[schema.httpMethod] || 'gray'}>
						{schema.httpMethod}
					</Badge>
					<h1 class="text-2xl font-bold text-gray-900 font-mono">{schema.path}</h1>
				</div>
				<p class="mt-2 text-sm text-gray-500">
					Version {schema.version} | Team: {schema.team}
				</p>
			</div>

			<div class="flex gap-2">
				{#if sessionInfo && canReadSchemas(sessionInfo)}
					<Button onclick={() => handleExport(false)} variant="secondary" disabled={isExporting}>
						<Download class="h-4 w-4 mr-2" />
						{isExporting ? 'Exporting...' : 'Export OpenAPI'}
					</Button>
				{/if}
			</div>
		</div>

		<!-- Stats Cards -->
		<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">Confidence</p>
						<p class="text-2xl font-bold" class:text-green-600={schema.confidenceScore >= 0.9}
							class:text-yellow-600={schema.confidenceScore >= 0.7 && schema.confidenceScore < 0.9}
							class:text-red-600={schema.confidenceScore < 0.7}>
							{formatConfidence(schema.confidenceScore)}
						</p>
					</div>
					<div class="p-3 rounded-lg" class:bg-green-100={schema.confidenceScore >= 0.9}
						class:bg-yellow-100={schema.confidenceScore >= 0.7 && schema.confidenceScore < 0.9}
						class:bg-red-100={schema.confidenceScore < 0.7}>
						<CheckCircle class="h-6 w-6 {schema.confidenceScore >= 0.9 ? 'text-green-600' : schema.confidenceScore >= 0.7 ? 'text-yellow-600' : 'text-red-600'}" />
					</div>
				</div>
			</div>

			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">Sample Count</p>
						<p class="text-2xl font-bold text-gray-900">{schema.sampleCount.toLocaleString()}</p>
					</div>
					<div class="p-3 bg-blue-100 rounded-lg">
						<BarChart3 class="h-6 w-6 text-blue-600" />
					</div>
				</div>
			</div>

			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">Version</p>
						<p class="text-2xl font-bold text-gray-900">{schema.version}</p>
					</div>
					<div class="p-3 bg-purple-100 rounded-lg">
						<FileCode class="h-6 w-6 text-purple-600" />
					</div>
				</div>
			</div>

			{#if schema.breakingChanges && schema.breakingChanges.length > 0}
				<div class="bg-white rounded-lg shadow-sm border border-orange-200 p-4">
					<div class="flex items-center justify-between">
						<div>
							<p class="text-sm font-medium text-gray-600">Breaking Changes</p>
							<p class="text-2xl font-bold text-orange-600">{schema.breakingChanges.length}</p>
						</div>
						<div class="p-3 bg-orange-100 rounded-lg">
							<AlertTriangle class="h-6 w-6 text-orange-600" />
						</div>
					</div>
				</div>
			{:else}
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
					<div class="flex items-center justify-between">
						<div>
							<p class="text-sm font-medium text-gray-600">Breaking Changes</p>
							<p class="text-2xl font-bold text-green-600">None</p>
						</div>
						<div class="p-3 bg-green-100 rounded-lg">
							<CheckCircle class="h-6 w-6 text-green-600" />
						</div>
					</div>
				</div>
			{/if}
		</div>

		<!-- Breaking Changes Alert -->
		{#if schema.breakingChanges && schema.breakingChanges.length > 0}
			<div class="mb-6 bg-orange-50 border border-orange-200 rounded-lg p-4">
				<div class="flex items-start gap-3">
					<AlertTriangle class="h-5 w-5 text-orange-600 flex-shrink-0 mt-0.5" />
					<div class="flex-1">
						<h3 class="font-medium text-orange-800">Breaking Changes Detected</h3>
						<ul class="mt-2 space-y-2">
							{#each schema.breakingChanges as change}
								<li class="text-sm">
									<span class="font-medium text-orange-700">{change.changeType}</span>
									<span class="text-orange-600"> at </span>
									<code class="bg-orange-100 px-1 py-0.5 rounded text-orange-800">{change.path}</code>
									<p class="text-orange-600 mt-0.5">{change.description}</p>
								</li>
							{/each}
						</ul>
					</div>
				</div>
			</div>
		{/if}

		<!-- Tabs -->
		<div class="mb-6 border-b border-gray-200">
			<nav class="flex gap-4">
				<button
					onclick={() => activeTab = 'request'}
					class="py-3 px-1 border-b-2 font-medium text-sm transition-colors {activeTab === 'request'
						? 'border-blue-600 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
				>
					Request Schema
				</button>
				<button
					onclick={() => activeTab = 'response'}
					class="py-3 px-1 border-b-2 font-medium text-sm transition-colors {activeTab === 'response'
						? 'border-blue-600 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
				>
					Response Schemas
				</button>
				{#if schema.previousVersionId}
					<button
						onclick={() => activeTab = 'comparison'}
						class="py-3 px-1 border-b-2 font-medium text-sm transition-colors flex items-center gap-2 {activeTab === 'comparison'
							? 'border-blue-600 text-blue-600'
							: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
					>
						<GitCompare class="h-4 w-4" />
						Version Compare
					</button>
				{/if}
			</nav>
		</div>

		<!-- Tab Content -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			{#if activeTab === 'request'}
				<div class="p-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Request Schema</h3>
					{#if schema.requestSchema}
						<pre class="bg-gray-50 rounded-lg p-4 overflow-x-auto text-sm font-mono text-gray-800">{formatJson(schema.requestSchema)}</pre>
					{:else}
						<p class="text-gray-500 italic">No request schema captured</p>
					{/if}
				</div>
			{:else if activeTab === 'response'}
				<div class="p-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Response Schemas</h3>
					{#if schema.responseSchemas && Object.keys(schema.responseSchemas).length > 0}
						{#each Object.entries(schema.responseSchemas) as [statusCode, responseSchema]}
							<div class="mb-6 last:mb-0">
								<div class="flex items-center gap-2 mb-2">
									<Badge variant={statusCode.startsWith('2') ? 'green' : statusCode.startsWith('4') ? 'yellow' : 'red'}>
										{statusCode}
									</Badge>
									<span class="text-sm text-gray-500">Response</span>
								</div>
								<pre class="bg-gray-50 rounded-lg p-4 overflow-x-auto text-sm font-mono text-gray-800">{formatJson(responseSchema)}</pre>
							</div>
						{/each}
					{:else}
						<p class="text-gray-500 italic">No response schemas captured</p>
					{/if}
				</div>
			{:else if activeTab === 'comparison'}
				<div class="p-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Version Comparison</h3>
					{#if isLoadingComparison}
						<div class="flex justify-center items-center py-8">
							<RefreshCw class="h-6 w-6 animate-spin text-gray-400" />
						</div>
					{:else if comparison}
						<div class="space-y-6">
							<!-- Version info -->
							<div class="grid grid-cols-2 gap-4 text-sm">
								<div class="bg-gray-50 rounded-lg p-3">
									<span class="text-gray-500">Previous Version:</span>
									<span class="ml-2 font-medium">{comparison.comparedSchema.version}</span>
								</div>
								<div class="bg-gray-50 rounded-lg p-3">
									<span class="text-gray-500">Current Version:</span>
									<span class="ml-2 font-medium">{comparison.currentSchema.version}</span>
								</div>
							</div>

							<!-- Differences summary -->
							<div class="grid grid-cols-3 gap-4 text-sm">
								<div class="bg-gray-50 rounded-lg p-3">
									<span class="text-gray-500">Sample Count Change:</span>
									<span class="ml-2 font-medium" class:text-green-600={comparison.differences.sampleCountChange > 0}
										class:text-red-600={comparison.differences.sampleCountChange < 0}>
										{comparison.differences.sampleCountChange > 0 ? '+' : ''}{comparison.differences.sampleCountChange}
									</span>
								</div>
								<div class="bg-gray-50 rounded-lg p-3">
									<span class="text-gray-500">Confidence Change:</span>
									<span class="ml-2 font-medium" class:text-green-600={comparison.differences.confidenceChange > 0}
										class:text-red-600={comparison.differences.confidenceChange < 0}>
										{comparison.differences.confidenceChange > 0 ? '+' : ''}{(comparison.differences.confidenceChange * 100).toFixed(1)}%
									</span>
								</div>
								<div class="bg-gray-50 rounded-lg p-3">
									<span class="text-gray-500">Version Change:</span>
									<span class="ml-2 font-medium">+{comparison.differences.versionChange}</span>
								</div>
							</div>

							<!-- Breaking changes -->
							{#if comparison.differences.hasBreakingChanges && comparison.differences.breakingChanges && comparison.differences.breakingChanges.length > 0}
								<div>
									<h4 class="font-medium text-gray-900 mb-2">Breaking Changes</h4>
									<ul class="space-y-2">
										{#each comparison.differences.breakingChanges as change}
											<li class="flex items-start gap-2 text-sm bg-red-50 p-3 rounded-lg">
												<AlertTriangle class="h-4 w-4 text-red-500 flex-shrink-0 mt-0.5" />
												<pre class="text-red-700 text-xs overflow-x-auto">{JSON.stringify(change, null, 2)}</pre>
											</li>
										{/each}
									</ul>
								</div>
							{:else}
								<div class="flex items-center gap-2 text-green-600 bg-green-50 p-3 rounded-lg">
									<CheckCircle class="h-5 w-5" />
									<span>No breaking changes between versions</span>
								</div>
							{/if}
						</div>
					{:else}
						<p class="text-gray-500 italic">Unable to load version comparison</p>
					{/if}
				</div>
			{/if}
		</div>

		<!-- Metadata -->
		<div class="mt-6 bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4 flex items-center gap-2">
				<Calendar class="h-5 w-5 text-gray-500" />
				Timeline
			</h2>
			<dl class="grid grid-cols-1 sm:grid-cols-3 gap-4">
				<div>
					<dt class="text-sm font-medium text-gray-500">First Observed</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(schema.firstObserved)}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Last Observed</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(schema.lastObserved)}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Last Updated</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(schema.updatedAt)}</dd>
				</div>
			</dl>
		</div>
	{/if}
</div>
