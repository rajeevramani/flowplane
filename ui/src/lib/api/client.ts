// API client with CSRF token handling
import { goto } from '$app/navigation';
import { env } from '$env/dynamic/public';
import type {
	LoginRequest,
	LoginResponse,
	ChangePasswordRequest,
	BootstrapStatusResponse,
	BootstrapInitializeRequest,
	BootstrapInitializeResponse,
	SessionInfoResponse,
	DashboardStats,
	ApiError,
	PersonalAccessToken,
	CreateTokenRequest,
	TokenSecretResponse,
	UpdateTokenRequest,
	ImportOpenApiRequest,
	ImportResponse,
	ImportSummary,
	ImportDetailsResponse,
	ListenerResponse,
	RouteResponse,
	ClusterResponse,
	BootstrapConfigRequest,
	ListTeamsResponse,
	TeamResponse,
	CreateTeamRequest,
	UpdateTeamRequest,
	AdminListTeamsResponse,
	UserResponse,
	UserWithTeamsResponse,
	CreateUserRequest,
	UpdateUserRequest,
	ListUsersResponse,
	UserTeamMembership,
	CreateTeamMembershipRequest,
	AuditLogEntry,
	ListAuditLogsQuery,
	ListAuditLogsResponse
} from './types';

const API_BASE = env.PUBLIC_API_BASE || 'http://localhost:8080';

class ApiClient {
	private csrfToken: string | null = null;

	constructor() {
		// Load CSRF token from sessionStorage on initialization
		if (typeof window !== 'undefined') {
			this.csrfToken = sessionStorage.getItem('csrf_token');
		}
	}

	private getHeaders(includeCSRF: boolean = false): HeadersInit {
		const headers: HeadersInit = {
			'Content-Type': 'application/json',
		};

		if (includeCSRF && this.csrfToken) {
			headers['X-CSRF-Token'] = this.csrfToken;
		}

		return headers;
	}

