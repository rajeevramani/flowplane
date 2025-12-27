// API response types matching backend DTOs

export interface LoginRequest {
	email: string;
	password: string;
}

export interface LoginResponse {
	sessionId: string;
	csrfToken: string;
	expiresAt: string;
	userId: string;
	userEmail: string;
	teams: string[];
	scopes: string[];
}

export interface ChangePasswordRequest {
	currentPassword: string;
	newPassword: string;
}

export interface BootstrapStatusResponse {
	needsInitialization: boolean;
	message: string;
}

export interface BootstrapInitializeRequest {
	email: string;
	password: string;
	name: string;
}

export interface BootstrapInitializeResponse {
	setupToken: string;
	expiresAt: string;
	maxUsageCount: number;
	message: string;
	nextSteps: string[];
}

export interface SessionInfoResponse {
	sessionId: string;
	userId: string;
	name: string;
	email: string;
	isAdmin: boolean;
	teams: string[];
	scopes: string[];
	expiresAt: string | null;
	version: string;
}

export interface ListTeamsResponse {
	teams: string[];
}

export type TeamStatus = 'active' | 'suspended' | 'archived';

export interface TeamResponse {
	id: string;
	name: string;
	displayName: string;
	description: string | null;
	ownerUserId: string | null;
	settings: any | null;
	status: TeamStatus;
	envoyAdminPort: number | null;
	createdAt: string;
	updatedAt: string;
}

export interface CreateTeamRequest {
	name: string;
	displayName: string;
	description?: string | null;
	ownerUserId?: string | null;
	settings?: any | null;
}

export interface UpdateTeamRequest {
	displayName?: string;
	description?: string | null;
	ownerUserId?: string | null;
	settings?: any | null;
	status?: TeamStatus;
}

export interface AdminListTeamsResponse {
	teams: TeamResponse[];
	total: number;
	limit: number;
	offset: number;
}

export interface DashboardStats {
	importsCount: number;
	listenersCount: number;
	routesCount: number;
	clustersCount: number;
}

export interface ApiError {
	message: string;
	code?: string;
}

export type TokenStatus = 'Active' | 'Revoked' | 'Expired';

export interface PersonalAccessToken {
	id: string;
	name: string;
	description: string | null;
	status: TokenStatus;
	expiresAt: string | null;
	lastUsedAt: string | null;
	createdBy: string | null;
	createdAt: string;
	updatedAt: string;
	scopes: string[];
}

export interface CreateTokenRequest {
	name: string;
	description?: string;
	expiresAt?: string | null;
	scopes: string[];
}

export interface TokenSecretResponse {
	id: string;
	token: string;
}

export interface UpdateTokenRequest {
	name?: string;
	description?: string;
}

export interface ImportOpenApiRequest {
	spec: string; // YAML or JSON string
	team?: string;
	listenerMode: 'existing' | 'new';
	existingListenerName?: string; // when mode='existing'
	newListenerName?: string; // when mode='new'
	newListenerAddress?: string;
	newListenerPort?: number;
}

export interface ImportResponse {
	importId: string;
	specName: string;
	specVersion: string | null;
	routesCreated: number;
	clustersCreated: number;
	clustersReused: number;
	listenerName: string | null;
}

export interface OpenApiSpec {
	openapi?: string;
	swagger?: string;
	info: {
		title: string;
		version: string;
		description?: string;
	};
	servers?: Array<{
		url: string;
		description?: string;
	}>;
	paths: Record<string, any>;
}

// Import types (replacing API Definition types)
export interface ImportSummary {
	id: string;
	specName: string;
	specVersion: string | null;
	team: string;
	listenerName: string | null;
	importedAt: string;
	updatedAt: string;
}

export interface ImportDetailsResponse {
	id: string;
	specName: string;
	specVersion: string | null;
	specChecksum: string | null;
	team: string;
	listenerName: string | null;
	importedAt: string;
	updatedAt: string;
	routeCount: number;
	clusterCount: number;
	listenerCount: number;
}

// Listener types
export interface ListenerResponse {
	name: string;
	team: string;
	address: string;
	port: number | null;
	protocol: string;
	version: number;
	importId?: string;
	config: any; // Full listener config
}

// Route types
export interface RouteResponse {
	name: string;
	team: string;
	pathPrefix: string;
	clusterTargets: string;
	importId?: string;
	routeOrder?: number;
	config: any; // Full route config
}

// Legacy type - no longer used after API definitions removal
// Routes are now accessed directly via RouteResponse

// Cluster types
export interface ClusterResponse {
	name: string;
	team: string;
	serviceName: string;
	importId?: string;
	config: any; // Full cluster config
}

// Bootstrap configuration types
export interface BootstrapConfigRequest {
	team: string;
	format?: 'yaml' | 'json';
}

// User Management types
export type UserStatus = 'Active' | 'Inactive' | 'Suspended';

export interface UserResponse {
	id: string;
	email: string;
	name: string;
	status: UserStatus;
	isAdmin: boolean;
	createdAt: string;
	updatedAt: string;
}

export interface UserTeamMembership {
	id: string;
	userId: string;
	team: string;
	scopes: string[];
	createdAt: string;
}

