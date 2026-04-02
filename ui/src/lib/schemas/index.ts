/**
 * Centralized export for all Zod schemas.
 * Import schemas from here for consistency.
 */

// Authentication schemas
export {
	inviteMemberSchema,
	type InviteMemberSchema
} from './auth';

// Cluster schemas
export {
	ClusterConfigSchema,
	ClusterResponseSchema,
	type ClusterConfig,
	type ClusterResponseData
} from './cluster';

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
