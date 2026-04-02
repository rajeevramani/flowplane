// API client with OIDC JWT authentication
import { goto } from '$app/navigation';
import { env } from '$env/dynamic/public';
import { z } from 'zod';
import { SecretResponseSchema, AdminListOrgsResponseSchema, AdminResourceSummarySchema, paginatedSchema } from './schemas';
import { ClusterResponseSchema } from '$lib/schemas/cluster';
import { getUserManager } from '$lib/auth/oidc-config';
import type {
	BootstrapStatusResponse,
	BootstrapInitializeRequest,
	BootstrapInitializeResponse,
	SessionInfoResponse,
	DashboardStats,
	ApiError,
	ImportOpenApiRequest,
	ImportResponse,
	ImportSummary,
	ImportDetailsResponse,
	ListenerResponse,
	RouteResponse,
	ClusterResponse,
	EnvoyConfigRequest,
	EnvoyConfigRequestWithMtls,
	ListTeamsResponse,
	TeamResponse,
	CreateTeamRequest,
	UpdateTeamRequest,
	AdminListTeamsResponse,
	AuditLogEntry,
	ListAuditLogsQuery,
	ListAuditLogsResponse,
	CreateClusterBody,
	CreateRouteBody,
	UpdateRouteBody,
	CreateListenerBody,
	UpdateListenerBody,
	ListScopesResponse,
	FilterResponse,
	CreateFilterRequest,
	UpdateFilterRequest,
	RouteFiltersResponse,
	ListenerFiltersResponse,
	VirtualHostSummary,
	RouteSummary,
	VirtualHostFiltersResponse,
	RouteHierarchyFiltersResponse,
	MtlsStatusResponse,
	GenerateCertificateRequest,
	GenerateCertificateResponse,
	CertificateMetadata,
	ListCertificatesResponse,
	ListCertificatesQuery,
	FilterTypesResponse,
	FilterTypeInfo,
	StatsEnabledResponse,
	StatsOverviewResponse,
	ClustersStatsResponse,
	ClusterStatsResponse,
	AppStatusResponse,
	SetAppStatusRequest,
	SecretResponse,
	CreateSecretRequest,
	CreateSecretReferenceRequest,
	UpdateSecretRequest,
	RotateSecretRequest,
	ListSecretsQuery,
	// Filter Install/Configure types
	InstallFilterRequest,
	InstallFilterResponse,
	FilterInstallationsResponse,
	ConfigureFilterRequest,
	ConfigureFilterResponse,
	FilterConfigurationsResponse,
	FilterStatusResponse,
	// Learning Session types
	LearningSessionResponse,
	CreateLearningSessionRequest,
	ListLearningSessionsQuery,
	// Aggregated Schema types
	AggregatedSchemaResponse,
	ListAggregatedSchemasQuery,
	SchemaComparisonResponse,
	OpenApiExportResponse,
	ExportMultipleSchemasRequest,
	// Custom WASM Filter types
	CustomWasmFilterResponse,
	CreateCustomWasmFilterRequest,
	UpdateCustomWasmFilterRequest,
	ListCustomWasmFiltersResponse,
	// MCP types
	ListMcpToolsResponse,
	ListMcpToolsQuery,
	McpTool,
	UpdateMcpToolRequest,
	McpStatus,
	EnableMcpRequest,
	EnableMcpResponse,
	McpOperationResponse,
	BulkMcpRequest,
	BulkMcpResponse,
	LearnedSchemaAvailability,
	ApplyLearnedSchemaRequest,
	ApplyLearnedSchemaResponse,
	// Expose types
	ExposeRequest,
	ExposeResponse,
	// Dataplane types
	DataplaneResponse,
	CreateDataplaneBody,
	UpdateDataplaneBody,
	// Organization types
	OrganizationResponse,
	OrgMembershipResponse,
	CreateOrganizationRequest,
	UpdateOrganizationRequest,
	AddOrgMemberRequest,
	AdminListOrgsResponse,
	CurrentOrgResponse,
	ListOrgTeamsResponse,
	OrgTeamMemberResponse,
	ListOrgTeamMembersResponse,
	AddOrgTeamMemberRequest,
	OrgRole,
	// Agent types
	AgentInfo,
	ListAgentsResponse,
	CreateAgentRequest,
	CreateAgentResponse,
	CreateGrantRequest,
	GrantResponse,
	GrantListResponse,
	GrantSummary,
	// Invite types
	InviteOrgMemberRequest,
	InviteOrgMemberResponse,
	PaginatedResponse,
	// Admin Summary types
	AdminResourceSummary
} from './types';
import { currentOrg } from '$lib/stores/org';

const API_BASE = env.PUBLIC_API_BASE || 'http://localhost:8080';

function parseResponse<T>(data: unknown, schema: z.ZodType<T>): T {
	const result = schema.safeParse(data);
	if (!result.success) {
		console.warn('API response validation failed:', result.error.issues);
		return data as T;
	}
	return result.data;
}

class ApiClient {
	private async getHeaders(): Promise<HeadersInit> {
		const headers: Record<string, string> = {
			'Content-Type': 'application/json',
		};

		const userManager = await getUserManager();
		const user = await userManager.getUser();
		if (user?.access_token) {
			headers['Authorization'] = `Bearer ${user.access_token}`;
		}

		return headers;
	}

	private async handleResponse<T>(response: Response): Promise<T> {
		if (!response.ok) {
			// Handle 401 Unauthorized - redirect to login
			if (response.status === 401) {
				this.clearAuth();
				goto('/login');
				throw new Error('Unauthorized - redirecting to login');
			}

			// Try to parse error response
			let errorMessage = `HTTP ${response.status}: ${response.statusText}`;
			try {
				const errorData: ApiError = await response.json();
				errorMessage = errorData.message || errorMessage;
			} catch {
				// If JSON parsing fails, use status text
			}

			throw new Error(errorMessage);
		}

		// Handle empty responses (like logout)
		const contentType = response.headers.get('content-type');
		if (!contentType || !contentType.includes('application/json')) {
			return {} as T;
		}

		return response.json();
	}

	async login(): Promise<void> {
		const userManager = await getUserManager();
		await userManager.signinRedirect();
	}

	async logout(): Promise<void> {
		try {
			const userManager = await getUserManager();
			await userManager.signoutRedirect();
		} finally {
			this.clearAuth();
		}
	}