export interface UserWithTeamsResponse {
	id: string;
	email: string;
	name: string;
	status: UserStatus;
	isAdmin: boolean;
	createdAt: string;
	updatedAt: string;
	teams: UserTeamMembership[];
}

export interface CreateUserRequest {
	email: string;
	password: string;
	name: string;
	isAdmin?: boolean;
}

export interface UpdateUserRequest {
	email?: string;
	name?: string;
	status?: UserStatus;
	isAdmin?: boolean;
}

export interface CreateTeamMembershipRequest {
	userId: string;
	team: string;
	scopes: string[];
}

export interface ListUsersResponse {
	users: UserResponse[];
	total: number;
	limit: number;
	offset: number;
}

// Audit Log Types
export interface AuditLogEntry {
	id: number;
	resourceType: string;
	resourceId: string | null;
	resourceName: string | null;
	action: string;
	oldConfiguration: string | null;
	newConfiguration: string | null;
	userId: string | null;
	clientIp: string | null;
	userAgent: string | null;
	createdAt: string;
}

export interface ListAuditLogsQuery {
	resource_type?: string;
	action?: string;
	user_id?: string;
	start_date?: string;
	end_date?: string;
	limit?: number;
	offset?: number;
}

export interface ListAuditLogsResponse {
	entries: AuditLogEntry[];
	total: number;
	limit: number;
	offset: number;
}

// === Create API Types ===

// Cluster creation types
export interface EndpointRequest {
	host: string;
	port: number;
}

export interface HealthCheckRequest {
	type?: string;
	path?: string;
	host?: string;
	method?: string;
	intervalSeconds?: number;
	timeoutSeconds?: number;
	healthyThreshold?: number;
	unhealthyThreshold?: number;
	expectedStatuses?: number[];
}

export interface CircuitBreakerThresholdsRequest {
	maxConnections?: number;
	maxPendingRequests?: number;
	maxRequests?: number;
	maxRetries?: number;
}

export interface CircuitBreakersRequest {
	default?: CircuitBreakerThresholdsRequest;
	high?: CircuitBreakerThresholdsRequest;
}

export interface OutlierDetectionRequest {
	consecutive5xx?: number;
	intervalSeconds?: number;
	baseEjectionTimeSeconds?: number;
	maxEjectionPercent?: number;
	minHosts?: number;
}

export interface CreateClusterBody {
	team: string;
	name: string;
	serviceName?: string;
	endpoints: EndpointRequest[];
	connectTimeoutSeconds?: number;
	useTls?: boolean;
	tlsServerName?: string;
	dnsLookupFamily?: 'AUTO' | 'V4_ONLY' | 'V6_ONLY' | 'V4_PREFERRED' | 'ALL';
	lbPolicy?: 'ROUND_ROBIN' | 'LEAST_REQUEST' | 'RANDOM' | 'RING_HASH' | 'MAGLEV' | 'CLUSTER_PROVIDED';
	healthChecks?: HealthCheckRequest[];
	circuitBreakers?: CircuitBreakersRequest;
	outlierDetection?: OutlierDetectionRequest;
}

// Route creation types
export type PathMatchType = 'exact' | 'prefix' | 'regex' | 'template';

export interface PathMatchDefinition {
	type: PathMatchType;
	value?: string;
	template?: string;
}

export interface HeaderMatchDefinition {
	name: string;
	value?: string;
	regex?: string;
	present?: boolean;
}

export interface QueryParameterMatchDefinition {
	name: string;
	value?: string;
	regex?: string;
	present?: boolean;
}

export interface RouteMatchDefinition {
	path: PathMatchDefinition;
	headers?: HeaderMatchDefinition[];
	queryParameters?: QueryParameterMatchDefinition[];
}

export interface WeightedClusterDefinition {
	name: string;
	weight: number;
}

export interface BackoffConfig {
	baseIntervalMs?: number;
	maxIntervalMs?: number;
}

export interface RetryPolicyDefinition {
	maxRetries?: number;
	retryOn: string[];
	perTryTimeoutSeconds?: number;
	backoff?: BackoffConfig;
}

export type RouteActionDefinition =
	| {
			type: 'forward';
			cluster: string;
			timeoutSeconds?: number;
			prefixRewrite?: string;
			templateRewrite?: string;
			retryPolicy?: RetryPolicyDefinition;
	  }
	| {
			type: 'weighted';
			clusters: WeightedClusterDefinition[];
			totalWeight?: number;
	  }
	| {
			type: 'redirect';
			hostRedirect?: string;
			pathRedirect?: string;
			responseCode?: number;
	  };

export interface RouteRuleDefinition {
	name?: string;
	match: RouteMatchDefinition;
	action: RouteActionDefinition;
	typedPerFilterConfig?: {
		'envoy.filters.http.header_mutation'?: HeaderMutationPerRouteConfig;
	};
}

export interface VirtualHostDefinition {
	name: string;
	domains: string[];
	routes: RouteRuleDefinition[];
}

export interface CreateRouteBody {
	team: string;
	name: string;
	virtualHosts: VirtualHostDefinition[];
}

