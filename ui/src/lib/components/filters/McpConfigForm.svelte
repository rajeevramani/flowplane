<script lang="ts">
	import type { McpFilterConfig, McpTrafficMode } from '$lib/api/types';
	import { Info } from 'lucide-svelte';

	interface Props {
		config: McpFilterConfig;
		onConfigChange: (config: McpFilterConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Initialize state from config
	let trafficMode = $state<McpTrafficMode>(config.traffic_mode || 'pass_through');

	// Update parent when values change
	function updateParent() {
		onConfigChange({
			traffic_mode: trafficMode
		});
	}

	// Mode descriptions
	const modeDescriptions: Record<McpTrafficMode, { title: string; description: string }> = {
		pass_through: {
			title: 'Pass Through',
			description: 'Allow all traffic regardless of MCP compliance. Traffic is proxied normally without validation.'
		},
		reject_no_mcp: {
			title: 'Reject Non-MCP',
			description: 'Only allow valid MCP requests. Non-MCP traffic receives an error response.'
		}
	};
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Model Context Protocol (MCP) Filter</p>
				<p class="mt-1">
					Inspects and validates traffic for AI/LLM gateway compatibility.
					MCP uses JSON-RPC 2.0 over HTTP POST for tool calls and Server-Sent Events (SSE)
					for streaming responses.
				</p>
				<div class="mt-2">
					<p class="font-medium">Valid MCP requests:</p>
					<ul class="list-disc list-inside mt-1 space-y-0.5">
						<li>POST requests with JSON-RPC 2.0 messages (Content-Type: application/json)</li>
						<li>GET requests for SSE streams (Accept: text/event-stream)</li>
					</ul>
				</div>
			</div>
		</div>
	</div>

	<!-- Traffic Mode Selection -->
	<div>
		<label class="block text-sm font-medium text-gray-700 mb-3">
			Traffic Mode <span class="text-red-500">*</span>
		</label>

		<div class="space-y-3">
			{#each Object.entries(modeDescriptions) as [mode, info]}
				<label class="flex items-start gap-3 p-4 border rounded-lg cursor-pointer transition-colors
					{trafficMode === mode ? 'border-blue-500 bg-blue-50' : 'border-gray-200 hover:border-gray-300 hover:bg-gray-50'}">
					<input
						type="radio"
						name="trafficMode"
						value={mode}
						bind:group={trafficMode}
						onchange={updateParent}
						class="mt-0.5 h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
					/>
					<div class="flex-1">
						<span class="block text-sm font-medium text-gray-900">{info.title}</span>
						<span class="block text-xs text-gray-500 mt-1">{info.description}</span>
					</div>
				</label>
			{/each}
		</div>
	</div>

	<!-- Selected Mode Details -->
	<div class="p-4 bg-gray-50 rounded-lg border border-gray-200">
		<div class="flex items-center gap-2 mb-2">
			<span class="text-sm font-medium text-gray-700">Current Configuration:</span>
			<span class="px-2 py-0.5 text-xs rounded bg-blue-100 text-blue-700">
				{modeDescriptions[trafficMode].title}
			</span>
		</div>
		<p class="text-xs text-gray-600">
			{#if trafficMode === 'pass_through'}
				All HTTP traffic will be proxied to the upstream without MCP validation.
				This is useful for mixed traffic environments or during initial deployment.
			{:else}
				Only valid MCP-compliant requests will be forwarded. Invalid requests will receive
				an HTTP 400 Bad Request response. Use this mode to enforce strict MCP protocol compliance.
			{/if}
		</p>
	</div>
</div>
