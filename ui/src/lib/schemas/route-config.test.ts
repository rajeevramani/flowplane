/**
 * Example usage and validation tests for route config schemas.
 * This file demonstrates how to use the schemas in your forms.
 */

import {
	RouteNameSchema,
	PathSchema,
	DomainSchema,
	RouteFormSchema,
	VirtualHostFormSchema,
	RouteConfigFormSchema,
	McpConfigSchema,
	type RouteFormData,
	type VirtualHostFormData,
	type RouteConfigFormData
} from './route-config';

// ============================================================================
// Example 1: Validating a route name
// ============================================================================

function validateRouteName(name: string): { success: boolean; error?: string } {
	const result = RouteNameSchema.safeParse(name);
	if (result.success) {
		return { success: true };
	}
	return { success: false, error: result.error.errors[0]?.message };
}

// Valid examples
console.assert(validateRouteName('api-route').success === true);
console.assert(validateRouteName('route1').success === true);
console.assert(validateRouteName('my-api-v2').success === true);

// Invalid examples
console.assert(validateRouteName('').success === false);
console.assert(validateRouteName('Route-Name').success === false); // uppercase not allowed
console.assert(validateRouteName('-route').success === false); // cannot start with dash
console.assert(validateRouteName('route-').success === false); // cannot end with dash

// ============================================================================
// Example 2: Validating a path
// ============================================================================

function validatePath(path: string): { success: boolean; error?: string } {
	const result = PathSchema.safeParse(path);
	if (result.success) {
		return { success: true };
	}
	return { success: false, error: result.error.errors[0]?.message };
}

// Valid examples
console.assert(validatePath('/api/users').success === true);
console.assert(validatePath('/').success === true);
console.assert(validatePath('/v1/{id}').success === true);

// Invalid examples
console.assert(validatePath('').success === false);
console.assert(validatePath('api/users').success === false); // must start with /

// ============================================================================
// Example 3: Validating a complete route
// ============================================================================

const validRoute: RouteFormData = {
	name: 'get-users',
	method: 'GET',
	path: '/api/users',
	pathType: 'prefix',
	cluster: 'backend-cluster',
	timeout: 30
};

const validationResult = RouteFormSchema.safeParse(validRoute);
console.assert(validationResult.success === true);

// Route with retry policy
const routeWithRetry: RouteFormData = {
	name: 'post-orders',
	method: 'POST',
	path: '/api/orders',
	pathType: 'exact',
	cluster: 'order-service',
	timeout: 60,
	retryEnabled: true,
	maxRetries: 3,
	retryOn: ['5xx', 'reset', 'connect-failure'],
	perTryTimeout: 10,
	backoffBaseMs: 100,
	backoffMaxMs: 1000
};

const retryValidation = RouteFormSchema.safeParse(routeWithRetry);
console.assert(retryValidation.success === true);

// Invalid route - retry enabled but missing required fields
const invalidRetryRoute = {
	name: 'invalid-route',
	method: 'GET',
	path: '/api/test',
	pathType: 'prefix',
	cluster: 'test-cluster',
	retryEnabled: true
	// Missing: maxRetries, retryOn, perTryTimeout
};

const invalidRetryValidation = RouteFormSchema.safeParse(invalidRetryRoute);
console.assert(invalidRetryValidation.success === false);

// ============================================================================
// Example 4: Validating a virtual host
// ============================================================================

const validVirtualHost: VirtualHostFormData = {
	name: 'api-vhost',
	domains: ['api.example.com', 'api.example.net'],
	routes: [validRoute, routeWithRetry]
};

const vhostValidation = VirtualHostFormSchema.safeParse(validVirtualHost);
console.assert(vhostValidation.success === true);

// Invalid - duplicate domains
const invalidVHost = {
	name: 'test-vhost',
	domains: ['api.example.com', 'api.example.com'], // duplicate
	routes: [validRoute]
};

const invalidVHostValidation = VirtualHostFormSchema.safeParse(invalidVHost);
console.assert(invalidVHostValidation.success === false);

// ============================================================================
// Example 5: Validating a complete route configuration
// ============================================================================

const validRouteConfig: RouteConfigFormData = {
	name: 'production-config',
	team: 'platform-team',
	virtualHosts: [validVirtualHost]
};

