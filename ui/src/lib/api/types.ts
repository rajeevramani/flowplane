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

// Filter type uses snake_case to match backend serde serialization
export type FilterType = 'header_mutation' | 'jwt_auth' | 'cors' | 'rate_limit' | 'ext_authz';

// FilterConfig uses tagged enum format: { type: '...', config: {...} }
// This matches the Rust #[serde(tag = "type", content = "config")] serialization
export type FilterConfig = {
	type: 'header_mutation';
	config: HeaderMutationFilterConfig;
};

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

export interface AttachFilterRequest {
	filterId: string;
	order?: number;
}

export interface RouteFiltersResponse {
	routeId: string;
	filters: FilterResponse[];
}
