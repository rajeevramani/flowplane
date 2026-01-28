<script lang="ts">
	import type { McpStatus, EnableMcpRequest, McpSchemaSource } from '$lib/api/types';
	import Button from '../Button.svelte';
	import LearnedSchemaBadge from './LearnedSchemaBadge.svelte';

	interface Props {
		show: boolean;
		status: McpStatus;
		routePath: string;
		routeMethod: string;
		onClose: () => void;
		onEnable: (request: EnableMcpRequest) => void;
		loading?: boolean;
	}

	let {
		show,
		status,
		routePath,
		routeMethod,
		onClose,
		onEnable,
		loading = false
	}: Props = $props();

	// Form state
	let toolName = $state('');
	let description = $state('');
	let schemaSource = $state<McpSchemaSource>('openapi');

	// Initialize form values when modal opens
	$effect(() => {
		if (show && status) {
			toolName = status.metadata?.operationId ? `api_${status.metadata.operationId}` : '';
			description = status.metadata?.summary || status.metadata?.description || '';
			schemaSource = (status.recommendedSource as McpSchemaSource) || 'openapi';
		}
	});

	function handleSubmit() {
		onEnable({
			toolName: toolName || undefined,
			description: description || undefined,
			schemaSource,
			summary: description || undefined,
			httpMethod: routeMethod || undefined
		});
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onClose();
		}
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			onClose();
		}
	}

	// Check if schema sources are available
	let hasOpenapi = $derived(status?.schemaSources?.openapi?.hasInputSchema);
	let hasLearned = $derived(status?.schemaSources?.learned?.available);
</script>

{#if show}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		aria-label="Enable MCP modal"
		tabindex="-1"
		onclick={handleBackdropClick}
		onkeydown={handleKeydown}
	>
		<div
			class="bg-white rounded-lg shadow-xl max-w-lg w-full mx-4 max-h-[90vh] overflow-y-auto"
			role="presentation"
			onclick={(e) => e.stopPropagation()}
			onkeydown={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div class="px-6 py-4 border-b border-gray-200">
				<h2 class="text-lg font-semibold text-gray-900">Enable MCP for Route</h2>
				<p class="mt-1 text-sm text-gray-500">
					<span class="font-medium">{routeMethod}</span>
					<span class="font-mono ml-1">{routePath}</span>
				</p>
			</div>

			<!-- Body -->
			<div class="px-6 py-4 space-y-4">
				<!-- Learned Schema Badge -->
				{#if status?.schemaSources?.learned?.available}
					<div class="p-3 bg-purple-50 rounded-lg border border-purple-100">
						<LearnedSchemaBadge schema={status.schemaSources.learned} />
					</div>
				{/if}

				<!-- Schema Source Selection -->
				<fieldset class="space-y-2">
					<legend class="block text-sm font-medium text-gray-700">Schema Source</legend>
					<div class="flex flex-wrap gap-4">
						{#if hasOpenapi}
							<label class="flex items-center gap-2 cursor-pointer">
								<input
									type="radio"
									name="schemaSource"
									value="openapi"
									bind:group={schemaSource}
									class="h-4 w-4 text-blue-600 focus:ring-blue-500"
								/>
								<span class="text-sm text-gray-700">OpenAPI</span>
								{#if status?.recommendedSource === 'openapi'}
									<span class="text-xs text-gray-500">(recommended)</span>
								{/if}
							</label>
						{/if}
						{#if hasLearned}
							<label class="flex items-center gap-2 cursor-pointer">
								<input
									type="radio"
									name="schemaSource"
									value="learned"
									bind:group={schemaSource}
									class="h-4 w-4 text-blue-600 focus:ring-blue-500"
								/>
								<span class="text-sm text-gray-700">Learned</span>
								{#if status?.recommendedSource === 'learned'}
									<span class="text-xs text-gray-500">(recommended)</span>
								{/if}
							</label>
						{/if}
						<label class="flex items-center gap-2 cursor-pointer">
							<input
								type="radio"
								name="schemaSource"
								value="manual"
								bind:group={schemaSource}
								class="h-4 w-4 text-blue-600 focus:ring-blue-500"
							/>
							<span class="text-sm text-gray-700">Manual</span>
						</label>
					</div>
				</fieldset>

				<!-- Tool Name -->
				<div class="space-y-2">
					<label for="toolName" class="block text-sm font-medium text-gray-700">Tool Name</label>
					<input
						id="toolName"
						type="text"
						bind:value={toolName}
						placeholder="api_getUserById"
						class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
					<p class="text-xs text-gray-500">Used by AI assistants to call this API</p>
				</div>

				<!-- Description -->
				<div class="space-y-2">
					<label for="description" class="block text-sm font-medium text-gray-700">Description</label>
					<input
						id="description"
						type="text"
						bind:value={description}
						placeholder="Retrieves user profile by ID"
						class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
					/>
				</div>
			</div>

			<!-- Footer -->
			<div class="px-6 py-4 border-t border-gray-200 flex justify-end gap-3">
				<Button variant="ghost" onclick={onClose} disabled={loading}>
					Cancel
				</Button>
				<Button variant="primary" onclick={handleSubmit} disabled={loading}>
					{loading ? 'Enabling...' : 'Enable MCP'}
				</Button>
			</div>
		</div>
	</div>
{/if}
