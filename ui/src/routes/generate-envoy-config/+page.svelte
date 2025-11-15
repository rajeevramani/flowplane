<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import hljs from 'highlight.js/lib/core';
	import yaml from 'highlight.js/lib/languages/yaml';
	import json from 'highlight.js/lib/languages/json';
	import 'highlight.js/styles/github-dark.css';

	// Register languages
	hljs.registerLanguage('yaml', yaml);
	hljs.registerLanguage('json', json);

	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let teams = $state<string[]>([]);
	let selectedTeam = $state('');
	let format = $state<'yaml' | 'json'>('yaml');
	let includeDefault = $state(false);
	let bootstrapConfig = $state('');
	let highlightedCode = $state('');
	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let copySuccess = $state(false);

	onMount(async () => {
		// Check authentication and get user info
		try {
			sessionInfo = await apiClient.getSessionInfo();

			// Fetch teams based on user role
			// Admin users get all teams, non-admin users get only their teams
			const teamsResponse = await apiClient.listTeams();
			teams = teamsResponse.teams;

			// Set first team as default
			if (teams.length > 0) {
				selectedTeam = teams[0];
				await loadBootstrapConfig();
			}
		} catch (err) {
			goto('/login');
		}
	});

	async function loadBootstrapConfig() {
		if (!selectedTeam) return;

		isLoading = true;
		error = null;

		try {
			bootstrapConfig = await apiClient.getBootstrapConfig({
				team: selectedTeam,
				format,
				includeDefault
			});

			// Highlight the code
			const language = format === 'yaml' ? 'yaml' : 'json';
			highlightedCode = hljs.highlight(bootstrapConfig, { language }).value;
		} catch (err: any) {
			error = err.message || 'Failed to load bootstrap configuration';
		} finally {
			isLoading = false;
		}
	}

	function handleTeamChange() {
		loadBootstrapConfig();
	}

	function handleFormatChange() {
		loadBootstrapConfig();
	}

	function handleIncludeDefaultChange() {
		loadBootstrapConfig();
	}

	function downloadConfig() {
		const blob = new Blob([bootstrapConfig], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `bootstrap-${selectedTeam}.${format}`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	async function copyToClipboard() {
		try {
			await navigator.clipboard.writeText(bootstrapConfig);
			copySuccess = true;
			setTimeout(() => {
				copySuccess = false;
			}, 2000);
		} catch (err) {
			error = 'Failed to copy to clipboard';
		}
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a href="/dashboard" class="text-blue-600 hover:text-blue-800" aria-label="Back to dashboard">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Envoy Configuration</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
			<!-- Configuration Options -->
			<div class="lg:col-span-1">
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Configuration Options</h2>

					<div class="space-y-4">
						<!-- Team Selection -->
						<div>
							<label for="team" class="block text-sm font-medium text-gray-700 mb-2">
								Team
							</label>
							<select
								id="team"
								bind:value={selectedTeam}
								onchange={handleTeamChange}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								{#each teams as team}
									<option value={team}>{team}</option>
								{/each}
							</select>
						</div>

						<!-- Format Selection -->
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-2">Format</label>
							<div class="space-y-2">
								<label class="flex items-center">
									<input
										type="radio"
										bind:group={format}
										value="yaml"
										onchange={handleFormatChange}
										class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
									/>
									<span class="ml-2 text-sm text-gray-700">YAML</span>
								</label>
								<label class="flex items-center">
									<input
										type="radio"
										bind:group={format}
										value="json"
										onchange={handleFormatChange}
										class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
									/>
									<span class="ml-2 text-sm text-gray-700">JSON</span>
								</label>
							</div>
						</div>

						<!-- Include Defaults -->
						<div>
							<label class="flex items-center">
								<input
									type="checkbox"
									bind:checked={includeDefault}
									onchange={handleIncludeDefaultChange}
									class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
								/>
								<span class="ml-2 text-sm text-gray-700">Include default configurations</span>
							</label>
							<p class="mt-1 text-xs text-gray-500 ml-6">
								Apply global defaults and shared configurations
							</p>
						</div>

						<!-- Actions -->
						<div class="pt-4 space-y-2">
							<button
								onclick={downloadConfig}
								disabled={!bootstrapConfig || isLoading}
								class="w-full px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
							>
								Download Configuration
							</button>
							<button
								onclick={copyToClipboard}
								disabled={!bootstrapConfig || isLoading}
								class="w-full px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 disabled:opacity-50 disabled:cursor-not-allowed"
							>
								{copySuccess ? 'Copied!' : 'Copy to Clipboard'}
							</button>
						</div>
					</div>
				</div>

				<!-- Usage Instructions -->
				<div class="mt-6 bg-blue-50 border-l-4 border-blue-500 rounded-md p-4">
					<h3 class="text-sm font-medium text-blue-800 mb-2">How to Use</h3>
					<div class="text-sm text-blue-700 space-y-2">
						<p>1. Download or copy the bootstrap configuration</p>
						<p>2. Save it as <code class="bg-blue-100 px-1 rounded">bootstrap.yaml</code></p>
						<p>3. Start Envoy with the configuration:</p>
						<pre class="mt-2 p-2 bg-blue-100 rounded text-xs overflow-x-auto">
envoy -c bootstrap.yaml
						</pre>
					</div>
				</div>
			</div>

			<!-- Configuration Preview -->
			<div class="lg:col-span-2">
				<div class="bg-white rounded-lg shadow-md p-6">
					<div class="flex justify-between items-center mb-4">
						<h2 class="text-lg font-semibold text-gray-900">Configuration Preview</h2>
						<span class="text-sm text-gray-500">
							bootstrap-{selectedTeam}.{format}
						</span>
					</div>

					{#if isLoading}
						<div class="flex justify-center items-center py-12">
							<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
						</div>
					{:else if bootstrapConfig}
						<div class="relative">
							<pre class="bg-gray-900 text-gray-100 rounded-lg p-4 overflow-x-auto text-sm"><code
									class="hljs"
								>{@html highlightedCode}</code></pre>
						</div>
					{:else}
						<p class="text-center text-gray-500 py-12">
							Select a team to generate bootstrap configuration
						</p>
					{/if}
				</div>
			</div>
		</div>
	</main>
</div>
