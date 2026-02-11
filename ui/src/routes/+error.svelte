<script lang="ts">
	import { page } from '$app/state';
</script>

<div class="min-h-screen bg-gray-50 flex items-center justify-center px-4">
	<div class="max-w-md w-full text-center">
		<div class="bg-white rounded-lg shadow-md p-8">
			<div class="text-6xl font-bold text-gray-300 mb-4">
				{page.status}
			</div>
			<h1 class="text-xl font-semibold text-gray-900 mb-2">
				{#if page.status === 404}
					Page Not Found
				{:else if page.status === 500}
					Internal Error
				{:else}
					Something went wrong
				{/if}
			</h1>
			<p class="text-gray-600 mb-6">
				{#if page.error?.message}
					{page.error.message}
				{:else}
					An unexpected error occurred.
				{/if}
			</p>
			<div class="flex justify-center gap-4">
				<a
					href="/dashboard"
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					Go to Dashboard
				</a>
				<button
					onclick={() => window.location.reload()}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
				>
					Retry
				</button>
			</div>
		</div>
	</div>
</div>
