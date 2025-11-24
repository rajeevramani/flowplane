<script lang="ts">
	interface Tab {
		id: string;
		label: string;
		count: number;
	}

	interface Props {
		tabs: Tab[];
		activeTab: string;
		onTabChange: (tabId: string) => void;
	}

	let { tabs, activeTab, onTabChange }: Props = $props();
</script>

<div class="border-b border-gray-200">
	<nav class="-mb-px flex gap-4" aria-label="Tabs">
		{#each tabs as tab}
			<button
				onclick={() => onTabChange(tab.id)}
				class="whitespace-nowrap py-3 px-1 border-b-2 font-medium text-sm transition-colors {activeTab === tab.id
					? 'border-blue-500 text-blue-600'
					: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
				aria-current={activeTab === tab.id ? 'page' : undefined}
			>
				{tab.label}
				<span
					class="ml-2 py-0.5 px-2 rounded-full text-xs {activeTab === tab.id
						? 'bg-blue-100 text-blue-600'
						: 'bg-gray-100 text-gray-600'}"
				>
					{tab.count}
				</span>
			</button>
		{/each}
	</nav>
</div>