const configValidation = RouteConfigFormSchema.safeParse(validRouteConfig);
console.assert(configValidation.success === true);

// ============================================================================
// Example 6: Validating MCP configuration
// ============================================================================

// MCP enabled with required fields
const validMcpConfig = {
	enabled: true,
	toolName: 'get_user_profile',
	description: 'Retrieve user profile information',
	schemaSource: 'openapi' as const
};

const mcpValidation = McpConfigSchema.safeParse(validMcpConfig);
console.assert(mcpValidation.success === true);

// MCP enabled but missing tool name - should fail
const invalidMcpConfig = {
	enabled: true
	// Missing: toolName
};

const invalidMcpValidation = McpConfigSchema.safeParse(invalidMcpConfig);
console.assert(invalidMcpValidation.success === false);

// MCP disabled - tool name not required
const disabledMcpConfig = {
	enabled: false
};

const disabledMcpValidation = McpConfigSchema.safeParse(disabledMcpConfig);
console.assert(disabledMcpValidation.success === true);

// ============================================================================
// Example 7: Using schemas in Svelte forms
// ============================================================================

/**
 * Example of how to use these schemas in a Svelte component:
 *
 * ```typescript
 * <script lang="ts">
 *   import { RouteFormSchema, type RouteFormData } from '$lib/schemas/route-config';
 *   import { superForm } from 'sveltekit-superforms/client';
 *   import { zod } from 'sveltekit-superforms/adapters';
 *
 *   // Initialize form with schema
 *   const { form, errors, enhance, submitting } = superForm<RouteFormData>(
 *     {
 *       name: '',
 *       method: 'GET',
 *       path: '/',
 *       pathType: 'prefix',
 *       cluster: '',
 *       timeout: 30
 *     },
 *     {
 *       validators: zod(RouteFormSchema),
 *       onUpdate: async ({ form }) => {
 *         // form.valid is true if validation passes
 *         if (form.valid) {
 *           // Submit to API
 *           await apiClient.createRoute(form.data);
 *         }
 *       }
 *     }
 *   );
 * </script>
 *
 * <form method="POST" use:enhance>
 *   <input
 *     type="text"
 *     bind:value={$form.name}
 *     placeholder="Route name"
 *   />
 *   {#if $errors.name}
 *     <span class="error">{$errors.name}</span>
 *   {/if}
 *
 *   <input
 *     type="text"
 *     bind:value={$form.path}
 *     placeholder="/api/path"
 *   />
 *   {#if $errors.path}
 *     <span class="error">{$errors.path}</span>
 *   {/if}
 *
 *   <button type="submit" disabled={$submitting}>
 *     Create Route
 *   </button>
 * </form>
 * ```
 */

// ============================================================================
// Example 8: Parsing and transforming form data
// ============================================================================

/**
 * Example function to validate and transform route form data
 */
function processRouteFormData(rawData: unknown): RouteFormData | { error: string } {
	const result = RouteFormSchema.safeParse(rawData);

	if (result.success) {
		// Data is validated and transformed (e.g., strings trimmed)
		return result.data;
	}

	// Return first validation error
	return {
		error: result.error.errors[0]?.message || 'Validation failed'
	};
}

// ============================================================================
// Example 9: Partial validation for edit forms
// ============================================================================

/**
 * When editing, you might want to validate only the fields that changed.
 * You can use Zod's .partial() method:
 */
const PartialRouteSchema = RouteFormSchema.partial();

const partialUpdate = {
	timeout: 60 // Only updating timeout
};

const partialValidation = PartialRouteSchema.safeParse(partialUpdate);
console.assert(partialValidation.success === true);

// ============================================================================
// Example 10: Custom error messages
// ============================================================================

/**
 * Extract user-friendly error messages from Zod validation errors
 */
function getFormErrors(error: unknown): Record<string, string> {
	if (error instanceof Error) {
		try {
			const zodError = JSON.parse(error.message);
			const errors: Record<string, string> = {};

			for (const err of zodError) {
				const field = err.path.join('.');
				errors[field] = err.message;
			}

			return errors;
		} catch {
			return { _global: error.message };
		}
	}

	return { _global: 'An unknown error occurred' };
}

export {
	validateRouteName,
	validatePath,
	processRouteFormData,
	getFormErrors
};