	private async handleResponse<T>(response: Response): Promise<T> {
		// Check for CSRF token in response headers
		const csrfHeader = response.headers.get('X-CSRF-Token');
		if (csrfHeader) {
			this.csrfToken = csrfHeader;
			sessionStorage.setItem('csrf_token', csrfHeader);
		}

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

	async login(credentials: LoginRequest): Promise<LoginResponse> {
		const response = await fetch(`${API_BASE}/api/v1/auth/login`, {
			method: 'POST',
			headers: this.getHeaders(),
			body: JSON.stringify(credentials),
			credentials: 'include', // Include cookies
		});

		const data = await this.handleResponse<LoginResponse>(response);

		// Store CSRF token
		if (data.csrfToken) {
			this.csrfToken = data.csrfToken;
			sessionStorage.setItem('csrf_token', data.csrfToken);
		}

		return data;
	}

	async logout(): Promise<void> {
		try {
			const response = await fetch(`${API_BASE}/api/v1/auth/sessions/logout`, {
				method: 'POST',
				headers: this.getHeaders(true), // Include CSRF token
				credentials: 'include',
			});

			await this.handleResponse(response);
		} finally {
			// Always clear local auth state
			this.clearAuth();
		}
	}

	async getSessionInfo(): Promise<SessionInfoResponse> {
		const response = await fetch(`${API_BASE}/api/v1/auth/sessions/me`, {
			method: 'GET',
			headers: this.getHeaders(),
			credentials: 'include',
		});

		return this.handleResponse<SessionInfoResponse>(response);
	}

	async changePassword(request: ChangePasswordRequest): Promise<void> {
		const response = await fetch(`${API_BASE}/api/v1/auth/change-password`, {
			method: 'POST',
			headers: this.getHeaders(true), // Include CSRF token
			body: JSON.stringify(request),
			credentials: 'include',
		});

		await this.handleResponse<void>(response);
	}

	clearAuth() {
		this.csrfToken = null;
		if (typeof window !== 'undefined') {
			sessionStorage.removeItem('csrf_token');
		}
	}

	// Generic methods for authenticated requests
	async get<T>(path: string): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'GET',
			headers: this.getHeaders(),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async post<T>(path: string, body: any): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'POST',
			headers: this.getHeaders(true), // Include CSRF
			body: JSON.stringify(body),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async put<T>(path: string, body: any): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'PUT',
			headers: this.getHeaders(true), // Include CSRF
			body: JSON.stringify(body),
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	async delete<T>(path: string): Promise<T> {
		const response = await fetch(`${API_BASE}${path}`, {
			method: 'DELETE',
			headers: this.getHeaders(true), // Include CSRF
			credentials: 'include',
		});

		return this.handleResponse<T>(response);
	}

	// Bootstrap methods
	async getBootstrapStatus(): Promise<BootstrapStatusResponse> {
		const response = await fetch(`${API_BASE}/api/v1/bootstrap/status`, {
			method: 'GET',
			headers: this.getHeaders(),
		});

		return this.handleResponse<BootstrapStatusResponse>(response);
	}

	async bootstrapInitialize(
		data: BootstrapInitializeRequest
	): Promise<BootstrapInitializeResponse> {
		const response = await fetch(`${API_BASE}/api/v1/bootstrap/initialize`, {
			method: 'POST',
			headers: this.getHeaders(),
			body: JSON.stringify(data),
		});

		return this.handleResponse<BootstrapInitializeResponse>(response);
	}

	// Token management methods
	async listTokens(limit?: number, offset?: number): Promise<PersonalAccessToken[]> {
		let path = '/api/v1/tokens';
		const params = new URLSearchParams();
		if (limit) params.append('limit', limit.toString());
		if (offset) params.append('offset', offset.toString());
		if (params.toString()) path += `?${params.toString()}`;

		return this.get<PersonalAccessToken[]>(path);
	}

	async getToken(id: string): Promise<PersonalAccessToken> {
		return this.get<PersonalAccessToken>(`/api/v1/tokens/${id}`);
	}

	async createToken(request: CreateTokenRequest): Promise<TokenSecretResponse> {
		return this.post<TokenSecretResponse>('/api/v1/tokens', request);
	}

	async updateToken(id: string, request: UpdateTokenRequest): Promise<PersonalAccessToken> {
		return this.put<PersonalAccessToken>(`/api/v1/tokens/${id}`, request);
	}

	async revokeToken(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/tokens/${id}`);
	}

	async rotateToken(id: string): Promise<TokenSecretResponse> {
		return this.post<TokenSecretResponse>(`/api/v1/tokens/${id}/rotate`, {});
	}

	// OpenAPI import
	async importOpenApiSpec(request: ImportOpenApiRequest): Promise<ImportResponse> {
		const params = new URLSearchParams();
		if (request.team) params.append('team', request.team);
		if (request.listenerIsolation !== undefined) {
			params.append('shared_listener', (!request.listenerIsolation).toString());
		}
		if (request.port) params.append('port', request.port.toString());

		const path = `/api/v1/openapi/import${params.toString() ? `?${params.toString()}` : ''}`;

		// Determine content type based on spec format
		const isYaml = request.spec.trim().startsWith('openapi:') || request.spec.trim().startsWith('swagger:');
		const contentType = isYaml ? 'application/yaml' : 'application/json';

		const response = await fetch(`${API_BASE}${path}`, {
			method: 'POST',
			headers: {
				...this.getHeaders(true), // Include CSRF
				'Content-Type': contentType
			},
			body: request.spec,
			credentials: 'include'
		});

		return this.handleResponse<ImportResponse>(response);
	}

	// Import methods (replacing API Definition methods)
	async listImports(team: string): Promise<ImportSummary[]> {
		const path = `/api/v1/openapi/imports?team=${encodeURIComponent(team)}`;
		const response = await this.get<{ imports: ImportSummary[] }>(path);
		return response.imports;
	}

	// List all imports across all teams (admin only)
	async listAllImports(): Promise<ImportSummary[]> {
		const path = '/api/v1/openapi/imports';
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
	async listListeners(params?: { limit?: number; offset?: number }): Promise<ListenerResponse[]> {
		let path = '/api/v1/listeners';
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		return this.get<ListenerResponse[]>(path);
	}

	async getListener(name: string): Promise<ListenerResponse> {
		return this.get<ListenerResponse>(`/api/v1/listeners/${name}`);
	}

	async deleteListener(name: string): Promise<void> {
		return this.delete<void>(`/api/v1/listeners/${name}`);
	}

	// Route methods
	async listRoutes(params?: { limit?: number; offset?: number }): Promise<RouteResponse[]> {
		let path = '/api/v1/routes';
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		return this.get<RouteResponse[]>(path);
	}

	async getRoute(name: string): Promise<RouteResponse> {
		return this.get<RouteResponse>(`/api/v1/routes/${name}`);
	}

	async deleteRoute(name: string): Promise<void> {
		return this.delete<void>(`/api/v1/routes/${name}`);
	}

	// Cluster methods
	async listClusters(params?: { limit?: number; offset?: number }): Promise<ClusterResponse[]> {
		let path = '/api/v1/clusters';
		const searchParams = new URLSearchParams();
		if (params?.limit) searchParams.append('limit', params.limit.toString());
		if (params?.offset) searchParams.append('offset', params.offset.toString());
		if (searchParams.toString()) path += `?${searchParams.toString()}`;

		return this.get<ClusterResponse[]>(path);
	}

	async getCluster(name: string): Promise<ClusterResponse> {
		return this.get<ClusterResponse>(`/api/v1/clusters/${name}`);
	}

	async deleteCluster(name: string): Promise<void> {
		return this.delete<void>(`/api/v1/clusters/${name}`);
	}

	// Bootstrap configuration methods
	async getBootstrapConfig(request: BootstrapConfigRequest): Promise<string> {
		const params = new URLSearchParams();
		if (request.format) params.append('format', request.format);

		const path = `/api/v1/teams/${request.team}/bootstrap${params.toString() ? `?${params.toString()}` : ''}`;

		const response = await fetch(`${API_BASE}${path}`, {
			method: 'GET',
			headers: this.getHeaders(),
			credentials: 'include'
		});

		if (!response.ok) {
			const errorText = await response.text();
			throw new Error(errorText || `HTTP ${response.status}: ${response.statusText}`);
		}

		// Return the raw text (YAML or JSON)
		return response.text();
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
		return this.put<TeamResponse>(`/api/v1/admin/teams/${id}`, request);
	}

	async adminDeleteTeam(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/admin/teams/${id}`);
	}

