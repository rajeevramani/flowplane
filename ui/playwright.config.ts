import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const authFile = join(__dirname, 'test-results', '.auth', 'user.json');

export default defineConfig({
	testDir: './e2e',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 2 : 0,
	workers: process.env.CI ? 1 : undefined,
	reporter: [['list'], ['html', { open: 'never' }]],
	use: {
		baseURL: process.env.BASE_URL || 'http://localhost:8080',
		trace: 'on-first-retry'
	},
	projects: [
		{
			name: 'setup',
			testMatch: /auth\.setup\.ts/
		},
		{
			name: 'chromium',
			use: {
				...devices['Desktop Chrome'],
				storageState: authFile
			},
			dependencies: ['setup']
		}
	]
});
