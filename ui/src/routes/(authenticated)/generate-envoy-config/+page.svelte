<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';
	import hljs from 'highlight.js/lib/core';
	import yaml from 'highlight.js/lib/languages/yaml';
	import json from 'highlight.js/lib/languages/json';
	import 'highlight.js/styles/github-dark.css';

	// Register languages
	hljs.registerLanguage('yaml', yaml);
	hljs.registerLanguage('json', json);

	let currentTeam = $state('');
	let format = $state<'yaml' | 'json'>('yaml');
	let bootstrapConfig = $state('');
	let highlightedCode = $state('');
	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let copySuccess = $state(false);
	let unsubscribe: Unsubscriber;

	onMount(async () => {
		// Subscribe to team changes from shared store
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				await loadBootstrapConfig();
			}
		});
	});

	onDestroy(() => {
		if (unsubscribe) {
			unsubscribe();
		}
	});

	async function loadBootstrapConfig() {
		if (!currentTeam) return;

		isLoading = true;
		error = null;

		try {
			bootstrapConfig = await apiClient.getBootstrapConfig({
				team: currentTeam,
				format
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

	function handleFormatChange() {
		loadBootstrapConfig();
	}

	function downloadConfig() {
		const blob = new Blob([bootstrapConfig], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `bootstrap-${currentTeam}.${format}`;
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
				<!-- Team Display (controlled by navbar) -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">
						Team
					</label>
					<div class="px-3 py-2 bg-gray-100 border border-gray-300 rounded-md text-sm text-gray-700">
						{currentTeam || 'No team selected'}
					</div>
					<p class="mt-1 text-xs text-gray-500">
						Use the navbar team selector to change teams
					</p>
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
					bootstrap-{currentTeam}.{format}
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
