import { Page, ConsoleMessage } from '@playwright/test';

export interface PageErrors {
	consoleErrors: string[];
	consoleWarnings: string[];
	jsErrors: string[];
	apiValidationFailures: string[];
}

/**
 * Set up error collectors on a page. Call BEFORE navigation.
 * Returns a PageErrors object that accumulates errors.
 */
export function collectPageErrors(page: Page): PageErrors {
	const errors: PageErrors = {
		consoleErrors: [],
		consoleWarnings: [],
		jsErrors: [],
		apiValidationFailures: []
	};

	page.on('console', (msg: ConsoleMessage) => {
		const text = msg.text();
		if (msg.type() === 'error') {
			errors.consoleErrors.push(text);
		}
		if (msg.type() === 'warning') {
			errors.consoleWarnings.push(text);
			// Specifically catch Zod validation failures from parseResponse()
			if (text.includes('API response validation failed')) {
				errors.apiValidationFailures.push(text);
			}
		}
	});

	page.on('pageerror', (error) => {
		errors.jsErrors.push(error.message);
	});

	return errors;
}

/**
 * Assert no critical errors occurred on the page.
 * - No JS errors (TypeError, ReferenceError, etc.)
 * - No API validation failures (Zod schema mismatches)
 */
export function assertNoPageErrors(errors: PageErrors) {
	// JS crashes are always failures
	if (errors.jsErrors.length > 0) {
		throw new Error(`Page had JavaScript errors:\n${errors.jsErrors.join('\n')}`);
	}
	// Zod validation failures indicate frontend-backend schema drift
	if (errors.apiValidationFailures.length > 0) {
		throw new Error(
			`API response validation failures (schema mismatch):\n${errors.apiValidationFailures.join('\n')}`
		);
	}
}

/**
 * Wait for page to be in a stable loaded state.
 * Waits for network to settle and spinners to disappear.
 */
export async function waitForPageLoad(page: Page, timeout = 15000) {
	// Wait for network idle (no requests for 500ms)
	await page.waitForLoadState('networkidle', { timeout });
	// Wait for any loading spinners to disappear
	await page
		.waitForSelector('[class*="animate-spin"]', { state: 'hidden', timeout: 5000 })
		.catch(() => {});
}