// UpdateRouteBody - full payload required for route updates
// Note: team and name must match existing route
export interface UpdateRouteBody {
	team: string;
	name: string;
	virtualHosts: VirtualHostDefinition[];
}

// Listener creation types
export interface ListenerTlsContextInput {
	certChainFile?: string;
	privateKeyFile?: string;
	caCertFile?: string;
	requireClientCertificate?: boolean;
	minTlsVersion?: 'V1_0' | 'V1_1' | 'V1_2' | 'V1_3';
}

export interface ListenerAccessLogInput {
	path?: string;
	format?: string;
}

export interface ListenerTracingInput {
	provider: string;
	config: Record<string, unknown>;
}

// Header Mutation Filter Types
export interface HeaderMutationEntry {
	key: string;
	value: string;
	append: boolean;
}

export interface HeaderMutationConfig {
	requestHeadersToAdd?: HeaderMutationEntry[];
	requestHeadersToRemove?: string[];
	responseHeadersToAdd?: HeaderMutationEntry[];
	responseHeadersToRemove?: string[];
}

export interface HeaderMutationPerRouteConfig {
	requestHeadersToAdd?: HeaderMutationEntry[];
	requestHeadersToRemove?: string[];
	responseHeadersToAdd?: HeaderMutationEntry[];
	responseHeadersToRemove?: string[];
}

// HttpFilterKind - discriminated union for filter types
export type HttpFilterKind =
	| { type: 'router' }
	| { type: 'cors'; config: unknown }
	| { type: 'local_rate_limit'; config: unknown }
	| { type: 'jwt_authn'; config: unknown }
	| { type: 'rate_limit'; config: unknown }
	| { type: 'header_mutation'; config: HeaderMutationConfig }
	| { type: 'health_check'; config: unknown };

// HttpFilterConfigEntry - matches Rust HttpFilterConfigEntry
export interface HttpFilterConfigEntry {
	name?: string;
	isOptional?: boolean;
	disabled?: boolean;
	filter: HttpFilterKind;
}

// ListenerFilterInput uses flattened structure - type discriminator is at the same level as name
export type ListenerFilterInput =
	| {
			name: string;
			type: 'httpConnectionManager';
			routeConfigName?: string;
			inlineRouteConfig?: unknown;
			accessLog?: ListenerAccessLogInput;
			tracing?: ListenerTracingInput;
			httpFilters?: HttpFilterConfigEntry[];
	  }
	| {
			name: string;
			type: 'tcpProxy';
			cluster: string;
			accessLog?: ListenerAccessLogInput;
	  };

export interface ListenerFilterChainInput {
	name?: string;
	filters: ListenerFilterInput[];
	tlsContext?: ListenerTlsContextInput;
}

export interface CreateListenerBody {
	team: string;
	name: string;
	address: string;
	port: number;
	protocol?: string;
	filterChains: ListenerFilterChainInput[];
}

// UpdateListenerBody - no name or team fields (from path param / existing listener)
export interface UpdateListenerBody {
	address: string;
	port: number;
	filterChains: ListenerFilterChainInput[];
	protocol?: string;
}

// === Scope Types ===

export interface ScopeDefinition {
	id: string;
	value: string;
	resource: string;
	action: string;
	label: string;
	description: string | null;
	category: string;
	visibleInUi: boolean;
	enabled: boolean;
	createdAt: string;
	updatedAt: string;
}

export interface ListScopesResponse {
	scopes: ScopeDefinition[];
	count: number;
}

// === Filter Types ===

// Attachment point - where a filter can be attached
export type AttachmentPoint = 'route' | 'listener' | 'cluster';

// Filter type uses snake_case to match backend serde serialization
// Note: jwt_authn is the Envoy filter name variant (with 'n')
export type FilterType = 'header_mutation' | 'jwt_auth' | 'jwt_authn' | 'cors' | 'local_rate_limit' | 'rate_limit' | 'ext_authz' | 'custom_response' | 'mcp';

// ============================================================================
// JWT Authentication Filter Types
// ============================================================================

// JWKS Source: Remote configuration (HTTP URI with cluster)
export interface RemoteJwksConfig {
	http_uri: {
		uri: string;
		cluster: string;
		timeout_ms?: number;
	};
	cache_duration_seconds?: number;
	async_fetch?: {
		fast_listener?: boolean;
		failed_refetch_duration_seconds?: number;
	};
	retry_policy?: {
		num_retries?: number;
		retry_backoff?: {
			base_interval_ms?: number;
			max_interval_ms?: number;
		};
	};
}

// JWKS Source: Local configuration (inline or file)
export interface LocalJwksConfig {
	filename?: string;
	inline_string?: string;
	inline_bytes?: string;
	environment_variable?: string;
}

// JWKS Source: Discriminated union (matches Rust tagged enum)
export type JwtJwksSourceConfig =
	| { type: 'remote'; } & RemoteJwksConfig
	| { type: 'local'; } & LocalJwksConfig;

// JWT Header extraction configuration
export interface JwtHeaderConfig {
	name: string;
	value_prefix?: string;
}

// JWT Claim to header forwarding configuration
export interface JwtClaimToHeaderConfig {
	header_name: string;
	claim_name: string;
}