	// User Management methods (admin only)
	async listUsers(limit: number = 50, offset: number = 0): Promise<ListUsersResponse> {
		const params = new URLSearchParams();
		params.append('limit', limit.toString());
		params.append('offset', offset.toString());

		return this.get<ListUsersResponse>(`/api/v1/users?${params.toString()}`);
	}

	async getUser(id: string): Promise<UserWithTeamsResponse> {
		return this.get<UserWithTeamsResponse>(`/api/v1/users/${id}`);
	}

	async createUser(request: CreateUserRequest): Promise<UserResponse> {
		return this.post<UserResponse>('/api/v1/users', request);
	}

	async updateUser(id: string, request: UpdateUserRequest): Promise<UserResponse> {
		return this.put<UserResponse>(`/api/v1/users/${id}`, request);
	}

	async deleteUser(id: string): Promise<void> {
		return this.delete<void>(`/api/v1/users/${id}`);
	}

	async listUserTeams(userId: string): Promise<UserTeamMembership[]> {
		return this.get<UserTeamMembership[]>(`/api/v1/users/${userId}/teams`);
	}

	async addTeamMembership(userId: string, request: CreateTeamMembershipRequest): Promise<UserTeamMembership> {
		return this.post<UserTeamMembership>(`/api/v1/users/${userId}/teams`, request);
	}

	async removeTeamMembership(userId: string, team: string): Promise<void> {
		return this.delete<void>(`/api/v1/users/${userId}/teams/${team}`);
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
}

// Export singleton instance
export const apiClient = new ApiClient();
