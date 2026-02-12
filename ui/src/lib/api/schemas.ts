import { z } from 'zod';

// PaginatedResponse schema factory
export function paginatedSchema<T extends z.ZodType>(itemSchema: T) {
	return z.object({
		items: z.array(itemSchema),
		total: z.number(),
		limit: z.number(),
		offset: z.number()
	});
}

// SecretResponse schema
export const SecretResponseSchema = z.object({
	id: z.string(),
	name: z.string(),
	secret_type: z.enum([
		'generic_secret',
		'tls_certificate',
		'certificate_validation_context',
		'session_ticket_keys'
	]),
	description: z.string().nullable(),
	version: z.number(),
	source: z.string(),
	team: z.string(),
	created_at: z.string(),
	updated_at: z.string(),
	expires_at: z.string().nullable(),
	backend: z.enum(['vault', 'aws_secrets_manager', 'gcp_secret_manager']).optional(),
	reference: z.string().optional(),
	reference_version: z.string().optional()
});

// OrganizationResponse schema
export const OrganizationResponseSchema = z.object({
	id: z.string(),
	name: z.string(),
	displayName: z.string(),
	description: z.string().optional(),
	ownerUserId: z.string().optional(),
	status: z.enum(['active', 'suspended', 'archived']),
	createdAt: z.string(),
	updatedAt: z.string()
});

// AdminListOrgsResponse schema
export const AdminListOrgsResponseSchema = paginatedSchema(OrganizationResponseSchema);

// SessionInfoResponse schema
export const SessionInfoResponseSchema = z.object({
	sessionId: z.string(),
	userId: z.string(),
	name: z.string(),
	email: z.string(),
	isAdmin: z.boolean(),
	isPlatformAdmin: z.boolean(),
	teams: z.array(z.string()),
	scopes: z.array(z.string()),
	expiresAt: z.string().nullable(),
	version: z.string(),
	orgId: z.string().optional(),
	orgName: z.string().optional(),
	orgRole: z.string().optional()
});

// Admin Resource Summary schemas
const TeamSummarySchema = z.object({
	teamName: z.string(),
	teamDisplayName: z.string(),
	clusters: z.number(),
	listeners: z.number(),
	routeConfigs: z.number(),
	filters: z.number(),
	dataplanes: z.number(),
	secrets: z.number(),
	imports: z.number()
});

const OrgSummarySchema = z.object({
	orgId: z.string().nullable(),
	orgName: z.string().nullable(),
	teams: z.array(TeamSummarySchema)
});

const SummaryTotalsSchema = z.object({
	teams: z.number(),
	clusters: z.number(),
	listeners: z.number(),
	routeConfigs: z.number(),
	filters: z.number(),
	dataplanes: z.number(),
	secrets: z.number(),
	imports: z.number()
});

export const AdminResourceSummarySchema = z.object({
	totals: SummaryTotalsSchema,
	orgs: z.array(OrgSummarySchema)
});

// TeamResponse schema
export const TeamResponseSchema = z.object({
	id: z.string(),
	name: z.string(),
	displayName: z.string(),
	description: z.string().nullable(),
	ownerUserId: z.string().nullable(),
	settings: z.record(z.string(), z.unknown()).nullable(),
	status: z.enum(['active', 'suspended', 'archived']),
	envoyAdminPort: z.number().nullable(),
	createdAt: z.string(),
	updatedAt: z.string(),
	orgId: z.string().optional()
});