// String matcher for subject validation
export type JwtStringMatcherConfig =
	| { type: 'exact'; value: string }
	| { type: 'prefix'; value: string }
	| { type: 'suffix'; value: string }
	| { type: 'contains'; value: string }
	| { type: 'regex'; value: string };

// Normalize payload configuration
export interface JwtNormalizePayloadConfig {
	space_delimited_claims?: string[];
}

// JWT cache configuration
export interface JwtCacheConfig {
	jwt_cache_size?: number;
	jwt_max_token_size?: number;
}

// JWT Provider configuration
export interface JwtProviderConfig {
	issuer?: string;
	audiences?: string[];
	subjects?: JwtStringMatcherConfig;
	require_expiration?: boolean;
	max_lifetime_seconds?: number;
	clock_skew_seconds?: number;
	forward?: boolean;
	from_headers?: JwtHeaderConfig[];
	from_params?: string[];
	from_cookies?: string[];
	forward_payload_header?: string;
	pad_forward_payload_header?: boolean;
	payload_in_metadata?: string;
	header_in_metadata?: string;
	failed_status_in_metadata?: string;
	normalize_payload_in_metadata?: JwtNormalizePayloadConfig;
	jwt_cache_config?: JwtCacheConfig;
	claim_to_headers?: JwtClaimToHeaderConfig[];
	clear_route_cache?: boolean;
	jwks: JwtJwksSourceConfig;
}

// JWT Requirement: Discriminated union for requirement types
export type JwtRequirementConfig =
	| { type: 'provider_name'; provider_name: string }
	| { type: 'requires_any'; requirements: JwtRequirementConfig[] }
	| { type: 'requires_all'; requirements: JwtRequirementConfig[] }
	| { type: 'allow_missing_or_failed' }
	| { type: 'allow_missing' };

// JWT Requirement Rule: Maps route matches to requirements
export interface JwtRequirementRuleConfig {
	match?: {
		prefix?: string;
		path?: string;
		safe_regex?: { regex: string };
	};
	requires?: JwtRequirementConfig;
	requirement_name?: string;
}

// JWT Authentication full configuration
export interface JwtAuthenticationFilterConfig {
	providers: Record<string, JwtProviderConfig>;
	rules?: JwtRequirementRuleConfig[];
	requirement_map?: Record<string, JwtRequirementConfig>;
	bypass_cors_preflight?: boolean;
	filter_state_rules?: {
		name?: string;
		requires?: Record<string, JwtRequirementConfig>;
	};
}

// ============================================================================
// Local Rate Limit Configuration Types
// ============================================================================

// Token bucket configuration for rate limiting
export interface TokenBucketConfig {
	max_tokens: number;
	tokens_per_fill?: number;
	fill_interval_ms: number;
}

// Fractional percent denominator options
export type FractionalPercentDenominator = 'hundred' | 'ten_thousand' | 'million';

// Runtime fractional percent configuration for enabling/enforcing rate limits
export interface RuntimeFractionalPercentConfig {
	runtime_key?: string;
	numerator: number;
	denominator?: FractionalPercentDenominator;
}

// Local Rate Limit filter configuration
export interface LocalRateLimitConfig {
	stat_prefix: string;
	token_bucket?: TokenBucketConfig;
	status_code?: number;
	filter_enabled?: RuntimeFractionalPercentConfig;
	filter_enforced?: RuntimeFractionalPercentConfig;
	per_downstream_connection?: boolean;
	rate_limited_as_resource_exhausted?: boolean;
	max_dynamic_descriptors?: number;
	always_consume_default_token_bucket?: boolean;
}

// ============================================================================
// Custom Response Filter Types
// ============================================================================

// Status code matcher for custom response rules
export type StatusCodeMatcher =
	| { type: 'exact'; code: number }
	| { type: 'range'; min: number; max: number }
	| { type: 'list'; codes: number[] };

// Local response policy for custom responses
export interface LocalResponsePolicy {
	status_code?: number;
	body?: string;
	headers?: Record<string, string>;
}

// Custom response matcher rule
export interface ResponseMatcherRule {
	status_code: StatusCodeMatcher;
	response: LocalResponsePolicy;
}

// Custom response filter configuration
export interface CustomResponseConfig {
	matchers: ResponseMatcherRule[];
}

// ============================================================================
// MCP Filter Types
// ============================================================================

// Traffic mode for MCP filter
export type McpTrafficMode = 'pass_through' | 'reject_no_mcp';

// MCP filter configuration
export interface McpFilterConfig {
	traffic_mode: McpTrafficMode;
}

// ============================================================================
// FilterConfig Union Type
// ============================================================================

// FilterConfig uses tagged enum format: { type: '...', config: {...} }
// This matches the Rust #[serde(tag = "type", content = "config")] serialization
export type FilterConfig =
	| { type: 'header_mutation'; config: HeaderMutationFilterConfig }
	| { type: 'jwt_auth'; config: JwtAuthenticationFilterConfig }
	| { type: 'local_rate_limit'; config: LocalRateLimitConfig }
	| { type: 'custom_response'; config: CustomResponseConfig }
	| { type: 'mcp'; config: McpFilterConfig };

