/**
 * Centralized export for all Zod schemas.
 * Import schemas from here for consistency.
 */

// Authentication schemas
export {
	loginSchema,
	type LoginSchema,
	bootstrapSchema,
	type BootstrapSchema
} from './auth';

// Route configuration schemas
export {
	RouteNameSchema,
	PathSchema,
	DomainSchema,
	RouteFormSchema,
	VirtualHostFormSchema,
	RouteConfigFormSchema,
	McpConfigSchema,
	type RouteFormData,
	type VirtualHostFormData,
	type RouteConfigFormData,
	type McpConfigData,
	type PartialRouteFormData,
	type PartialVirtualHostFormData,
	type PartialRouteConfigFormData
} from './route-config';
