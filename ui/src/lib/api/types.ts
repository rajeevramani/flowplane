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

export interface ApiError {
	message: string;
	code?: string;
}