// Backend uses snake_case field names for HeaderMutationFilterConfig
export interface HeaderMutationFilterConfig {
	request_headers_to_add?: HeaderMutationEntry[];
	request_headers_to_remove?: string[];
	response_headers_to_add?: HeaderMutationEntry[];
	response_headers_to_remove?: string[];
}

export interface FilterResponse {
	id: string;
	name: string;
	filterType: string;
	description: string | null;
	config: FilterConfig;
	version: number;
	source: string;
	team: string;
	createdAt: string;
	updatedAt: string;
	allowedAttachmentPoints: AttachmentPoint[];
	/** Number of resources this filter is attached to (optional, may not be returned by all endpoints) */
	attachmentCount?: number;
}

export interface CreateFilterRequest {
	name: string;
	filterType: FilterType;
	description?: string;
	config: FilterConfig;
	team: string;
}

export interface UpdateFilterRequest {
	name?: string;
	description?: string;
	config?: FilterConfig;
}

export interface RouteFiltersResponse {
	routeId: string;
	filters: FilterResponse[];
}

export interface ListenerFiltersResponse {
	listenerId: string;
	filters: FilterResponse[];
}

// ============================================================================
// Route Hierarchy Types (Virtual Hosts and Routes)
// ============================================================================

// Virtual host summary as returned by the API
export interface VirtualHostSummary {
	id: string;
	name: string;
	domains: string[];
	ruleOrder: number;
	routeCount: number;
	filterCount: number;
}

// Route (individual route rule) summary as returned by the API
export interface RouteSummary {
	id: string;
	name: string;
	pathPattern: string;
	matchType: 'prefix' | 'exact' | 'regex' | 'path_template' | 'connect_matcher';
	ruleOrder: number;
	filterCount: number;
}

// Virtual host filters response
export interface VirtualHostFiltersResponse {
	virtualHostId: string;
	virtualHostName: string;
	filters: FilterResponse[];
}

// Route filters response (individual route within virtual host)
export interface RouteHierarchyFiltersResponse {
	routeId: string;
	routeName: string;
	filters: FilterResponse[];
}

// Hierarchy filter summary for displaying inherited filters
export interface HierarchyFilterSummary {
	routeConfigFilters: FilterResponse[];
	virtualHostFilters: FilterResponse[];
	routeFilters: FilterResponse[];
}

// Effective filters after applying inheritance/override rules
export interface EffectiveFilter {
	filter: FilterResponse;
	source: 'route_config' | 'virtual_host' | 'route';
	isOverridden: boolean;
	overriddenBy?: 'virtual_host' | 'route';
}

// Hierarchical filter attachment context - used by FilterSelectorModal
export type HierarchyLevel = 'route_config' | 'virtual_host' | 'route';

export interface HierarchicalFilterContext {
	level: HierarchyLevel;
	routeConfigName: string;
	virtualHostName?: string;
	routeName?: string;
}

// Attached filter with source information for display
export interface AttachedFilterWithSource {
	filter: FilterResponse;
	level: HierarchyLevel;
	order: number;
}

// ============================================================================
// mTLS and Proxy Certificate Types
// ============================================================================

// mTLS status response
export interface MtlsStatusResponse {
	/** Whether mTLS is fully enabled (PKI configured + xDS TLS configured) */
	enabled: boolean;
	/** Whether the xDS server has TLS enabled (server certificate configured) */
	xdsServerTls: boolean;
	/** Whether client certificate authentication is required */
	clientAuthRequired: boolean;
	/** SPIFFE trust domain for certificate identity URIs */
	trustDomain: string;
	/** Whether Vault PKI mount is configured for certificate generation */
	pkiMountConfigured: boolean;
	/** Message describing the current mTLS status */
	message: string;
}

// Request to generate a proxy certificate
export interface GenerateCertificateRequest {
	/** Unique identifier for the proxy instance (e.g., hostname, pod name) */
	proxyId: string;
}

// Response after generating a certificate
export interface GenerateCertificateResponse {
	/** Certificate record ID */
	id: string;
	/** Proxy instance identifier */
	proxyId: string;
	/** SPIFFE URI embedded in the certificate */
	spiffeUri: string;
	/** PEM-encoded X.509 certificate */
	certificate: string;
	/** PEM-encoded private key (only returned once at generation time) */
	privateKey: string;
	/** PEM-encoded CA certificate chain */
	caChain: string;
	/** Certificate expiration timestamp (ISO 8601) */
	expiresAt: string;
}

// Certificate metadata (without private key)
export interface CertificateMetadata {
	id: string;
	proxyId: string;
	spiffeUri: string;
	serialNumber: string;
	issuedAt: string;
	expiresAt: string;
	isValid: boolean;
	isExpired: boolean;
	isRevoked: boolean;
	revokedAt: string | null;
	revokedReason: string | null;
}

// Response for listing certificates
export interface ListCertificatesResponse {
	/** List of certificates (without private keys) */
	certificates: CertificateMetadata[];
	/** Total number of certificates for this team */
	total: number;
	/** Pagination limit used */
	limit: number;
	/** Pagination offset used */
	offset: number;
}

// Query parameters for listing certificates
export interface ListCertificatesQuery {
	/** Maximum number of certificates to return */
	limit?: number;
	/** Offset for pagination */
	offset?: number;
}

