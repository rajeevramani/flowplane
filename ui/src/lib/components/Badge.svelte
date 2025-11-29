<script lang="ts">
	interface Props {
		variant?: 'blue' | 'purple' | 'green' | 'red' | 'yellow' | 'orange' | 'indigo' | 'gray';
		size?: 'sm' | 'md';
		children: any;
	}

	let { variant = 'blue', size = 'sm', children }: Props = $props();

	const variantClasses = {
		blue: 'bg-blue-100 text-blue-800',
		purple: 'bg-purple-100 text-purple-800',
		green: 'bg-green-100 text-green-800',
		red: 'bg-red-100 text-red-800',
		yellow: 'bg-yellow-100 text-yellow-800',
		orange: 'bg-orange-100 text-orange-800',
		indigo: 'bg-indigo-100 text-indigo-800',
		gray: 'bg-gray-100 text-gray-800'
	};

	const sizeClasses = {
		sm: 'px-2.5 py-0.5 text-xs',
		md: 'px-3 py-1 text-sm'
	};
</script>

<span
	class="inline-flex items-center rounded-full font-medium {variantClasses[variant]} {sizeClasses[size]}"
>
	{@render children()}
</span>
