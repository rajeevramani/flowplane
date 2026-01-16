/**
 * Zod schemas for route configuration forms.
 * These schemas validate route configs, virtual hosts, and routes with comprehensive error messages.
 */
import { z } from 'zod';

/**
 * Reusable schema for route/resource names.
 * Validates lowercase alphanumeric with dashes, starting and ending with alphanumeric.
 */
export const RouteNameSchema = z
	.string()
	.min(1, 'Name is required')
	.transform((val) => val.trim())
	.refine(
		(val) => {
			// Pattern: lowercase, alphanumeric with dashes, must start and end with alphanumeric
			const pattern = /^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/;
			return pattern.test(val);
		},
		{
			message:
				'Name must be lowercase, start and end with alphanumeric characters, and may contain hyphens'
		}
	);

/**
 * Schema for route path patterns.
 * Validates that paths start with / and are non-empty.
 */
export const PathSchema = z
	.string()
	.min(1, 'Path is required')
	.transform((val) => val.trim())
	.refine((val) => val.startsWith('/'), {
		message: 'Path must start with /'
	});

/**
 * Schema for domain names in virtual hosts.
 * Basic validation for domain format.
 */
export const DomainSchema = z
	.string()
	.min(1, 'Domain is required')
	.transform((val) => val.trim())
	.refine(
		(val) => {
			// Basic domain validation: at least one dot, no spaces, valid characters
			const domainPattern = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/i;
			// Also allow wildcards like *.example.com
			const wildcardPattern = /^\*\.([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)*[a-z0-9]([a-z0-9-]*[a-z0-9])?$/i;
			return domainPattern.test(val) || wildcardPattern.test(val);
		},
		{
			message: 'Domain must be a valid domain name (e.g., example.com or *.example.com)'
		}
	);

/**
 * Schema for retry conditions.
 * Common Envoy retry conditions.
 */
const RetryOnConditions = [
	'5xx',
	'gateway-error',
	'connect-failure',
	'retriable-4xx',
	'refused-stream',
	'reset',
	'retriable-status-codes',
	'retriable-headers',
	'envoy-ratelimited'
] as const;

/**
 * Main route form schema.
 * Validates individual route configuration within a virtual host.
 */
export const RouteFormSchema = z
	.object({
		/** Unique route name (lowercase, alphanumeric with dashes) */
		name: RouteNameSchema,

		/** HTTP method (GET, POST, etc. or ANY for all methods) */
		method: z.enum(['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS', 'ANY'], {
			message: 'Invalid HTTP method'
		}),

		/** Path pattern for route matching */
		path: PathSchema,

		/** Type of path matching to use */
		pathType: z.enum(['prefix', 'exact', 'template', 'regex'], {
			message: 'Invalid path matching type'
		}),

		/** Target cluster name for traffic routing */
		cluster: z.string().min(1, 'Target cluster is required').transform((val) => val.trim()),

		/** Request timeout in seconds (1-300) */
		timeout: z
			.number({ message: 'Timeout must be a number' })
			.int('Timeout must be an integer')
			.min(1, 'Timeout must be at least 1 second')
			.max(300, 'Timeout cannot exceed 300 seconds')
			.optional()
			.default(30),

		/** Path prefix rewrite rule (for prefix/exact/regex paths) */
		prefixRewrite: z.string().trim().optional(),

		/** Path template rewrite rule (for template paths) */
		templateRewrite: z.string().trim().optional(),

		/** Whether retry policy is enabled */
		retryEnabled: z.boolean().optional().default(false),

		/** Maximum number of retry attempts (1-10) */
		maxRetries: z
			.number({ message: 'Max retries must be a number' })
			.int('Max retries must be an integer')
			.min(1, 'Max retries must be at least 1')
			.max(10, 'Max retries cannot exceed 10')
			.optional(),

		/** Retry conditions (when to retry) */
		retryOn: z.array(z.enum(RetryOnConditions)).optional(),

		/** Per-try timeout in seconds (1-60) */
		perTryTimeout: z
			.number({ message: 'Per-try timeout must be a number' })
			.int('Per-try timeout must be an integer')
			.min(1, 'Per-try timeout must be at least 1 second')
			.max(60, 'Per-try timeout cannot exceed 60 seconds')
			.optional(),

		/** Backoff base interval in milliseconds (0-60000) */
		backoffBaseMs: z
			.number({ message: 'Backoff base interval must be a number' })
			.int('Backoff base interval must be an integer')
			.min(0, 'Backoff base interval cannot be negative')
			.max(60000, 'Backoff base interval cannot exceed 60 seconds')
			.optional(),

		/** Backoff max interval in milliseconds (0-300000) */
		backoffMaxMs: z
			.number({ message: 'Backoff max interval must be a number' })
			.int('Backoff max interval must be an integer')
			.min(0, 'Backoff max interval cannot be negative')
			.max(300000, 'Backoff max interval cannot exceed 300 seconds')
			.optional()
	})
	.refine(
		(data) => {
			// If pathType is template, templateRewrite can be used (prefixRewrite is ignored)
			// If pathType is prefix/exact/regex, prefixRewrite can be used (templateRewrite is ignored)
			// This is informational - both can be present, but only one will be used
			return true;
		},
		{
			message: 'Path rewrite configuration is valid'
		}
	)
	.refine(
		(data) => {
			// If retry is enabled, required retry fields must be present
			if (data.retryEnabled) {
				return (
					data.maxRetries !== undefined &&
					data.retryOn !== undefined &&
					data.retryOn.length > 0 &&
					data.perTryTimeout !== undefined
				);
			}
			return true;
		},
		{
			message: 'When retry is enabled, max retries, retry conditions, and per-try timeout are required',
			path: ['retryEnabled']
		}
	)
	.refine(
		(data) => {
			// If backoff intervals are specified, max must be >= base
			if (data.backoffBaseMs !== undefined && data.backoffMaxMs !== undefined) {
				return data.backoffMaxMs >= data.backoffBaseMs;
			}
			return true;
		},
		{
			message: 'Backoff max interval must be greater than or equal to base interval',
			path: ['backoffMaxMs']
		}
	);

/**
 * Virtual host form schema.
 * Validates a virtual host with its domains and routes.
 */
export const VirtualHostFormSchema = z.object({
	/** Virtual host name (lowercase, alphanumeric with dashes) */
	name: RouteNameSchema,

	/** List of domains this virtual host matches */
	domains: z
		.array(DomainSchema)
		.min(1, 'At least one domain is required')
		.refine(
			(domains) => {
				// Check for duplicate domains
				const uniqueDomains = new Set(domains.map((d) => d.toLowerCase()));
				return uniqueDomains.size === domains.length;
			},
			{
				message: 'Duplicate domains are not allowed'
			}
		),

	/** Routes within this virtual host */
	routes: z.array(RouteFormSchema).min(1, 'At least one route is required').refine(
		(routes) => {
			// Check for duplicate route names
			const routeNames = routes.map((r) => r.name);
			const uniqueNames = new Set(routeNames);
			return uniqueNames.size === routeNames.length;
		},
		{
			message: 'Duplicate route names are not allowed within the same virtual host'
		}
	)
});

/**
 * Route configuration form schema.
 * Top-level schema for creating/editing route configurations.
 */
export const RouteConfigFormSchema = z.object({
	/** Configuration name (lowercase, alphanumeric with dashes) */
	name: RouteNameSchema,

	/** Team identifier */
	team: z.string().min(1, 'Team is required').transform((val) => val.trim()),

	/** Virtual hosts in this configuration */
	virtualHosts: z
		.array(VirtualHostFormSchema)
		.min(1, 'At least one virtual host is required')
		.refine(
			(vhosts) => {
				// Check for duplicate virtual host names
				const vhostNames = vhosts.map((vh) => vh.name);
				const uniqueNames = new Set(vhostNames);
				return uniqueNames.size === vhostNames.length;
			},
			{
				message: 'Duplicate virtual host names are not allowed'
			}
		)
		.refine(
			(vhosts) => {
				// Check for duplicate domains across virtual hosts
				const allDomains: string[] = [];
				for (const vh of vhosts) {
					allDomains.push(...vh.domains);
				}
				const uniqueDomains = new Set(allDomains.map((d) => d.toLowerCase()));
				return uniqueDomains.size === allDomains.length;
			},
			{
				message: 'Duplicate domains across virtual hosts are not allowed'
			}
		)
});

/**
 * MCP (Model Context Protocol) configuration schema.
 * Validates MCP tool settings for routes.
 */
export const McpConfigSchema = z
	.object({
		/** Whether MCP tool is enabled for this route */
		enabled: z.boolean(),

		/** MCP tool name (required when enabled) */
		toolName: z.string().trim().optional(),

		/** Human-readable description of the tool */
		description: z.string().trim().optional(),

		/** Source of the API schema (auto-detected, OpenAPI, learned, or manual) */
		schemaSource: z.enum(['auto', 'openapi', 'learned', 'manual']).optional()
	})
	.refine(
		(data) => {
			// If MCP is enabled, tool name is required
			if (data.enabled) {
				return data.toolName !== undefined && data.toolName.length > 0;
			}
			return true;
		},
		{
			message: 'Tool name is required when MCP is enabled',
			path: ['toolName']
		}
	);

/**
 * Type inference exports for TypeScript.
 * Use these types in your components for proper type safety.
 */
export type RouteFormData = z.infer<typeof RouteFormSchema>;
export type VirtualHostFormData = z.infer<typeof VirtualHostFormSchema>;
export type RouteConfigFormData = z.infer<typeof RouteConfigFormSchema>;
export type McpConfigData = z.infer<typeof McpConfigSchema>;

/**
 * Helper type for partial route form data (useful for edit forms with optional fields).
 */
export type PartialRouteFormData = Partial<RouteFormData>;

/**
 * Helper type for partial virtual host form data.
 */
export type PartialVirtualHostFormData = Partial<VirtualHostFormData>;

/**
 * Helper type for partial route config form data.
 */
export type PartialRouteConfigFormData = Partial<RouteConfigFormData>;
