<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	// Redirect to the new APIs page
	onMount(() => {
		goto('/apis', { replaceState: true });
	});
</script>

<div class="flex items-center justify-center min-h-[50vh]">
	<div class="text-center">
		<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto"></div>
		<p class="mt-4 text-gray-600">Redirecting to APIs...</p>
	</div>
</div>