// Bootstrap configuration request with mTLS options
export interface BootstrapConfigRequestWithMtls extends BootstrapConfigRequest {
	/** Enable mTLS configuration in bootstrap */
	mtls?: boolean;
	/** Path to client certificate file */
	certPath?: string;
	/** Path to client private key file */
	keyPath?: string;
	/** Path to CA certificate file */
	caPath?: string;
}

// ============================================================================
// Dynamic Filter Types API
// ============================================================================

/** UI hints for filter form generation */
export interface FilterTypeUiHints {
	/** Form layout style */
	formLayout: 'flat' | 'sections' | 'tabs';
	/** Form sections for grouped fields */
	sections: FilterTypeFormSection[];
	/** Custom form component name (if using a custom form) */
	customFormComponent?: string;
}

/** A section in a form layout */
export interface FilterTypeFormSection {
	/** Section name/title */
	name: string;
	/** Field names included in this section */
	fields: string[];
	/** Whether the section is collapsible */
	collapsible: boolean;
	/** Whether the section is collapsed by default */
	collapsedByDefault: boolean;
}

/** Information about a filter type available in the system */
export interface FilterTypeInfo {
	/** Unique filter type name (e.g., "header_mutation") */
	name: string;
	/** Human-readable display name (e.g., "Header Mutation") */
	displayName: string;
	/** Description of what this filter does */
	description: string;
	/** Schema version */
	version: string;
	/** Envoy HTTP filter name */
	envoyFilterName: string;
	/** Valid attachment points for this filter */
	attachmentPoints: AttachmentPoint[];
	/** Whether this filter requires listener-level configuration */
	requiresListenerConfig: boolean;
	/** How this filter handles per-route configuration */
	perRouteBehavior: 'full_config' | 'reference_only' | 'disable_only' | 'not_supported';
	/** Whether this filter type is fully implemented */
	isImplemented: boolean;
	/** Source of this filter definition (built_in or custom) */
	source: 'built_in' | 'custom';
	/** JSON Schema for configuration validation */
	configSchema: JSONSchema7;
	/** UI hints for form generation (if available) */
	uiHints?: FilterTypeUiHints;
}

/** Response for listing all filter types */
export interface FilterTypesResponse {
	/** List of available filter types */
	filterTypes: FilterTypeInfo[];
	/** Total count of filter types */
	total: number;
	/** Count of implemented filter types */
	implementedCount: number;
}

// JSON Schema type (simplified for our use case)
export interface JSONSchema7 {
	type?: string | string[];
	properties?: Record<string, JSONSchema7>;
	required?: string[];
	items?: JSONSchema7;
	enum?: (string | number | boolean | null)[];
	const?: unknown;
	default?: unknown;
	title?: string;
	description?: string;
	minimum?: number;
	maximum?: number;
	minLength?: number;
	maxLength?: number;
	pattern?: string;
	format?: string;
	oneOf?: JSONSchema7[];
	anyOf?: JSONSchema7[];
	allOf?: JSONSchema7[];
	$ref?: string;
	additionalProperties?: boolean | JSONSchema7;
}

// ============================================================================
// Stats Dashboard Types
// ============================================================================

/** Response for checking if stats dashboard is enabled */
export interface StatsEnabledResponse {
	enabled: boolean;
}

/** Overview stats for team dashboard */
export interface StatsOverviewResponse {
	/** Team name */
	team: string;
	/** Total requests per second */
	totalRps: number;
	/** Total active connections */
	totalConnections: number;
	/** Error rate (0.0 - 1.0) */
	errorRate: number;
	/** P99 latency in milliseconds */
	p99LatencyMs: number;
	/** Number of healthy clusters */
	healthyClusters: number;
	/** Number of degraded clusters */
	degradedClusters: number;
	/** Number of unhealthy clusters */
	unhealthyClusters: number;
	/** Total clusters */
	totalClusters: number;
	/** Overall health status (healthy, degraded, unhealthy) */
	healthStatus: 'healthy' | 'degraded' | 'unhealthy';
	/** When this data was collected (ISO 8601) */
	timestamp: string;
}

/** Single cluster stats */
export interface ClusterStatsResponse {
	/** Cluster name */
	clusterName: string;
	/** Health status */
	healthStatus: 'healthy' | 'degraded' | 'unhealthy';
	/** Number of healthy hosts */
	healthyHosts: number;
	/** Total hosts */
	totalHosts: number;
	/** Active connections */
	activeConnections: number;
	/** Active requests */
	activeRequests: number;
	/** Pending requests */
	pendingRequests: number;
	/** Success rate (0.0 - 1.0), null if no data */
	successRate: number | null;
	/** Circuit breaker is open */
	circuitBreakerOpen: boolean;
	/** Number of outlier ejections */
	outlierEjections: number;
}

/** Response for cluster stats list */
export interface ClustersStatsResponse {
	/** Team name */
	team: string;
	/** Cluster stats */
	clusters: ClusterStatsResponse[];
	/** Total count */
	count: number;
}

