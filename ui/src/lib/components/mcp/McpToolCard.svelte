<script lang="ts">
	import type { McpTool } from '$lib/api/types';
	import Badge from '../Badge.svelte';
	import SchemaPreview from './SchemaPreview.svelte';
	import { FileText, Database, Terminal, Settings, Code, AlertCircle, Eye, Sparkles } from 'lucide-svelte';

	interface Props {
		tool: McpTool;
		onToggle: () => void;
		onView: () => void;
		hasLearnedSchemaAvailable?: boolean;
	}

	let { tool, onToggle, onView, hasLearnedSchemaAvailable = false }: Props = $props();

	let expanded = $state(false);

	const methodColors: Record<string, 'blue' | 'green' | 'yellow' | 'orange' | 'red' | 'gray'> = {
		GET: 'blue',
		POST: 'green',
		PUT: 'yellow',
		PATCH: 'orange',
		DELETE: 'red'
	};

	// Category to icon mapping
	const categoryIcons = {
		'File Operations': FileText,
		'Database': Database,
		'Execution': Terminal,
		'Configuration': Settings,
		'gateway_api': Code,
		'control_plane': Settings
	};

	let methodColor = $derived(methodColors[tool.httpMethod || ''] || 'gray');
	let confidencePercent = $derived(tool.confidence !== null ? Math.round(tool.confidence * 100) : null);

	// Check if tool has incomplete information
	let isIncomplete = $derived(
		!tool.description ||
		!tool.inputSchema ||
		(typeof tool.inputSchema === 'object' && !Object.keys(tool.inputSchema).length) ||
		!tool.outputSchema ||
		(typeof tool.outputSchema === 'object' && !Object.keys(tool.outputSchema).length)
	);

	// Get the icon component for the tool's category
	let IconComponent = $derived(
		(tool.category && categoryIcons[tool.category as keyof typeof categoryIcons]) || Code
	);
</script>

<div class="bg-white rounded-lg shadow-sm border p-6 hover:shadow-lg transition-shadow relative {isIncomplete ? 'border-amber-300' : 'border-gray-200'}">
	<!-- Incomplete warning icon in top-right -->
	{#if isIncomplete}
		<div class="absolute top-3 right-3">
			<AlertCircle class="w-5 h-5 text-amber-600" />
		</div>
	{:else if hasLearnedSchemaAvailable}
		<div class="absolute top-3 right-3" title="Learned schema available">
			<Sparkles class="w-5 h-5 text-purple-600" />
		</div>
	{/if}

	<!-- Header with icon, name, and category -->
	<div class="flex items-start justify-between mb-4">
		<div class="flex items-center gap-3">
			<div class="p-2 bg-blue-100 rounded-lg">
				<svelte:component this={IconComponent} class="w-5 h-5 text-blue-600" />
			</div>
			<div>
				<h3 class="text-gray-900 font-medium">{tool.name}</h3>
				<span class="text-xs text-gray-500">{tool.category || 'Uncategorized'}</span>
			</div>
		</div>
	</div>

	<!-- Description -->
	<p class="text-sm text-gray-600 mb-4 min-h-[40px]">
		{#if tool.description}
			{tool.description}
		{:else}
			<span class="italic text-gray-400">No description</span>
		{/if}
	</p>

	<!-- HTTP Method and Path -->
	<div class="space-y-3">
		{#if tool.httpMethod && tool.httpPath}
			<div class="flex items-center gap-2">
				<Badge variant={methodColor} size="sm">{tool.httpMethod}</Badge>
				<code class="text-xs text-gray-700 bg-gray-100 px-2 py-1 rounded flex-1 truncate">
					{tool.httpPath}
				</code>
			</div>
		{/if}

		<!-- Source badges -->
		<div class="flex flex-wrap gap-2">
			<Badge variant="gray" size="sm">{tool.sourceType}</Badge>
			{#if tool.schemaSource}
				<Badge variant="purple" size="sm">{tool.schemaSource}</Badge>
			{/if}
			{#if confidencePercent !== null}
				<Badge variant="indigo" size="sm">{confidencePercent}%</Badge>
			{/if}
		</div>

		<!-- Toggle and View buttons -->
		<div class="flex items-center gap-2 pt-2">
			<button
				onclick={onToggle}
				class="flex-1 px-3 py-2 rounded transition-colors text-sm font-medium {tool.enabled
					? 'bg-green-100 text-green-700 hover:bg-green-200'
					: 'bg-gray-100 text-gray-700 hover:bg-gray-200'}"
			>
				{tool.enabled ? 'Enabled' : 'Disabled'}
			</button>
			<button
				onclick={onView}
				class="px-3 py-2 bg-blue-600 text-white rounded hover:bg-blue-700 transition-colors flex items-center gap-1 text-sm font-medium"
			>
				<Eye class="w-4 h-4" />
				View
			</button>
		</div>

		<!-- Expandable schema preview -->
		<button
			onclick={() => (expanded = !expanded)}
			class="w-full text-sm text-blue-600 hover:text-blue-800 font-medium focus:outline-none text-left"
		>
			{expanded ? 'Hide Schema' : 'Show Schema'}
		</button>

		{#if expanded}
			<div class="mt-2">
				<SchemaPreview schema={tool.inputSchema} title="Input Schema" />
			</div>
		{/if}
	</div>
</div>
