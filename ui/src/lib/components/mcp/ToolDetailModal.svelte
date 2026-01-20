<script lang="ts">
	import { X, Code, FileJson, AlertCircle, Edit, Sparkles, Database, Server } from 'lucide-svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type { McpTool } from '$lib/api/types';

	interface Props {
		show: boolean;
		tool: McpTool | null;
		onClose: () => void;
		onEdit: (tool: McpTool) => void;
		onApplyLearned?: () => void;
		hasLearnedSchemaAvailable?: boolean;
	}

	let { show, tool, onClose, onEdit, onApplyLearned, hasLearnedSchemaAvailable = false }: Props = $props();

	// Check if the tool has incomplete information
	let hasIncompleteInfo = $derived(
		tool
			? !tool.description ||
				!tool.inputSchema ||
				Object.keys(tool.inputSchema).length === 0 ||
				!tool.outputSchema
			: false
	);

	// Format the route display - show just the path if method is missing
	let routeDisplay = $derived(
		tool
			? tool.httpMethod
				? `${tool.httpMethod} ${tool.httpPath || '/'}`
				: tool.httpPath || '/'
			: ''
	);

	// Category badge variant mapping
	type BadgeVariant = 'blue' | 'purple' | 'green' | 'red' | 'yellow' | 'orange' | 'indigo' | 'gray';
	let categoryVariant: BadgeVariant = $derived(
		tool?.category === 'control_plane'
			? 'purple'
			: tool?.category === 'gateway_api'
				? 'blue'
				: 'gray'
	);

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onClose();
		}
	}

	function handleEdit() {
		if (tool) {
			onEdit(tool);
		}
	}
</script>

{#if show && tool}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4"
		role="dialog"
		aria-modal="true"
		onclick={handleBackdropClick}
	>
		<div
			class="bg-white rounded-lg shadow-xl max-w-3xl w-full max-h-[90vh] overflow-y-auto"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div
				class="sticky top-0 bg-white border-b border-gray-200 px-6 py-4 flex items-center justify-between z-10"
			>
				<div class="flex items-center gap-3">
					<Code class="w-6 h-6 text-blue-600" />
					<h2 class="text-xl font-semibold text-gray-900">{tool.name}</h2>
				</div>
				<button
					type="button"
					onclick={onClose}
					class="text-gray-500 hover:text-gray-700 transition-colors"
					aria-label="Close modal"
				>
					<X class="w-6 h-6" />
				</button>
			</div>

			<!-- Content -->
			<div class="px-6 py-6 space-y-6">
				<!-- Incomplete Information Warning -->
				{#if hasIncompleteInfo}
					<div
						class="bg-amber-50 border border-amber-200 rounded-lg p-4 flex items-start gap-3"
					>
						<AlertCircle class="w-5 h-5 text-amber-600 flex-shrink-0 mt-0.5" />
						<div class="flex-1">
							<h3 class="text-sm font-medium text-amber-900 mb-1">
								Incomplete Information
							</h3>
							<p class="text-sm text-amber-700 mb-3">
								This tool is missing some information. Click the button below to add missing
								details.
							</p>
							<Button variant="primary" size="sm" onclick={handleEdit}>
								<Edit class="w-4 h-4 mr-2" />
								Add Missing Information
							</Button>
						</div>
					</div>
				{/if}

				<!-- Basic Information -->
				<div>
					<h3 class="text-sm font-medium text-gray-500 mb-2">Route</h3>
					<code
						class="block bg-gray-100 px-3 py-2 rounded text-sm text-gray-900 font-mono"
					>
						{routeDisplay}
					</code>
				</div>

				<div>
					<h3 class="text-sm font-medium text-gray-500 mb-2">Category</h3>
					<Badge variant={categoryVariant}>
						{tool.category === 'control_plane'
							? 'Control Plane'
							: tool.category === 'gateway_api'
								? 'Gateway API'
								: tool.category}
					</Badge>
				</div>

				<!-- Execution Details (Cluster and Port) -->
				{#if tool.clusterName || tool.listenerPort}
					<div>
						<h3 class="text-sm font-medium text-gray-500 mb-2">Execution Details</h3>
						<div class="bg-gray-50 rounded-lg p-3 space-y-2">
							{#if tool.clusterName}
								<div class="flex items-center gap-2">
									<Database class="w-4 h-4 text-gray-500" />
									<span class="text-sm text-gray-600 font-medium w-20">Cluster:</span>
									<code class="text-sm text-gray-900 bg-white px-2 py-1 rounded border flex-1 truncate">
										{tool.clusterName}
									</code>
								</div>
							{/if}
							{#if tool.listenerPort}
								<div class="flex items-center gap-2">
									<Server class="w-4 h-4 text-gray-500" />
									<span class="text-sm text-gray-600 font-medium w-20">Port:</span>
									<code class="text-sm text-gray-900 bg-white px-2 py-1 rounded border">
										{tool.listenerPort}
									</code>
								</div>
							{/if}
						</div>
					</div>
				{/if}

				<div>
					<h3 class="text-sm font-medium text-gray-500 mb-2">Description</h3>
					{#if tool.description}
						<p class="text-gray-700">{tool.description}</p>
					{:else}
						<p class="text-gray-400 italic">No description provided</p>
					{/if}
				</div>

				<!-- Request Schema -->
				<div>
					<div class="flex items-center gap-2 mb-2">
						<FileJson class="w-4 h-4 text-gray-500" />
						<h3 class="text-sm font-medium text-gray-500">Request Schema</h3>
					</div>
					{#if tool.inputSchema && Object.keys(tool.inputSchema).length > 0}
						<pre
							class="bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm font-mono">{JSON.stringify(tool.inputSchema, null, 2)}</pre>
					{:else}
						<div class="bg-gray-100 p-4 rounded-lg">
							<p class="text-gray-400 italic text-sm">No request schema defined</p>
						</div>
					{/if}
				</div>

				<!-- Response Schema -->
				<div>
					<div class="flex items-center gap-2 mb-2">
						<FileJson class="w-4 h-4 text-gray-500" />
						<h3 class="text-sm font-medium text-gray-500">Response Schema</h3>
					</div>
					{#if tool.outputSchema && Object.keys(tool.outputSchema).length > 0}
						<pre
							class="bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm font-mono">{JSON.stringify(tool.outputSchema, null, 2)}</pre>
					{:else}
						<div class="bg-gray-100 p-4 rounded-lg">
							<p class="text-gray-400 italic text-sm">No response schema defined</p>
						</div>
					{/if}
				</div>
			</div>

			<!-- Footer -->
			<div
				class="sticky bottom-0 bg-gray-50 border-t border-gray-200 px-6 py-4 flex justify-between gap-3 z-10"
			>
				<div>
					{#if hasLearnedSchemaAvailable && onApplyLearned}
						<Button variant="secondary" onclick={onApplyLearned}>
							<Sparkles class="w-4 h-4 mr-2" />
							Apply Learned Schema
						</Button>
					{/if}
				</div>
				<div class="flex gap-3">
					<Button variant="primary" onclick={handleEdit}>
						<Edit class="w-4 h-4 mr-2" />
						Edit Tool
					</Button>
					<Button variant="ghost" onclick={onClose}> Close </Button>
				</div>
			</div>
		</div>
	</div>
{/if}
