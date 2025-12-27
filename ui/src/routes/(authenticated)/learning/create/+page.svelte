<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { ArrowLeft, AlertCircle, CheckCircle } from 'lucide-svelte';
	import type { CreateLearningSessionRequest } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';

	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
	let regexError = $state<string | null>(null);

	// Form state
	let routePattern = $state('^/api/.*');
	let clusterName = $state('');
	let httpMethods = $state<string[]>([]);
	let targetSampleCount = $state(100);
	let maxDurationSeconds = $state<number | null>(null);
	let triggeredBy = $state('');
	let deploymentVersion = $state('');

	// HTTP method options
	const availableMethods = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS'];

	// Toggle HTTP method selection
	function toggleMethod(method: string) {
		if (httpMethods.includes(method)) {
			httpMethods = httpMethods.filter((m) => m !== method);
		} else {
			httpMethods = [...httpMethods, method];
		}
	}

	// Validate regex pattern
	function validateRegex() {
		regexError = null;
		if (!routePattern.trim()) {
			regexError = 'Route pattern is required';
			return false;
		}
		try {
			new RegExp(routePattern);
			return true;
		} catch (e) {
			regexError = `Invalid regex: ${e instanceof Error ? e.message : 'Unknown error'}`;
			return false;
		}
	}

	// Handle form submission
	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = null;

		if (!validateRegex()) {
			return;
		}

		if (targetSampleCount < 1 || targetSampleCount > 100000) {
			error = 'Target sample count must be between 1 and 100,000';
			return;
		}

		isSubmitting = true;

		try {
			const request: CreateLearningSessionRequest = {
				routePattern: routePattern.trim(),
				targetSampleCount
			};

			if (clusterName.trim()) {
				request.clusterName = clusterName.trim();
			}

			if (httpMethods.length > 0) {
				request.httpMethods = httpMethods;
			}

			if (maxDurationSeconds && maxDurationSeconds > 0) {
				request.maxDurationSeconds = maxDurationSeconds;
			}

			if (triggeredBy.trim()) {
				request.triggeredBy = triggeredBy.trim();
			}

			if (deploymentVersion.trim()) {
				request.deploymentVersion = deploymentVersion.trim();
			}

			const session = await apiClient.createLearningSession(request);
			goto(`/learning/${encodeURIComponent(session.id)}`);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to create learning session';
		} finally {
			isSubmitting = false;
		}
	}

	// Handle pattern input change
	function handlePatternChange(e: Event) {
		routePattern = (e.target as HTMLInputElement).value;
		// Clear error on change, will re-validate on blur
		regexError = null;
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

	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Create Learning Session</h1>
		<p class="mt-2 text-sm text-gray-600">
			Configure a session to capture and analyze API traffic
		</p>
	</div>

	<!-- Error -->
	{#if error}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-lg p-4 flex items-start gap-3">
			<AlertCircle class="h-5 w-5 text-red-500 flex-shrink-0 mt-0.5" />
			<div class="text-red-700">{error}</div>
		</div>
	{/if}

	<!-- Form -->
	<form onsubmit={handleSubmit} class="max-w-2xl space-y-6">
		<!-- Route Pattern -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Traffic Matching</h2>

			<div class="space-y-4">
				<div>
					<label for="routePattern" class="block text-sm font-medium text-gray-700 mb-1">
						Route Pattern (Regex) <span class="text-red-500">*</span>
					</label>
					<input
						id="routePattern"
						type="text"
						value={routePattern}
						oninput={handlePatternChange}
						onblur={validateRegex}
						placeholder="^/api/v1/users/.*"
						class="w-full px-4 py-2 border rounded-lg font-mono text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500 {regexError
							? 'border-red-300 bg-red-50'
							: 'border-gray-300'}"
					/>
					{#if regexError}
						<p class="mt-1 text-sm text-red-600">{regexError}</p>
					{:else}
						<p class="mt-1 text-sm text-gray-500">
							Regular expression to match request paths. Examples: <code>^/api/.*</code>,
							<code>^/users/[0-9]+</code>
						</p>
					{/if}
				</div>

				<div>
					<label for="clusterName" class="block text-sm font-medium text-gray-700 mb-1">
						Cluster Name (Optional)
					</label>
					<input
						id="clusterName"
						type="text"
						bind:value={clusterName}
						placeholder="api-cluster"
						class="w-full px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="mt-1 text-sm text-gray-500">
						Only capture traffic destined for this cluster
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2"> HTTP Methods (Optional) </label>
					<div class="flex flex-wrap gap-2">
						{#each availableMethods as method}
							<button
								type="button"
								onclick={() => toggleMethod(method)}
								class="px-3 py-1.5 text-sm font-medium rounded-lg border transition-colors {httpMethods.includes(
									method
								)
									? 'bg-blue-100 border-blue-300 text-blue-800'
									: 'bg-gray-50 border-gray-300 text-gray-700 hover:bg-gray-100'}"
							>
								{method}
							</button>
						{/each}
					</div>
					<p class="mt-2 text-sm text-gray-500">
						Leave empty to capture all HTTP methods
					</p>
				</div>
			</div>
		</div>

		<!-- Session Configuration -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Session Configuration</h2>

			<div class="space-y-4">
				<div>
					<label for="targetSampleCount" class="block text-sm font-medium text-gray-700 mb-1">
						Target Sample Count <span class="text-red-500">*</span>
					</label>
					<input
						id="targetSampleCount"
						type="number"
						bind:value={targetSampleCount}
						min="1"
						max="100000"
						class="w-full px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="mt-1 text-sm text-gray-500">
						Session completes after capturing this many samples (1 - 100,000)
					</p>
				</div>

				<div>
					<label for="maxDurationSeconds" class="block text-sm font-medium text-gray-700 mb-1">
						Maximum Duration (seconds)
					</label>
					<input
						id="maxDurationSeconds"
						type="number"
						bind:value={maxDurationSeconds}
						min="60"
						placeholder="3600"
						class="w-full px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="mt-1 text-sm text-gray-500">
						Session times out after this duration (minimum 60 seconds). Leave empty for no timeout.
					</p>
				</div>
			</div>
		</div>

		<!-- Metadata -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata (Optional)</h2>

			<div class="space-y-4">
				<div>
					<label for="triggeredBy" class="block text-sm font-medium text-gray-700 mb-1">
						Triggered By
					</label>
					<input
						id="triggeredBy"
						type="text"
						bind:value={triggeredBy}
						placeholder="deploy-pipeline, manual, ci-cd"
						class="w-full px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="mt-1 text-sm text-gray-500">
						Source that triggered this session (for tracking)
					</p>
				</div>

				<div>
					<label for="deploymentVersion" class="block text-sm font-medium text-gray-700 mb-1">
						Deployment Version
					</label>
					<input
						id="deploymentVersion"
						type="text"
						bind:value={deploymentVersion}
						placeholder="v1.2.3, commit-abc123"
						class="w-full px-4 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="mt-1 text-sm text-gray-500">
						Version of the API being learned
					</p>
				</div>
			</div>
		</div>

		<!-- Submit Buttons -->
		<div class="flex gap-4">
			<Button type="button" variant="secondary" onclick={() => goto('/learning')}>
				Cancel
			</Button>
			<Button type="submit" variant="primary" disabled={isSubmitting}>
				{#if isSubmitting}
					Creating...
				{:else}
					<CheckCircle class="h-4 w-4 mr-2" />
					Create Session
				{/if}
			</Button>
		</div>
	</form>
</div>
