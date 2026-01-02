<script lang="ts">
	import type { LearningSessionStatus } from '$lib/api/types';

	interface Props {
		status: string;
		size?: 'sm' | 'md';
	}

	let { status, size = 'sm' }: Props = $props();

	const statusConfig: Record<
		LearningSessionStatus,
		{ bg: string; text: string; dot: string; pulse: boolean; label: string }
	> = {
		pending: {
			bg: 'bg-gray-100',
			text: 'text-gray-800',
			dot: 'bg-gray-400',
			pulse: false,
			label: 'Pending'
		},
		active: {
			bg: 'bg-blue-100',
			text: 'text-blue-800',
			dot: 'bg-blue-500',
			pulse: true,
			label: 'Active'
		},
		completing: {
			bg: 'bg-yellow-100',
			text: 'text-yellow-800',
			dot: 'bg-yellow-500',
			pulse: true,
			label: 'Completing'
		},
		completed: {
			bg: 'bg-green-100',
			text: 'text-green-800',
			dot: 'bg-green-500',
			pulse: false,
			label: 'Completed'
		},
		cancelled: {
			bg: 'bg-gray-100',
			text: 'text-gray-600',
			dot: 'bg-gray-400',
			pulse: false,
			label: 'Cancelled'
		},
		failed: {
			bg: 'bg-red-100',
			text: 'text-red-800',
			dot: 'bg-red-500',
			pulse: false,
			label: 'Failed'
		}
	};

	let config = $derived(
		statusConfig[status as LearningSessionStatus] || statusConfig.pending
	);

	const sizeClasses = {
		sm: 'px-2.5 py-0.5 text-xs',
		md: 'px-3 py-1 text-sm'
	};
</script>

<span
	class="inline-flex items-center gap-1.5 rounded-full font-medium {config.bg} {config.text} {sizeClasses[size]}"
>
	<span class="relative flex h-2 w-2">
		{#if config.pulse}
			<span
				class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75 {config.dot}"
			></span>
		{/if}
		<span class="relative inline-flex rounded-full h-2 w-2 {config.dot}"></span>
	</span>
	{config.label}
</span>
