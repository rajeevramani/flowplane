/**
 * Zod schemas for custom filter configuration forms.
 * Validates CORS and Rate Limit filter configs on the client side.
 */

import { z } from 'zod';

// ============================================================================
// CORS Filter Schema
// ============================================================================

const CorsMatchTypeSchema = z.enum(['exact', 'prefix', 'suffix', 'contains', 'regex']);

const CorsOriginMatcherSchema = z.object({
	type: CorsMatchTypeSchema,
	value: z.string().min(1, 'Origin value is required')
});

const RuntimeFractionalPercentSchema = z.object({
	runtime_key: z.string().optional(),
	numerator: z.number().int().min(0),
	denominator: z.enum(['hundred', 'ten_thousand', 'million']).optional()
});

const CorsPolicySchema = z.object({
	allow_origin: z
		.array(CorsOriginMatcherSchema)
		.min(1, 'At least one origin matcher is required'),
	allow_methods: z.array(z.string()).optional(),
	allow_headers: z.array(z.string()).optional(),
	expose_headers: z.array(z.string()).optional(),
	max_age: z.number().int().min(0).max(315576000000).optional(),
	allow_credentials: z.boolean().optional(),
	filter_enabled: RuntimeFractionalPercentSchema.optional(),
	shadow_enabled: RuntimeFractionalPercentSchema.optional(),
	allow_private_network_access: z.boolean().optional(),
	forward_not_matching_preflights: z.boolean().optional()
});

export const CorsConfigSchema = z.object({
	policy: CorsPolicySchema
});

export type CorsConfigData = z.infer<typeof CorsConfigSchema>;

// ============================================================================
// Rate Limit (External) Filter Schema
// ============================================================================

const RateLimitGrpcServiceSchema = z.object({
	envoy_grpc: z
		.object({
			cluster_name: z.string().min(1, 'Cluster name is required'),
			authority: z.string().optional()
		})
		.optional(),
	google_grpc: z
		.object({
			target_uri: z.string().min(1, 'Target URI is required'),
			stat_prefix: z.string().optional()
		})
		.optional(),
	timeout: z.string().optional()
});

const RateLimitServiceConfigSchema = z.object({
	grpc_service: RateLimitGrpcServiceSchema,
	transport_api_version: z.string().optional()
});

export const RateLimitConfigSchema = z.object({
	domain: z.string().min(1, 'Domain is required'),
	rate_limit_service: RateLimitServiceConfigSchema,
	stage: z.number().int().min(0).max(10).optional(),
	request_type: z.enum(['internal', 'external', 'both']).optional(),
	timeout: z.string().optional(),
	failure_mode_deny: z.boolean().optional(),
	rate_limited_as_resource_exhausted: z.boolean().optional(),
	enable_x_ratelimit_headers: z.enum(['OFF', 'DRAFT_VERSION_03']).optional()
});

export type RateLimitConfigData = z.infer<typeof RateLimitConfigSchema>;
