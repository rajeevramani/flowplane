<script lang="ts">
	import type { PathMatchType } from "$lib/api/types";

	interface Props {
		pathType: PathMatchType;
		prefixRewrite: string;
		templateRewrite: string;
	}

	let {
		pathType,
		prefixRewrite = $bindable(),
		templateRewrite = $bindable(),
	}: Props = $props();

	// Clear invalid rewrite when path type changes
	$effect(() => {
		if (pathType !== "prefix" && pathType !== "exact") {
			prefixRewrite = "";
		}
		if (pathType !== "template") {
			templateRewrite = "";
		}
	});
</script>

{#key pathType}
	{#if pathType === "prefix" || pathType === "exact"}
		<div class="border-t border-gray-200 pt-4">
			<label class="block text-sm font-medium text-gray-700 mb-2"
				>Path Rewrite (Optional)</label
			>
			<div>
				<label for="prefix-rewrite" class="block text-sm text-gray-600 mb-1">
					Prefix Rewrite
					<span class="text-xs text-gray-400">(replaces matched prefix)</span>
				</label>
				<input
					id="prefix-rewrite"
					type="text"
					bind:value={prefixRewrite}
					placeholder="/new-prefix"
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>
				<p class="mt-1 text-xs text-gray-500">
					E.g., path "/api/v1" with prefix rewrite "/internal" turns
					"/api/v1/users" into "/internal/users"
				</p>
			</div>
		</div>
	{:else if pathType === "template"}
		<div class="border-t border-gray-200 pt-4">
			<label class="block text-sm font-medium text-gray-700 mb-2"
				>Path Rewrite (Optional)</label
			>
			<div>
				<label
					for="template-rewrite"
					class="block text-sm text-gray-600 mb-1"
				>
					Template Rewrite
					<span class="text-xs text-gray-400">(uses captured variables)</span>
				</label>
				<input
					id="template-rewrite"
					type="text"
					bind:value={templateRewrite}
					placeholder="/users/{'{'}id{'}'}/profile"
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>
				<p class="mt-1 text-xs text-gray-500">
					E.g., template "/users/{"{"}user_id{"}"}" with rewrite
					"/v2/users/{"{"}user_id{"}"}"
				</p>
			</div>
		</div>
	{:else if pathType === "regex"}
		<p class="text-sm text-gray-500 italic">
			Path rewrites are not available for regex path matching.
		</p>
	{/if}
{/key}
