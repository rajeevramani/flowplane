import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const adminAuthFile = join(__dirname, 'test-results', '.auth', 'admin.json');
const orgadminAuthFile = join(__dirname, 'test-results', '.auth', 'orgadmin.json');

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
			name: 'setup-orgadmin',
			testMatch: /orgadmin\.setup\.ts/,
			dependencies: ['setup']
		},
		{
			name: 'admin',
			use: {
				...devices['Desktop Chrome'],
				storageState: adminAuthFile
			},
			dependencies: ['setup'],
			testMatch: /^(?!.*orgadmin).*\.test\.ts$/
		},
		{
			name: 'orgadmin',
			use: {
				...devices['Desktop Chrome'],
				storageState: orgadminAuthFile
			},
			dependencies: ['setup-orgadmin'],
			testMatch: /orgadmin.*\.test\.ts$/
		}
	],
	webServer: {
		command: './target/debug/flowplane',
		cwd: join(__dirname, '..'),
		url: 'http://localhost:8080/api/v1/bootstrap/status',
		timeout: 30_000,
		reuseExistingServer: !process.env.CI,
		env: {
			FLOWPLANE_DATABASE_URL: 'postgresql://flowplane:flowplane@localhost:5432/flowplane',
			FLOWPLANE_DATABASE_AUTO_MIGRATE: 'true',
			FLOWPLANE_COOKIE_SECURE: 'false',
			FLOWPLANE_SKIP_SETUP_TOKEN: 'true',
			FLOWPLANE_API_PORT: '8080',
			FLOWPLANE_API_BIND_ADDRESS: '127.0.0.1',
			FLOWPLANE_UI_ORIGIN: 'http://localhost:8080',
			RUST_LOG: 'warn,flowplane=info',
		},
		stdout: 'pipe',
		stderr: 'pipe',
	}
});