	async getSessionInfo(): Promise<SessionInfoResponse> {
		const userManager = await getUserManager();
		const user = await userManager.getUser();
		if (!user || user.expired) {
			throw new Error('Not authenticated');
		}

		// Fetch DB-sourced permissions from backend (Auth v3: JWT is identity-only,
		// all permissions come from Flowplane DB, not Zitadel role claims).
		const backendSession = await this.get<{
			userId: string;
			email: string;
			name: string;
			isAdmin: boolean;
			isPlatformAdmin: boolean;
			orgScopes: string[];
			grants: GrantSummary[];
			teams: string[];
			orgId?: string;
			orgName?: string;
			orgRole?: string;
		}>('/api/v1/auth/session');

		return {
			userId: backendSession.userId,
			name: backendSession.name || (user.profile.name as string) || '',
			email: backendSession.email || (user.profile.email as string) || '',
			isPlatformAdmin: backendSession.isPlatformAdmin,
			teams: backendSession.teams,
			orgScopes: backendSession.orgScopes,
			grants: backendSession.grants,
			expiresAt: user.expires_at ? new Date(user.expires_at * 1000).toISOString() : null,
			orgId: backendSession.orgId,
			orgName: backendSession.orgName,
			orgRole: backendSession.orgRole,
		};
	}

	clearAuth() {
		getUserManager().then((um) => um.removeUser());
		// Clear org context to prevent session leaking across logins
		currentOrg.set({ organization: null, role: null });
	}

	// Generic methods for authenticated requests
	async get<T>(path: string): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'GET',
			headers: await this.getHeaders(),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async post<T>(path: string, body: unknown): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'POST',
			headers: await this.getHeaders(),
			body: JSON.stringify(body),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async put<T>(path: string, body: unknown): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'PUT',
			headers: await this.getHeaders(),
			body: JSON.stringify(body),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async delete<T>(path: string): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'DELETE',
			headers: await this.getHeaders(),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async patch<T>(path: string, body: unknown): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'PATCH',
			headers: await this.getHeaders(),
			body: JSON.stringify(body),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	// Bootstrap methods
	async getBootstrapStatus(): Promise<BootstrapStatusResponse> {
		const response = await fetch(`${API_BASE}/api/v1/bootstrap/status`, {
			method: 'GET',
			headers: { 'Content-Type': 'application/json' },
		});

		return this.handleResponse<BootstrapStatusResponse>(response);
	}

	async bootstrapInitialize(
		data: BootstrapInitializeRequest
	): Promise<BootstrapInitializeResponse> {
		const response = await fetch(`${API_BASE}/api/v1/bootstrap/initialize`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(data),
		});

