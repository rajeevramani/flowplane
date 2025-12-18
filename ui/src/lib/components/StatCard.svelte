<script lang="ts">
	interface Props {
		title: string;
		value: number | string;
		icon?: 'api' | 'cluster' | 'route' | 'listener';
		href?: string;
		colorClass?: 'blue' | 'purple' | 'green' | 'orange';
	}

	let { title, value, icon, href, colorClass = 'blue' }: Props = $props();

	const iconPaths = {
		api: 'M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10',
		cluster: 'M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01',
		route: 'M9 20l-5.447-2.724A1 1 0 013 16.382V5.618a1 1 0 011.447-.894L9 7m0 13l6-3m-6 3V7m6 10l4.553 2.276A1 1 0 0021 18.382V7.618a1 1 0 00-.553-.894L15 4m0 13V4m0 0L9 7',
		listener: 'M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9'
	};

	const colorClasses = {
		blue: 'bg-blue-100 text-blue-600',
		purple: 'bg-purple-100 text-purple-600',
		green: 'bg-green-100 text-green-600',
		orange: 'bg-orange-100 text-orange-600'
	};

	const baseClasses = 'block p-6 bg-white rounded-lg border border-gray-200 transition-all';
	const hoverClasses = href
		? `hover:border-${colorClass}-300 hover:shadow-md cursor-pointer`
		: '';
</script>

{#if href}
	<a {href} class="{baseClasses} {hoverClasses}">
		<div class="flex items-start justify-between">
			<div class="flex-1">
				<h3 class="text-sm font-medium text-gray-600 mb-1">{title}</h3>
				<p class="text-3xl font-bold text-gray-900">{value}</p>
			</div>
			{#if icon}
				<div class="flex-shrink-0 p-3 rounded-lg {colorClasses[colorClass] || colorClasses.blue}">
					<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d={iconPaths[icon]}
						/>
					</svg>
				</div>
			{/if}
		</div>
	</a>
{:else}
	<div class={baseClasses}>
		<div class="flex items-start justify-between">
			<div class="flex-1">
				<h3 class="text-sm font-medium text-gray-600 mb-1">{title}</h3>
				<p class="text-3xl font-bold text-gray-900">{value}</p>
			</div>
			{#if icon}
				<div class="flex-shrink-0 p-3 rounded-lg {colorClasses[colorClass] || colorClasses.blue}">
					<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d={iconPaths[icon]}
						/>
					</svg>
				</div>
			{/if}
		</div>
	</div>
{/if}
