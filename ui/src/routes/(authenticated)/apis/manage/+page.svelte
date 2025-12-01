<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	onMount(() => {
		// Redirect to new create page
		goto('/route-configs/create', { replaceState: true });
	});
</script>

<div class="min-h-screen bg-gray-100 flex items-center justify-center">
	<div class="flex flex-col items-center gap-3">
		<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
		<span class="text-sm text-gray-600">Redirecting...</span>
	</div>
</div>
