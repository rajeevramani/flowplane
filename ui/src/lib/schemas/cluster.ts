/**
 * Zod schemas for cluster API responses.
 * Validates cluster config matching the backend ClusterSpec (xds/cluster_spec.rs).
 */
import { z } from 'zod';

/**
 * Endpoint can be a string ("host:port") or structured { host, port }.
 */
const EndpointSpecSchema = z.union([
	z.string(),
	z.object({
		host: z.string(),
		port: z.number()
	})
]);

/**
 * Health check spec — tagged union by "type" field.
 * Backend serializes with #[serde(tag = "type", rename_all = "lowercase")].
 */
const HealthCheckSpecSchema = z.discriminatedUnion('type', [
	z.object({
		type: z.literal('http'),
		path: z.string(),
		host: z.string().optional(),
		method: z.string().optional(),
		intervalSeconds: z.number().optional(),
		timeoutSeconds: z.number().optional(),
		healthyThreshold: z.number().optional(),
		unhealthyThreshold: z.number().optional(),
		expectedStatuses: z.array(z.number()).optional()
	}),
	z.object({
		type: z.literal('tcp'),
		intervalSeconds: z.number().optional(),
		timeoutSeconds: z.number().optional(),
		healthyThreshold: z.number().optional(),
		unhealthyThreshold: z.number().optional()
	})
]);

/**
 * Circuit breaker thresholds.
 */
const CircuitBreakerThresholdsSpecSchema = z.object({
	maxConnections: z.number().optional(),
	maxPendingRequests: z.number().optional(),
	maxRequests: z.number().optional(),
	maxRetries: z.number().optional()
}).passthrough();

/**
 * Circuit breakers with default and high priority thresholds.
 */
const CircuitBreakersSpecSchema = z.object({
	default: CircuitBreakerThresholdsSpecSchema.optional(),
	high: CircuitBreakerThresholdsSpecSchema.optional()
}).passthrough();

/**
 * Outlier detection configuration.
 */
const OutlierDetectionSpecSchema = z.object({
	consecutive5xx: z.number().optional(),
	intervalSeconds: z.number().optional(),
	baseEjectionTimeSeconds: z.number().optional(),
	maxEjectionPercent: z.number().optional(),
	minHosts: z.number().optional()
}).passthrough();

/**
 * LB policy-specific configs.
 */
const LeastRequestPolicySchema = z.object({
	choiceCount: z.number().optional()
}).passthrough();

const RingHashPolicySchema = z.object({
	minimumRingSize: z.number().optional(),
	maximumRingSize: z.number().optional(),
	hashFunction: z.string().optional()
}).passthrough();

const MaglevPolicySchema = z.object({
	tableSize: z.number().optional()
}).passthrough();

/**
 * ClusterConfig schema — matches backend ClusterSpec serialized as camelCase.
 * Uses passthrough() to allow additional Envoy xDS fields that may be present.
 */
export const ClusterConfigSchema = z.object({
	connectTimeoutSeconds: z.number().optional(),
	endpoints: z.array(EndpointSpecSchema).default([]),
	useTls: z.boolean().optional(),
	tlsServerName: z.string().optional(),
	dnsLookupFamily: z.string().optional(),
	lbPolicy: z.string().optional(),
	leastRequest: LeastRequestPolicySchema.optional(),
	ringHash: RingHashPolicySchema.optional(),
	maglev: MaglevPolicySchema.optional(),
	circuitBreakers: CircuitBreakersSpecSchema.optional(),
	healthChecks: z.array(HealthCheckSpecSchema).default([]),
	outlierDetection: OutlierDetectionSpecSchema.optional(),
	protocolType: z.string().optional()
}).passthrough();

/**
 * ClusterResponse schema — matches backend ClusterResponse.
 */
export const ClusterResponseSchema = z.object({
	name: z.string(),
	team: z.string(),
	serviceName: z.string(),
	importId: z.string().optional(),
	config: ClusterConfigSchema
});

/**
 * Type exports inferred from schemas.
 */
export type ClusterConfig = z.infer<typeof ClusterConfigSchema>;
export type ClusterResponseData = z.infer<typeof ClusterResponseSchema>;