		return this.handleResponse<BootstrapInitializeResponse>(response);
	}

	// OpenAPI import
	async importOpenApiSpec(request: ImportOpenApiRequest): Promise<ImportResponse> {
		const params = new URLSearchParams();
		params.append('listener_mode', request.listenerMode);
		if (request.listenerMode === 'existing' && request.existingListenerName) {
			params.append('existing_listener_name', request.existingListenerName);
		}
		if (request.listenerMode === 'new') {
			if (request.newListenerName) params.append('new_listener_name', request.newListenerName);
			if (request.newListenerAddress) params.append('new_listener_address', request.newListenerAddress);
			if (request.newListenerPort) params.append('new_listener_port', request.newListenerPort.toString());
		}

		const path = `/api/v1/teams/${encodeURIComponent(request.team)}/openapi/import${params.toString() ? `?${params.toString()}` : ''}`;

		// Determine content type based on spec format
		const isYaml = request.spec.trim().startsWith('openapi:') || request.spec.trim().startsWith('swagger:');
		const contentType = isYaml ? 'application/yaml' : 'application/json';

		const baseHeaders = await this.getHeaders();
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'POST',
			headers: {
				...baseHeaders,
				'Content-Type': contentType
			},
			body: request.spec,
			credentials: 'include'
		});

		return this.handleResponse<ImportResponse>(response);
	}

	// Import methods (replacing API Definition methods)
	async listImports(team: string): Promise<ImportSummary[]> {
		const path = `/api/v1/teams/${encodeURIComponent(team)}/openapi/imports`;
		const response = await this.get<{ imports: ImportSummary[] }>(path);
		return response.imports;
	}

	// List all imports across all teams (admin only)
	// Note: use listImports(team) for team-scoped access
	async listAllImports(team: string): Promise<ImportSummary[]> {
		const path = `/api/v1/teams/${encodeURIComponent(team)}/openapi/imports`;
		const response = await this.get<{ imports: ImportSummary[] }>(path);
		return response.imports;
	}

	async getImport(id: string): Promise<ImportDetailsResponse> {
		return this.get<ImportDetailsResponse>(`/api/v1/openapi/imports/${id}`);
	}

	async deleteImport(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/openapi/imports/${id}`);
	}

	// Listener methods
	async listListeners(team: string, params?: { limit?: number; offset?: number }): Promise<ListenerResponse[]> {
		let path = `/api/v1/teams/${encodeURIComponent(team)}/listeners`;
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		const response = await this.get<PaginatedResponse<ListenerResponse>>(path);
		return response.items;
	}

	async getListener(team: string, name: string): Promise<ListenerResponse> {
		return this.get<ListenerResponse>(`/api/v1/teams/${encodeURIComponent(team)}/listeners/${encodeURIComponent(name)}`);
	}

	async deleteListener(team: string, name: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/listeners/${encodeURIComponent(name)}`);
	}

	// Route Config methods
	async listRouteConfigs(team: string, params?: { limit?: number; offset?: number }): Promise<RouteResponse[]> {
		let path = `/api/v1/teams/${encodeURIComponent(team)}/route-configs`;
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		const response = await this.get<PaginatedResponse<RouteResponse>>(path);
		return response.items;
	}

	async getRouteConfig(team: string, name: string): Promise<RouteResponse> {
		return this.get<RouteResponse>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(name)}`);
	}

	async deleteRouteConfig(team: string, name: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(name)}`);
	}

	async updateRouteConfig(team: string, name: string, body: UpdateRouteBody): Promise<RouteResponse> {
		return this.put<RouteResponse>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(name)}`, body);
	}

	// Cluster methods
	async listClusters(team: string, params?: { limit?: number; offset?: number }): Promise<ClusterResponse[]> {
		let path = `/api/v1/teams/${encodeURIComponent(team)}/clusters`;
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		const response = await this.get<PaginatedResponse<ClusterResponse>>(path);
		const validated = parseResponse(response, paginatedSchema(ClusterResponseSchema));
		return (validated as PaginatedResponse<ClusterResponse>).items;
	}

	async getCluster(team: string, name: string): Promise<ClusterResponse> {
		const data = await this.get<ClusterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/clusters/${encodeURIComponent(name)}`);
		return parseResponse(data, ClusterResponseSchema) as ClusterResponse;
	}

	async deleteCluster(team: string, name: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/clusters/${encodeURIComponent(name)}`);
	}

	async createCluster(team: string, body: CreateClusterBody): Promise<ClusterResponse> {
		return this.post<ClusterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/clusters`, body);
	}

	async updateCluster(team: string, name: string, body: CreateClusterBody): Promise<ClusterResponse> {
		return this.put<ClusterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/clusters/${encodeURIComponent(name)}`, body);
	}

	async createRouteConfig(team: string, body: CreateRouteBody): Promise<RouteResponse> {
		return this.post<RouteResponse>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs`, body);
	}

	async createListener(team: string, body: CreateListenerBody): Promise<ListenerResponse> {
		return this.post<ListenerResponse>(`/api/v1/teams/${encodeURIComponent(team)}/listeners`, body);
	}

	async updateListener(team: string, name: string, body: UpdateListenerBody): Promise<ListenerResponse> {
		return this.put<ListenerResponse>(`/api/v1/teams/${encodeURIComponent(team)}/listeners/${encodeURIComponent(name)}`, body);
	}

	// Team methods
	async listTeams(): Promise<ListTeamsResponse> {
		return this.get<ListTeamsResponse>('/api/v1/teams');
	}

	// Admin Team Management methods
	async adminListTeams(limit: number = 50, offset: number = 0): Promise<AdminListTeamsResponse> {
		const params = new URLSearchParams();
		params.append('limit', limit.toString());
		params.append('offset', offset.toString());

		return this.get<AdminListTeamsResponse>(`/api/v1/admin/teams?${params.toString()}`);
	}

	async adminGetTeam(id: string): Promise<TeamResponse> {
		return this.get<TeamResponse>(`/api/v1/admin/teams/${id}`);
	}

	async adminCreateTeam(request: CreateTeamRequest): Promise<TeamResponse> {
		return this.post<TeamResponse>('/api/v1/admin/teams', request);
	}

	async adminUpdateTeam(id: string, request: UpdateTeamRequest): Promise<TeamResponse> {
		return this.patch<TeamResponse>(`/api/v1/admin/teams/${id}`, request);
	}

	async adminDeleteTeam(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/admin/teams/${id}`);
	}

	// Audit Log methods (admin only)
	async listAuditLogs(query: ListAuditLogsQuery = {}): Promise<ListAuditLogsResponse> {
		const params = new URLSearchParams();

		if (query.resource_type) params.append('resource_type', query.resource_type);
		if (query.action) params.append('action', query.action);
		if (query.user_id) params.append('user_id', query.user_id);
		if (query.start_date) params.append('start_date', query.start_date);
		if (query.end_date) params.append('end_date', query.end_date);
		if (query.limit !== undefined) params.append('limit', query.limit.toString());
		if (query.offset !== undefined) params.append('offset', query.offset.toString());

		return this.get<ListAuditLogsResponse>(`/api/v1/audit-logs?${params.toString()}`);
	}

	// Scope methods (public - no auth required)
	async listScopes(): Promise<ListScopesResponse> {
		// This endpoint is public, no credentials needed
		const response = await fetch(`${API_BASE}/api/v1/scopes`, {
			method: 'GET',
			headers: { 'Content-Type': 'application/json' }
		});

		return this.handleResponse<ListScopesResponse>(response);
	}

	// Admin scope methods
	async listAllScopes(): Promise<ListScopesResponse> {
		return this.get<ListScopesResponse>('/api/v1/admin/scopes');
	}

	// Filter methods (team-scoped)
	async listFilters(team: string, params?: { limit?: number; offset?: number }): Promise<FilterResponse[]> {
		let path = `/api/v1/teams/${encodeURIComponent(team)}/filters`;
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		const response = await this.get<PaginatedResponse<FilterResponse>>(path);
		return response.items;
	}

	async getFilter(team: string, id: string): Promise<FilterResponse> {
		return this.get<FilterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/filters/${id}`);
	}

	async createFilter(team: string, body: CreateFilterRequest): Promise<FilterResponse> {
		return this.post<FilterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/filters`, body);
	}

	async updateFilter(team: string, id: string, body: UpdateFilterRequest): Promise<FilterResponse> {
		return this.patch<FilterResponse>(`/api/v1/teams/${encodeURIComponent(team)}/filters/${id}`, body);
	}

	async deleteFilter(team: string, id: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/filters/${id}`);
	}

	// Route Config Filter methods
	async listRouteConfigFilters(team: string, routeConfigName: string): Promise<RouteFiltersResponse> {
		return this.get<RouteFiltersResponse>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(routeConfigName)}/filters`);
	}

	// Listener-Filter methods
	async listListenerFilters(team: string, listenerName: string): Promise<ListenerFiltersResponse> {
		return this.get<ListenerFiltersResponse>(`/api/v1/teams/${encodeURIComponent(team)}/listeners/${encodeURIComponent(listenerName)}/filters`);
	}

	// ============================================================================
	// Route Hierarchy Methods (Virtual Hosts and Routes within RouteConfigs)
	// ============================================================================

	// List virtual hosts within a route config
	async listVirtualHosts(team: string, routeConfigName: string): Promise<VirtualHostSummary[]> {
		const response = await this.get<{ routeConfigName: string; virtualHosts: VirtualHostSummary[] }>(
			`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(routeConfigName)}/virtual-hosts`
		);
		return response.virtualHosts;
	}

	// List routes within a virtual host
	async listRoutesInVirtualHost(
		team: string,
		routeConfigName: string,
		virtualHostName: string
	): Promise<RouteSummary[]> {
		const response = await this.get<{
			routeConfigName: string;
			virtualHostName: string;
			routes: RouteSummary[];
		}>(`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(routeConfigName)}/virtual-hosts/${encodeURIComponent(virtualHostName)}/routes`);
		return response.routes;
	}

	// Virtual Host Filter methods
	async listVirtualHostFilters(
		team: string,
		routeConfigName: string,
		virtualHostName: string
	): Promise<VirtualHostFiltersResponse> {
		return this.get<VirtualHostFiltersResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(routeConfigName)}/virtual-hosts/${encodeURIComponent(virtualHostName)}/filters`
		);
	}

	// Route (within Virtual Host) Filter methods
	async listRouteHierarchyFilters(
		team: string,
		routeConfigName: string,
		virtualHostName: string,
		routeName: string
	): Promise<RouteHierarchyFiltersResponse> {
		return this.get<RouteHierarchyFiltersResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/route-configs/${encodeURIComponent(routeConfigName)}/virtual-hosts/${encodeURIComponent(virtualHostName)}/routes/${encodeURIComponent(routeName)}/filters`
		);
	}

	// ============================================================================
	// Expose / Unexpose Methods
	// ============================================================================

	async expose(team: string, body: ExposeRequest): Promise<ExposeResponse> {
		return this.post<ExposeResponse>(`/api/v1/teams/${encodeURIComponent(team)}/expose`, body);
	}

	async unexpose(team: string, name: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/expose/${encodeURIComponent(name)}`);
	}

	// ============================================================================
	// mTLS and Proxy Certificate Methods
	// ============================================================================

	/**
	 * Get mTLS configuration status for the control plane.
	 * This endpoint helps understand whether mTLS is enabled and properly configured.
	 */
	async getMtlsStatus(): Promise<MtlsStatusResponse> {
		return this.get<MtlsStatusResponse>('/api/v1/mtls/status');
	}

	/**
	 * Generate a new proxy certificate for mTLS authentication.
	 * The private key is only returned once at generation time.
	 */
	async generateProxyCertificate(
		team: string,
		request: GenerateCertificateRequest
	): Promise<GenerateCertificateResponse> {
		return this.post<GenerateCertificateResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/proxy-certificates`,
			request
		);
	}

	/**
	 * List all proxy certificates for a team.
	 * Returns certificate metadata without private keys.
	 */
	async listProxyCertificates(
		team: string,
		query?: ListCertificatesQuery
	): Promise<ListCertificatesResponse> {
		const params = new URLSearchParams();
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());

		const path = `/api/v1/teams/${encodeURIComponent(team)}/proxy-certificates${params.toString() ? `?${params.toString()}` : ''}`;
		return this.get<ListCertificatesResponse>(path);
	}

	/**
	 * Get a specific proxy certificate by ID.
	 * Returns certificate metadata without the private key.
	 */
	async getProxyCertificate(team: string, id: string): Promise<CertificateMetadata> {
		return this.get<CertificateMetadata>(
			`/api/v1/teams/${encodeURIComponent(team)}/proxy-certificates/${encodeURIComponent(id)}`
		);
	}

	/**
	 * Revoke a proxy certificate.
	 */
	async revokeProxyCertificate(
		team: string,
		id: string,
		reason: string
	): Promise<CertificateMetadata> {
		return this.post<CertificateMetadata>(
			`/api/v1/teams/${encodeURIComponent(team)}/proxy-certificates/${encodeURIComponent(id)}/revoke`,
			{ reason }
		);
	}

	// ============================================================================
	// Filter Types API (Dynamic Filter Framework)
	// ============================================================================

	/**
	 * List all available filter types with their schemas.
	 * Used for dynamic form generation and filter type selection.
	 */
	async listFilterTypes(): Promise<FilterTypesResponse> {
		return this.get<FilterTypesResponse>('/api/v1/filter-types');
	}

	/**
	 * Get information about a specific filter type.
	 * Returns the full schema and UI hints for form generation.
	 */
	async getFilterType(filterType: string): Promise<FilterTypeInfo> {
		return this.get<FilterTypeInfo>(`/api/v1/filter-types/${encodeURIComponent(filterType)}`);
	}

	/**
	 * Reload filter schemas from the schema directory (admin only).
	 * This allows hot-reloading of custom filter schemas.
	 */
	async reloadFilterSchemas(): Promise<void> {
		return this.post<void>('/api/v1/admin/filter-schemas/reload', {});
	}

	// ============================================================================
	// Stats Dashboard API
	// ============================================================================

	/**
	 * Check if the stats dashboard is enabled.
	 */
	async isStatsEnabled(): Promise<StatsEnabledResponse> {
		return this.get<StatsEnabledResponse>('/api/v1/stats/enabled');
	}

	/**
	 * Get stats overview for a team.
	 * Requires team:X:stats:read scope.
	 */
	async getStatsOverview(team: string): Promise<StatsOverviewResponse> {
		return this.get<StatsOverviewResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/stats/overview`
		);
	}

	/**
	 * Get all cluster stats for a team.
	 * Requires team:X:stats:read scope.
	 */
	async getClusterStats(team: string): Promise<ClustersStatsResponse> {
		return this.get<ClustersStatsResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/stats/clusters`
		);
	}

	/**
	 * Get stats for a specific cluster.
	 * Requires team:X:stats:read scope.
	 */
	async getClusterStatsById(team: string, clusterName: string): Promise<ClusterStatsResponse> {
		return this.get<ClusterStatsResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/stats/clusters/${encodeURIComponent(clusterName)}`
		);
	}

	// ============================================================================
	// Admin App Management API
	// ============================================================================

	/**
	 * List all instance apps (admin only).
	 */
	async listApps(): Promise<AppStatusResponse[]> {
		return this.get<AppStatusResponse[]>('/api/v1/admin/apps');
	}

	/**
	 * Get a specific app status (admin only).
	 */
	async getAppStatus(appId: string): Promise<AppStatusResponse> {
		return this.get<AppStatusResponse>(
			`/api/v1/admin/apps/${encodeURIComponent(appId)}`
		);
	}

	/**
	 * Enable or disable an app (admin only).
	 */
	async setAppStatus(appId: string, request: SetAppStatusRequest): Promise<AppStatusResponse> {
		return this.put<AppStatusResponse>(
			`/api/v1/admin/apps/${encodeURIComponent(appId)}`,
			request
		);
	}

	// ============================================================================
	// Admin Resource Summary API
	// ============================================================================

	async getAdminResourceSummary(): Promise<AdminResourceSummary> {
		const data = await this.get<AdminResourceSummary>('/api/v1/admin/resources/summary');
		return parseResponse(data, AdminResourceSummarySchema);
	}

	// ============================================================================
	// Secret Management API
	// ============================================================================

	/**
	 * List all secrets for a team.
	 */
	async listSecrets(team: string, query?: ListSecretsQuery): Promise<SecretResponse[]> {
		const params = new URLSearchParams();
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());
		if (query?.secret_type) params.append('secret_type', query.secret_type);

		const path = `/api/v1/teams/${encodeURIComponent(team)}/secrets${params.toString() ? `?${params.toString()}` : ''}`;
		const response = await this.get<PaginatedResponse<SecretResponse>>(path);
		const validated = parseResponse(response, paginatedSchema(SecretResponseSchema));
		return validated.items;
	}

	/**
	 * Get a specific secret by ID.
	 */
	async getSecret(team: string, secretId: string): Promise<SecretResponse> {
		return this.get<SecretResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets/${encodeURIComponent(secretId)}`
		);
	}

	/**
	 * Create a new secret with direct storage.
	 */
	async createSecret(team: string, request: CreateSecretRequest): Promise<SecretResponse> {
		return this.post<SecretResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets`,
			request
		);
	}

	/**
	 * Create a new secret with external reference.
	 */
	async createSecretReference(team: string, request: CreateSecretReferenceRequest): Promise<SecretResponse> {
		return this.post<SecretResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets/reference`,
			request
		);
	}

	/**
	 * Update an existing secret.
	 */
	async updateSecret(team: string, secretId: string, request: UpdateSecretRequest): Promise<SecretResponse> {
		return this.patch<SecretResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets/${encodeURIComponent(secretId)}`,
			request
		);
	}

	/**
	 * Rotate a secret (replaces configuration, bumps version).
	 */
	async rotateSecret(team: string, secretId: string, request: RotateSecretRequest): Promise<SecretResponse> {
		return this.post<SecretResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets/${encodeURIComponent(secretId)}/rotate`,
			request
		);
	}

	/**
	 * Delete a secret.
	 */
	async deleteSecret(team: string, secretId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/teams/${encodeURIComponent(team)}/secrets/${encodeURIComponent(secretId)}`
		);
	}

	// ============================================================================
	// Filter Install/Configure API (Filter Install/Configure Redesign)
	// ============================================================================

	/**
	 * Install a filter on a listener.
	 * This adds the filter to the listener's HCM filter chain.
	 */
	async installFilter(team: string, filterId: string, request: InstallFilterRequest): Promise<InstallFilterResponse> {
		return this.post<InstallFilterResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/installations`,
			request
		);
	}

	/**
	 * Uninstall a filter from a listener.
	 */
	async uninstallFilter(team: string, filterId: string, listenerId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/installations/${encodeURIComponent(listenerId)}`
		);
	}

	/**
	 * List all listener installations for a filter.
	 */
	async listFilterInstallations(team: string, filterId: string): Promise<FilterInstallationsResponse> {
		return this.get<FilterInstallationsResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/installations`
		);
	}

	/**
	 * Configure a filter for a scope (route-config, virtual-host, or route).
	 * This sets per-route behavior for the filter.
	 */
	async configureFilter(team: string, filterId: string, request: ConfigureFilterRequest): Promise<ConfigureFilterResponse> {
		return this.post<ConfigureFilterResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/configurations`,
			request
		);
	}

	/**
	 * Remove a filter configuration from a scope.
	 */
	async removeFilterConfiguration(team: string, filterId: string, scopeType: string, scopeId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/configurations/${encodeURIComponent(scopeType)}/${encodeURIComponent(scopeId)}`
		);
	}

	/**
	 * List all configurations for a filter.
	 */
	async listFilterConfigurations(team: string, filterId: string): Promise<FilterConfigurationsResponse> {
		return this.get<FilterConfigurationsResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/configurations`
		);
	}

	/**
	 * Get combined filter status with all installations and configurations.
	 */
	async getFilterStatus(team: string, filterId: string): Promise<FilterStatusResponse> {
		return this.get<FilterStatusResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/filters/${encodeURIComponent(filterId)}/status`
		);
	}

	// ============================================================================
	// Learning Sessions API
	// ============================================================================

	/**
	 * List learning sessions for a team.
	 * Supports filtering by status and pagination.
	 */
	async listLearningSessions(team: string, query?: ListLearningSessionsQuery): Promise<LearningSessionResponse[]> {
		const params = new URLSearchParams();
		if (query?.status) params.append('status', query.status);
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());

		const base = `/api/v1/teams/${encodeURIComponent(team)}/learning-sessions`;
		const path = `${base}${params.toString() ? `?${params.toString()}` : ''}`;
		return this.get<LearningSessionResponse[]>(path);
	}

	/**
	 * Get a specific learning session by ID.
	 */
	async getLearningSession(team: string, id: string): Promise<LearningSessionResponse> {
		return this.get<LearningSessionResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/learning-sessions/${encodeURIComponent(id)}`
		);
	}

	/**
	 * Create a new learning session.
	 * The session will automatically start capturing traffic matching the route pattern.
	 */
	async createLearningSession(team: string, request: CreateLearningSessionRequest): Promise<LearningSessionResponse> {
		return this.post<LearningSessionResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/learning-sessions`,
			request
		);
	}

	/**
	 * Cancel a learning session.
	 * This will stop traffic capture and mark the session as cancelled.
	 */
	async cancelLearningSession(team: string, id: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/teams/${encodeURIComponent(team)}/learning-sessions/${encodeURIComponent(id)}`
		);
	}

	// ============================================================================
	// Aggregated Schemas API
	// ============================================================================

	/**
	 * List aggregated schemas discovered through learning sessions.
	 * Supports filtering by path, HTTP method, and minimum confidence.
	 */
	async listAggregatedSchemas(team: string, query?: ListAggregatedSchemasQuery): Promise<AggregatedSchemaResponse[]> {
		const params = new URLSearchParams();
		if (query?.path) params.append('path', query.path);
		if (query?.httpMethod) params.append('http_method', query.httpMethod);
		if (query?.minConfidence) params.append('min_confidence', query.minConfidence.toString());
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());

		const basePath = `/api/v1/teams/${encodeURIComponent(team)}/aggregated-schemas`;
		const path = `${basePath}${params.toString() ? `?${params.toString()}` : ''}`;
		return this.get<AggregatedSchemaResponse[]>(path);
	}

	/**
	 * Get a specific aggregated schema by ID.
	 */
	async getAggregatedSchema(team: string, id: number): Promise<AggregatedSchemaResponse> {
		return this.get<AggregatedSchemaResponse>(`/api/v1/teams/${encodeURIComponent(team)}/aggregated-schemas/${id}`);
	}

	/**
	 * Compare two versions of a schema.
	 * Returns differences including breaking changes.
	 */
	async compareSchemaVersions(team: string, id: number, withVersion: number): Promise<SchemaComparisonResponse> {
		return this.get<SchemaComparisonResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/aggregated-schemas/${id}/compare?with_version=${withVersion}`
		);
	}

	/**
	 * Export a schema as OpenAPI 3.1 specification.
	 */
	async exportSchemaAsOpenApi(team: string, id: number, includeMetadata: boolean = false): Promise<OpenApiExportResponse> {
		const params = new URLSearchParams();
		params.append('include_metadata', includeMetadata.toString());

		return this.get<OpenApiExportResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/aggregated-schemas/${id}/export?${params.toString()}`
		);
	}

	/**
	 * Export multiple schemas as a unified OpenAPI 3.1 specification.
	 */
	async exportMultipleSchemasAsOpenApi(team: string, request: ExportMultipleSchemasRequest): Promise<OpenApiExportResponse> {
		return this.post<OpenApiExportResponse>(`/api/v1/teams/${encodeURIComponent(team)}/aggregated-schemas/export`, request);
	}

	// ============================================================================
	// Custom WASM Filters API (Plugin Management)
	// ============================================================================

	/**
	 * List all custom WASM filters for a team.
	 * Supports pagination via limit and offset.
	 */
	async listCustomWasmFilters(
		team: string,
		params?: { limit?: number; offset?: number }
	): Promise<ListCustomWasmFiltersResponse> {
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());

		const path = `/api/v1/teams/${encodeURIComponent(team)}/custom-filters${searchParams.toString() ? `?${searchParams.toString()}` : ''}`;
		return this.get<ListCustomWasmFiltersResponse>(path);
	}

	/**
	 * Get a specific custom WASM filter by ID.
	 */
	async getCustomWasmFilter(team: string, id: string): Promise<CustomWasmFilterResponse> {
		return this.get<CustomWasmFilterResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/custom-filters/${encodeURIComponent(id)}`
		);
	}

	/**
	 * Create a new custom WASM filter.
	 * The wasmBinaryBase64 field should contain the base64-encoded WASM binary.
	 * Upon successful creation, the filter type is automatically registered in the schema registry.
	 */
	async createCustomWasmFilter(
		team: string,
		request: CreateCustomWasmFilterRequest
	): Promise<CustomWasmFilterResponse> {
		return this.post<CustomWasmFilterResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/custom-filters`,
			request
		);
	}

	/**
	 * Update a custom WASM filter's metadata.
	 * Note: The WASM binary cannot be updated. Upload a new filter instead.
	 */
	async updateCustomWasmFilter(
		team: string,
		id: string,
		request: UpdateCustomWasmFilterRequest
	): Promise<CustomWasmFilterResponse> {
		return this.patch<CustomWasmFilterResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/custom-filters/${encodeURIComponent(id)}`,
			request
		);
	}

	/**
	 * Delete a custom WASM filter.
	 * Warning: Ensure no filter instances are using this filter type before deletion.
	 */
	async deleteCustomWasmFilter(team: string, id: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/teams/${encodeURIComponent(team)}/custom-filters/${encodeURIComponent(id)}`
		);
	}

	/**
	 * Download the WASM binary for a custom filter.
	 * Returns the binary as a Blob.
	 */
	async downloadCustomWasmFilterBinary(team: string, id: string): Promise<Blob> {
		const response = await fetch(
			`${API_BASE}/api/v1/teams/${encodeURIComponent(team)}/custom-filters/${encodeURIComponent(id)}/download`,
			{
				method: 'GET',
				headers: await this.getHeaders(),
				credentials: 'include'
			}
		);

		if (!response.ok) {
			let errorMessage = `HTTP ${response.status}: ${response.statusText}`;
			try {
				const errorData = await response.json();
				errorMessage = errorData.message || errorMessage;
			} catch {
				// If JSON parsing fails, use status text
			}
			throw new Error(errorMessage);
		}

		return response.blob();
	}

	// ============================================================================
	// MCP (Model Context Protocol) API
	// ============================================================================

	/**
	 * List MCP tools for a team.
	 * Supports filtering by category, enabled status, and search.
	 */
	async listMcpTools(team: string, query?: ListMcpToolsQuery): Promise<ListMcpToolsResponse> {
		const params = new URLSearchParams();
		if (query?.category) params.append('category', query.category);
		if (query?.enabled !== undefined) params.append('enabled', String(query.enabled));
		if (query?.search) params.append('search', query.search);
		if (query?.limit) params.append('limit', String(query.limit));
		if (query?.offset) params.append('offset', String(query.offset));

		const queryString = params.toString();
		const path = `/api/v1/teams/${encodeURIComponent(team)}/mcp/tools${queryString ? `?${queryString}` : ''}`;
		return this.get<ListMcpToolsResponse>(path);
	}

	/**
	 * Get a specific MCP tool by name.
	 */
	async getMcpTool(team: string, name: string): Promise<McpTool> {
		return this.get<McpTool>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/tools/${encodeURIComponent(name)}`
		);
	}

	/**
	 * Update an MCP tool (enable/disable or update description).
	 */
	async updateMcpTool(team: string, name: string, request: UpdateMcpToolRequest): Promise<McpTool> {
		return this.patch<McpTool>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/tools/${encodeURIComponent(name)}`,
			request
		);
	}

	/**
	 * Get MCP status for a route.
	 * Returns readiness, schema sources, and metadata.
	 */
	async getMcpStatus(team: string, routeId: string): Promise<McpStatus> {
		return this.get<McpStatus>(
			`/api/v1/teams/${encodeURIComponent(team)}/routes/${encodeURIComponent(routeId)}/mcp/status`
		);
	}

	/**
	 * Enable MCP on a route.
	 * Creates an MCP tool for the route with optional configuration.
	 */
	async enableMcp(team: string, routeId: string, request?: EnableMcpRequest): Promise<EnableMcpResponse> {
		return this.post<EnableMcpResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/routes/${encodeURIComponent(routeId)}/mcp/enable`,
			request || {}
		);
	}

	/**
	 * Disable MCP on a route.
	 * Removes the MCP tool for the route.
	 */
	async disableMcp(team: string, routeId: string): Promise<McpOperationResponse> {
		return this.post<McpOperationResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/routes/${encodeURIComponent(routeId)}/mcp/disable`,
			{}
		);
	}

	/**
	 * Refresh MCP schema for a route.
	 * Re-generates the input/output schemas from the latest metadata.
	 */
	async refreshMcpSchema(team: string, routeId: string): Promise<McpOperationResponse> {
		return this.post<McpOperationResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/routes/${encodeURIComponent(routeId)}/mcp/refresh`,
			{}
		);
	}

	/**
	 * Bulk enable MCP on multiple routes.
	 */
	async bulkEnableMcp(team: string, request: BulkMcpRequest): Promise<BulkMcpResponse> {
		return this.post<BulkMcpResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/bulk-enable`,
			request
		);
	}

	/**
	 * Bulk disable MCP on multiple routes.
	 */
	async bulkDisableMcp(team: string, request: BulkMcpRequest): Promise<BulkMcpResponse> {
		return this.post<BulkMcpResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/bulk-disable`,
			request
		);
	}

	/**
	 * Check if a learned schema is available for a route.
	 * Returns availability status, schema info, and whether force is required to override.
	 */
	async checkLearnedSchema(team: string, routeId: string): Promise<LearnedSchemaAvailability> {
		return this.get<LearnedSchemaAvailability>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/routes/${encodeURIComponent(routeId)}/learned-schema`
		);
	}

	/**
	 * Apply a learned schema to a route's MCP tool.
	 * If the route currently uses OpenAPI, force=true must be set to override.
	 */
	async applyLearnedSchema(
		team: string,
		routeId: string,
		force?: boolean
	): Promise<ApplyLearnedSchemaResponse> {
		const request: ApplyLearnedSchemaRequest = force ? { force } : {};
		return this.post<ApplyLearnedSchemaResponse>(
			`/api/v1/teams/${encodeURIComponent(team)}/mcp/routes/${encodeURIComponent(routeId)}/apply-learned`,
			request
		);
	}

	// ============================================================================
	// MCP Protocol Server Communication
	// ============================================================================

	/**
	 * Ping the MCP server to check connectivity and measure latency.
	 * Sends a JSON-RPC 2.0 ping request to the MCP endpoint.
	 */
	async pingMcpServer(team: string): Promise<{ success: boolean; latencyMs: number; error?: string }> {
		const start = Date.now();

		try {
			const response = await fetch(`${API_BASE}/api/v1/mcp/cp?team=${encodeURIComponent(team)}`, {
				method: 'POST',
				headers: await this.getHeaders(),
				credentials: 'include',
				body: JSON.stringify({
					jsonrpc: '2.0',
					id: crypto.randomUUID(),
					method: 'ping',
					params: {}
				})
			});

			const latencyMs = Date.now() - start;

			if (!response.ok) {
				return {
					success: false,
					latencyMs,
					error: `HTTP ${response.status}: ${response.statusText}`
				};
			}

			const data = await response.json();

			if (data.error) {
				return {
					success: false,
					latencyMs,
					error: data.error.message || 'Unknown error'
				};
			}

			return {
				success: true,
				latencyMs
			};
		} catch (error) {
			const latencyMs = Date.now() - start;
			return {
				success: false,
				latencyMs,
				error: error instanceof Error ? error.message : 'Network error'
			};
		}
	}

	/**
	 * List active MCP/SSE connections for a team
	 *
	 * @param team - Team identifier
	 * @returns List of active connections with metadata
	 */
	async listMcpConnections(
		team: string
	): Promise<{ connections: McpConnectionInfo[]; totalCount: number }> {
		const response = await fetch(
			`${API_BASE}/api/v1/mcp/cp/connections?team=${encodeURIComponent(team)}`,
			{
				method: 'GET',
				headers: await this.getHeaders(),
				credentials: 'include'
			}
		);

		if (!response.ok) {
			const error = await response.json().catch(() => ({ message: 'Unknown error' }));
			throw new Error(error.message || `HTTP ${response.status}: ${response.statusText}`);
		}

		return response.json();
	}
	// ============================================================================
	// Dataplane API
	// ============================================================================

	/**
	 * List dataplanes for a team.
	 */
	async listDataplanes(team: string, query?: { limit?: number; offset?: number }): Promise<DataplaneResponse[]> {
		const params = new URLSearchParams();
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());

		const path = `/api/v1/teams/${encodeURIComponent(team)}/dataplanes${params.toString() ? `?${params.toString()}` : ''}`;
		const response = await this.get<PaginatedResponse<DataplaneResponse>>(path);
		return response.items;
	}

	/**
	 * List all dataplanes across all teams (admin only).
	 */
	async listAllDataplanes(query?: { limit?: number; offset?: number }): Promise<DataplaneResponse[]> {
		const params = new URLSearchParams();
		if (query?.limit) params.append('limit', query.limit.toString());
		if (query?.offset) params.append('offset', query.offset.toString());

		const path = `/api/v1/dataplanes${params.toString() ? `?${params.toString()}` : ''}`;
		const response = await this.get<PaginatedResponse<DataplaneResponse>>(path);
		return response.items;
	}

	/**
	 * Get a specific dataplane by name.
	 */
	async getDataplane(team: string, name: string): Promise<DataplaneResponse> {
		return this.get<DataplaneResponse>(`/api/v1/teams/${encodeURIComponent(team)}/dataplanes/${encodeURIComponent(name)}`);
	}

	/**
	 * Create a new dataplane.
	 */
	async createDataplane(team: string, body: CreateDataplaneBody): Promise<DataplaneResponse> {
		return this.post<DataplaneResponse>(`/api/v1/teams/${encodeURIComponent(team)}/dataplanes`, body);
	}

	/**
	 * Update a dataplane.
	 */
	async updateDataplane(team: string, name: string, body: UpdateDataplaneBody): Promise<DataplaneResponse> {
		return this.patch<DataplaneResponse>(`/api/v1/teams/${encodeURIComponent(team)}/dataplanes/${encodeURIComponent(name)}`, body);
	}

	/**
	 * Delete a dataplane.
	 */
	async deleteDataplane(team: string, name: string): Promise<void> {
		return this.delete<void>(`/api/v1/teams/${encodeURIComponent(team)}/dataplanes/${encodeURIComponent(name)}`);
	}

	/**
	 * Get Envoy configuration for a dataplane.
	 * Returns YAML or JSON based on the format parameter.
	 */
	async getDataplaneEnvoyConfig(
		team: string,
		name: string,
		options: {
			format?: 'yaml' | 'json';
			mtls?: boolean;
			certPath?: string;
			keyPath?: string;
			caPath?: string;
		} = {}
	): Promise<string> {
		const params = new URLSearchParams();
		params.append('format', options.format || 'yaml');
		if (options.mtls !== undefined) {
			params.append('mtls', String(options.mtls));
		}
		if (options.certPath) {
			params.append('cert_path', options.certPath);
		}
		if (options.keyPath) {
			params.append('key_path', options.keyPath);
		}
		if (options.caPath) {
			params.append('ca_path', options.caPath);
		}

		const path = `/api/v1/teams/${encodeURIComponent(team)}/dataplanes/${encodeURIComponent(name)}/envoy-config?${params.toString()}`;

		const response = await fetch(`${API_BASE}${path}`, {
			method: 'GET',
			headers: await this.getHeaders(),
			credentials: 'include'
		});

		if (!response.ok) {
			const errorText = await response.text();
			throw new Error(errorText || `HTTP ${response.status}: ${response.statusText}`);
		}

		return response.text();
	}

	// ============================================================================
	// Admin Organization CRUD API
	// ============================================================================

	async createOrganization(data: CreateOrganizationRequest): Promise<OrganizationResponse> {
		return this.post<OrganizationResponse>('/api/v1/admin/organizations', data);
	}

	async listOrganizations(limit?: number, offset?: number): Promise<AdminListOrgsResponse> {
		const params = new URLSearchParams();
		if (limit !== undefined) params.append('limit', limit.toString());
		if (offset !== undefined) params.append('offset', offset.toString());

		const query = params.toString();
		return parseResponse(
			await this.get<AdminListOrgsResponse>(`/api/v1/admin/organizations${query ? `?${query}` : ''}`),
			AdminListOrgsResponseSchema
		);
	}

	async getOrganization(id: string): Promise<OrganizationResponse> {
		return this.get<OrganizationResponse>(`/api/v1/admin/organizations/${encodeURIComponent(id)}`);
	}

	async updateOrganization(id: string, data: UpdateOrganizationRequest): Promise<OrganizationResponse> {
		return this.patch<OrganizationResponse>(`/api/v1/admin/organizations/${encodeURIComponent(id)}`, data);
	}

	async deleteOrganization(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/admin/organizations/${encodeURIComponent(id)}`);
	}

	// ============================================================================
	// Admin Organization Members API
	// ============================================================================

	async listOrgMembers(orgId: string): Promise<OrgMembershipResponse[]> {
		const response = await this.get<{ members: OrgMembershipResponse[] }>(
			`/api/v1/admin/organizations/${encodeURIComponent(orgId)}/members`
		);
		return response.members;
	}

	async addOrgMember(orgId: string, data: AddOrgMemberRequest): Promise<OrgMembershipResponse> {
		return this.post<OrgMembershipResponse>(
			`/api/v1/admin/organizations/${encodeURIComponent(orgId)}/members`,
			data
		);
	}

	async updateOrgMemberRole(orgId: string, userId: string, role: OrgRole): Promise<OrgMembershipResponse> {
		return this.put<OrgMembershipResponse>(
			`/api/v1/admin/organizations/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}`,
			{ role }
		);
	}

	async removeOrgMember(orgId: string, userId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/admin/organizations/${encodeURIComponent(orgId)}/members/${encodeURIComponent(userId)}`
		);
	}

	// ============================================================================
	// Org-Scoped API
	// ============================================================================

	async getCurrentOrg(): Promise<CurrentOrgResponse> {
		return this.get<CurrentOrgResponse>('/api/v1/orgs/current');
	}

	async listOrgTeams(orgName: string): Promise<ListOrgTeamsResponse> {
		return this.get<ListOrgTeamsResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams`
		);
	}

	async createOrgTeam(orgName: string, data: CreateTeamRequest): Promise<TeamResponse> {
		return this.post<TeamResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams`,
			data
		);
	}

	async getOrgTeam(orgName: string, teamName: string): Promise<TeamResponse> {
		const response = await this.listOrgTeams(orgName);
		const team = response.teams.find((t) => t.name === teamName);
		if (!team) {
			throw new Error(`Team '${teamName}' not found`);
		}
		return team;
	}

	async updateOrgTeam(
		orgName: string,
		teamName: string,
		data: UpdateTeamRequest
	): Promise<TeamResponse> {
		return this.patch<TeamResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams/${encodeURIComponent(teamName)}`,
			data
		);
	}

	async deleteOrgTeam(orgName: string, teamName: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams/${encodeURIComponent(teamName)}`
		);
	}

	async listTeamMembers(
		orgName: string,
		teamName: string
	): Promise<ListOrgTeamMembersResponse> {
		return this.get<ListOrgTeamMembersResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams/${encodeURIComponent(teamName)}/members`
		);
	}

	async addTeamMember(
		orgName: string,
		teamName: string,
		data: AddOrgTeamMemberRequest
	): Promise<OrgTeamMemberResponse> {
		return this.post<OrgTeamMemberResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams/${encodeURIComponent(teamName)}/members`,
			data
		);
	}

	async removeTeamMember(orgName: string, teamName: string, userId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/teams/${encodeURIComponent(teamName)}/members/${encodeURIComponent(userId)}`
		);
	}

	// ============================================================================
	// Invite API (Instant Provisioning via Admin)
	// ============================================================================

	async inviteOrgMember(
		orgId: string,
		req: InviteOrgMemberRequest
	): Promise<InviteOrgMemberResponse> {
		return this.post<InviteOrgMemberResponse>(
			`/api/v1/admin/organizations/${encodeURIComponent(orgId)}/invite`,
			req
		);
	}

	// ============================================================================
	// Agent API
	// ============================================================================

	async listOrgAgents(orgName: string): Promise<ListAgentsResponse> {
		return this.get<ListAgentsResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/agents`
		);
	}

	async createOrgAgent(orgName: string, data: CreateAgentRequest): Promise<CreateAgentResponse> {
		return this.post<CreateAgentResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/agents`,
			data
		);
	}

	async deleteOrgAgent(orgName: string, agentName: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/agents/${encodeURIComponent(agentName)}`
		);
	}

	// ============================================================================
	// Unified Grant API (principals — users and agents)
	// ============================================================================

	async createPrincipalGrant(
		orgName: string,
		principalId: string,
		request: CreateGrantRequest
	): Promise<GrantResponse> {
		return this.post<GrantResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/principals/${encodeURIComponent(principalId)}/grants`,
			request
		);
	}

	async listPrincipalGrants(orgName: string, principalId: string): Promise<GrantListResponse> {
		return this.get<GrantListResponse>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/principals/${encodeURIComponent(principalId)}/grants`
		);
	}

	async deletePrincipalGrant(orgName: string, principalId: string, grantId: string): Promise<void> {
		return this.delete<void>(
			`/api/v1/orgs/${encodeURIComponent(orgName)}/principals/${encodeURIComponent(principalId)}/grants/${encodeURIComponent(grantId)}`
		);
	}
}

/**
 * MCP Client information (captured during initialize)
 */
export interface McpClientInfo {
	name: string;
	version: string;
}

/**
 * Type of MCP connection
 */
export type McpConnectionType = 'sse' | 'http';

/**
 * MCP Connection information
 */
export interface McpConnectionInfo {
	connectionId: string;
	team: string;
	createdAt: string;
	lastActivity: string;
	logLevel: string;
	/** Client information (name, version) if available */
	clientInfo?: McpClientInfo;
	/** Negotiated protocol version if initialized */
	protocolVersion?: string;
	/** Whether the connection has completed initialization */
	initialized: boolean;
	/** Type of connection (sse for streaming, http for stateless) */
	connectionType: McpConnectionType;
}

// Export singleton instance
export const apiClient = new ApiClient();
