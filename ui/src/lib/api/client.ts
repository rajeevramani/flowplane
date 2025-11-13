// API client with CSRF token handling
import { goto } from '$app/navigation';
import type { LoginRequest, LoginResponse, ApiError } from './types';

const API_BASE = 'http://localhost:8080';

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

	async getSessionInfo(): Promise<any> {
		const response = await fetch(`${API_BASE}/api/v1/auth/sessions/me`, {
			method: 'GET',
			headers: this.getHeaders(),
			credentials: 'include',
		});

		return this.handleResponse(response);
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
}

// Export singleton instance
export const apiClient = new ApiClient();