/** App status (for admin app management) */
export interface AppStatusResponse {
	/** App ID (e.g., "stats_dashboard") */
	appId: string;
	/** Whether the app is enabled */
	enabled: boolean;
	/** App configuration */
	config: Record<string, unknown> | null;
	/** Who enabled/disabled the app */
	enabledBy: string | null;
	/** When the app was enabled (ISO 8601) */
	enabledAt: string | null;
}

/** Request to set app status */
export interface SetAppStatusRequest {
	enabled: boolean;
	config?: Record<string, unknown>;
}

// ============================================================================
// Secret Management Types
// ============================================================================

/** Secret type enumeration matching backend */
export type SecretType = 'generic_secret' | 'tls_certificate' | 'certificate_validation_context' | 'session_ticket_keys';

/** Secret backend for external references */
export type SecretBackend = 'vault' | 'aws_secrets_manager' | 'gcp_secret_manager';

/** Secret response (metadata only, no secret values) */
export interface SecretResponse {
	/** Unique identifier */
	id: string;
	/** Name of the secret */
	name: string;
	/** Type of the secret */
	secret_type: SecretType;
	/** Optional description */
	description: string | null;
	/** Version number (incremented on updates) */
	version: number;
	/** Source of the secret (ui, api, import) */
	source: string;
	/** Team that owns this secret */
	team: string;
	/** Creation timestamp (ISO 8601) */
	created_at: string;
	/** Last update timestamp (ISO 8601) */
	updated_at: string;
	/** Expiration timestamp (if set) */
	expires_at: string | null;
	/** Backend type for reference-based secrets */
	backend?: SecretBackend;
	/** Backend-specific reference (Vault path, AWS ARN, etc.) */
	reference?: string;
	/** Optional version specifier for the external secret */
	reference_version?: string;
}

/** Request to create a new secret (direct storage) */
export interface CreateSecretRequest {
	/** Name of the secret (must be unique within the team) */
	name: string;
	/** Type of the secret */
	secret_type: SecretType;
	/** Optional description */
	description?: string;
	/** Secret configuration (varies by type) */
	configuration: Record<string, unknown>;
	/** Optional expiration time (ISO 8601) */
	expires_at?: string;
}

/** Request to create a reference-based secret (external backend) */
export interface CreateSecretReferenceRequest {
	/** Name of the secret (must be unique within the team) */
	name: string;
	/** Type of the secret */
	secret_type: SecretType;
	/** Optional description */
	description?: string;
	/** Backend type: "vault", "aws_secrets_manager", "gcp_secret_manager" */
	backend: SecretBackend;
	/** Backend-specific reference (Vault path, AWS ARN, GCP resource name) */
	reference: string;
	/** Optional version specifier for the external secret */
	reference_version?: string;
	/** Optional expiration time (ISO 8601) */
	expires_at?: string;
}

/** Request to update an existing secret */
export interface UpdateSecretRequest {
	/** Optional description update */
	description?: string;
	/** New secret configuration (replaces existing) */
	configuration?: Record<string, unknown>;
	/** Optional expiration time update */
	expires_at?: string | null;
}

/** Query parameters for listing secrets */
export interface ListSecretsQuery {
	/** Maximum number of secrets to return */
	limit?: number;
	/** Offset for pagination */
	offset?: number;
	/** Filter by secret type */
	secret_type?: SecretType;
}

// ============================================================================
// Filter Install/Configure Types (Filter Install/Configure Redesign)
// ============================================================================

/** Scope type for filter configuration */
export type ScopeType = 'route-config' | 'virtual-host' | 'route';

/** Request to install a filter on a listener */
export interface InstallFilterRequest {
	/** Listener name */
	listenerName: string;
	/** Optional execution order */
	order?: number;
}

/** Response after installing a filter */
export interface InstallFilterResponse {
	filterId: string;
	listenerId: string;
	listenerName: string;
	order: number;
}

/** Single installation item in list response */
export interface FilterInstallationItem {
	listenerId: string;
	listenerName: string;
	listenerAddress: string;
	order: number;
}

/** Response for listing filter installations */
export interface FilterInstallationsResponse {
	filterId: string;
	filterName: string;
	installations: FilterInstallationItem[];
}

/** Behavior for per-route filter settings */
export type FilterConfigBehavior = 'use_base' | 'disable' | 'override';

/** Per-route settings structure */
export interface PerRouteSettings {
	/** How the filter should behave at this scope */
	behavior: FilterConfigBehavior;
	/** Override config - only used when behavior is 'override' */
	config?: Record<string, unknown>;
	/** For JWT: requirement name reference (reference_only behavior) */
	requirementName?: string;
}

/** Request to configure a filter scope */
export interface ConfigureFilterRequest {
	/** Type of scope: "route-config", "virtual-host", or "route" */
	scopeType: ScopeType;
	/** ID or name of the scope resource */
	scopeId: string;
	/** Optional per-route/vhost settings */
	settings?: PerRouteSettings;
}

/** Response after configuring a filter */
export interface ConfigureFilterResponse {
	filterId: string;
	scopeType: ScopeType;
	scopeId: string;
	scopeName: string;
	settings?: Record<string, unknown>;
}

/** Single configuration item in list response */
export interface FilterConfigurationItem {
	scopeType: ScopeType;
	scopeId: string;
	scopeName: string;
	settings?: Record<string, unknown>;
}

