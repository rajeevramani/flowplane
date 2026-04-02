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

// ============================================================================
// Compressor Filter Schema
// ============================================================================

const CompressorCommonConfigSchema = z.object({
	min_content_length: z.number().int().min(0).optional(),
	content_type: z.array(z.string()).optional(),
	disable_on_etag_header: z.boolean().optional(),
	remove_accept_encoding_header: z.boolean().optional()
});

const CompressorResponseDirectionConfigSchema = z.object({
	common_config: CompressorCommonConfigSchema.optional()
});

const CompressorLibraryConfigSchema = z.object({
	type: z.literal('gzip'),
	compression_level: z.enum(['best_speed', 'best_compression', 'default_compression']).optional(),
	compression_strategy: z.enum(['default_strategy', 'filtered', 'huffman_only', 'rle', 'fixed']).optional(),
	memory_level: z.number().int().min(1).max(9).optional(),
	window_bits: z.number().int().min(9).max(15).optional(),
	chunk_size: z.number().int().min(1024).optional()
});

export const CompressorConfigSchema = z.object({
	response_direction_config: CompressorResponseDirectionConfigSchema.optional(),
	compressor_library: CompressorLibraryConfigSchema.optional()
});

export type CompressorConfigData = z.infer<typeof CompressorConfigSchema>;

// ============================================================================
// External Authorization Filter Schema
// ============================================================================

const ExtAuthzHeaderKeyValueSchema = z.object({
	key: z.string().min(1, 'Header key is required'),
	value: z.string()
});

const ExtAuthzServerUriSchema = z.object({
	uri: z.string().optional(),
	cluster: z.string().optional(),
	timeout_ms: z.number().int().min(1).optional()
});

const ExtAuthzServiceConfigSchema = z.object({
	type: z.enum(['grpc', 'http']),
	target_uri: z.string().optional(),
	timeout_ms: z.number().int().min(1).optional(),
	initial_metadata: z.array(ExtAuthzHeaderKeyValueSchema).optional(),
	server_uri: ExtAuthzServerUriSchema.optional(),
	path_prefix: z.string().optional(),
	headers_to_add: z.array(ExtAuthzHeaderKeyValueSchema).optional(),
	authorization_request: z.object({
		allowed_headers: z.array(z.string()).optional(),
		headers_to_add: z.array(ExtAuthzHeaderKeyValueSchema).optional()
	}).optional(),
	authorization_response: z.object({
		allowed_upstream_headers: z.array(z.string()).optional(),
		allowed_client_headers: z.array(z.string()).optional(),
		allowed_client_headers_on_success: z.array(z.string()).optional()
	}).optional()
});

const ExtAuthzWithRequestBodySchema = z.object({
	max_request_bytes: z.number().int().min(0).optional(),
	allow_partial_message: z.boolean().optional(),
	pack_as_bytes: z.boolean().optional()
});

export const ExtAuthzConfigSchema = z.object({
	service: ExtAuthzServiceConfigSchema,
	failure_mode_allow: z.boolean().optional(),
	with_request_body: ExtAuthzWithRequestBodySchema.optional(),
	clear_route_cache: z.boolean().optional(),
	status_on_error: z.number().int().min(100).max(599).optional(),
	stat_prefix: z.string().optional(),
	include_peer_certificate: z.boolean().optional()
});

export type ExtAuthzConfigData = z.infer<typeof ExtAuthzConfigSchema>;
