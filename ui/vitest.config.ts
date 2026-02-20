import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { playwright } from '@vitest/browser-playwright';
import path from 'path';

export default defineConfig({
	plugins: [svelte()],
	resolve: {
		alias: {
			$lib: path.resolve('./src/lib'),
			'$app/navigation': path.resolve('./src/lib/test-mocks/app-navigation.ts'),
			'$app/environment': path.resolve('./src/lib/test-mocks/app-environment.ts'),
			'$app/state': path.resolve('./src/lib/test-mocks/app-state.ts'),
			'$env/dynamic/public': path.resolve('./src/lib/test-mocks/env-dynamic-public.ts')
		}
	},
	optimizeDeps: {
		include: ['lucide-svelte', 'svelte/store']
	},
	test: {
		include: ['src/**/*.svelte.{test,spec}.ts'],
		browser: {
			enabled: true,
			provider: playwright(),
			instances: [{ browser: 'chromium' }]
		},
		setupFiles: ['vitest-browser-svelte']
	}
});