/** Response for listing filter configurations */
export interface FilterConfigurationsResponse {
	filterId: string;
	filterName: string;
	configurations: FilterConfigurationItem[];
}

/** Combined filter status with installations and configurations */
export interface FilterStatusResponse {
	filterId: string;
	filterName: string;
	filterType: string;
	description: string | null;
	installations: FilterInstallationItem[];
	configurations: FilterConfigurationItem[];
}

// ============================================================================
// Learning Session Types
// ============================================================================

/** Learning session status */
export type LearningSessionStatus =
	| 'pending'
	| 'active'
	| 'completing'
	| 'completed'
	| 'cancelled'
	| 'failed';

/** Learning session response from API */
export interface LearningSessionResponse {
	id: string;
	team: string;
	routePattern: string;
	clusterName: string | null;
	httpMethods: string[] | null;
	status: string;
	createdAt: string;
	startedAt: string | null;
	endsAt: string | null;
	completedAt: string | null;
	targetSampleCount: number;
	currentSampleCount: number;
	progressPercentage: number;
	triggeredBy: string | null;
	deploymentVersion: string | null;
	errorMessage: string | null;
}

/** Request to create a learning session */
export interface CreateLearningSessionRequest {
	routePattern: string;
	clusterName?: string;
	httpMethods?: string[];
	targetSampleCount: number;
	maxDurationSeconds?: number;
	triggeredBy?: string;
	deploymentVersion?: string;
	configurationSnapshot?: Record<string, unknown>;
}

/** Query parameters for listing learning sessions */
export interface ListLearningSessionsQuery {
	status?: LearningSessionStatus;
	limit?: number;
	offset?: number;
}

// ============================================================================
// Aggregated Schema Types
// ============================================================================

/** Breaking change information for schema versioning */
export interface BreakingChange {
	changeType: string;
	path: string;
	description: string;
	severity: 'warning' | 'error';
}

/** Aggregated schema response from API */
export interface AggregatedSchemaResponse {
	id: number;
	team: string;
	path: string;
	httpMethod: string;
	version: number;
	previousVersionId: number | null;
	requestSchema: Record<string, unknown> | null;
	responseSchemas: Record<string, unknown> | null;
	sampleCount: number;
	confidenceScore: number;
	breakingChanges: BreakingChange[] | null;
	firstObserved: string;
	lastObserved: string;
	createdAt: string;
	updatedAt: string;
}

/** Query parameters for listing aggregated schemas */
export interface ListAggregatedSchemasQuery {
	path?: string;
	httpMethod?: string;
	minConfidence?: number;
	limit?: number;
	offset?: number;
}

/** Schema comparison response */
export interface SchemaComparisonResponse {
	currentSchema: AggregatedSchemaResponse;
	comparedSchema: AggregatedSchemaResponse;
	differences: SchemaDifferences;
}

/** Differences between schema versions */
export interface SchemaDifferences {
	versionChange: number;
	sampleCountChange: number;
	confidenceChange: number;
	hasBreakingChanges: boolean;
	breakingChanges: BreakingChange[] | null;
}

/** OpenAPI export response */
export interface OpenApiExportResponse {
	openapi: string;
	info: {
		title: string;
		version: string;
		description: string | null;
	};
	paths: Record<string, unknown>;
	components: Record<string, unknown>;
}

/** Request to export multiple schemas as unified OpenAPI */
export interface ExportMultipleSchemasRequest {
	schemaIds: number[];
	title: string;
	version: string;
	description?: string;
	includeMetadata: boolean;
}

// ============================================================================
// Custom WASM Filter Types (Plugin Management)
// ============================================================================

/** Response for a custom WASM filter */
export interface CustomWasmFilterResponse {
	id: string;
	name: string;
	display_name: string;
	description: string | null;
	wasm_sha256: string;
	wasm_size_bytes: number;
	config_schema: Record<string, unknown>;
	per_route_config_schema: Record<string, unknown> | null;
	ui_hints: Record<string, unknown> | null;
	attachment_points: string[];
	runtime: string;
	failure_policy: string;
	version: number;
	team: string;
	created_by: string | null;
	created_at: string;
	updated_at: string;
	filter_type: string;
}

/** Request to create a custom WASM filter */
export interface CreateCustomWasmFilterRequest {
	name: string;
	display_name: string;
	description?: string;
	wasm_binary_base64: string;
	config_schema: Record<string, unknown>;
	per_route_config_schema?: Record<string, unknown>;
	ui_hints?: Record<string, unknown>;
	attachment_points?: string[];
	runtime?: string;
	failure_policy?: string;
}

/** Request to update a custom WASM filter (metadata only, not binary) */
export interface UpdateCustomWasmFilterRequest {
	display_name?: string;
	description?: string;
	config_schema?: Record<string, unknown>;
	per_route_config_schema?: Record<string, unknown>;
	ui_hints?: Record<string, unknown>;
	attachment_points?: string[];
}

/** Response for listing custom WASM filters */
export interface ListCustomWasmFiltersResponse {
	items: CustomWasmFilterResponse[];
	total: number;
}
