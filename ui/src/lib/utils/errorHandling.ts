export interface ApiErrorResponse {
	status: number;
	message: string;
	userMessage: string;
	showContactSupport: boolean;
}

/**
 * Handle API errors and return a user-friendly error response.
 * @param error - The error object (Response, Error, or unknown)
 * @param operation - Description of the operation being performed (e.g., "create user", "delete session")
 */
export function handleApiError(error: unknown, operation: string): ApiErrorResponse {
	// Check if it's a Response or has status
	let status = 500;
	let message = 'An unexpected error occurred';

	if (error instanceof Response) {
		status = error.status;
		message = error.statusText;
	} else if (error && typeof error === 'object' && 'status' in error) {
		status = (error as { status: number }).status;
		if ('message' in error) {
			message = String((error as { message: unknown }).message);
		}
	} else if (error instanceof Error) {
		message = error.message;
	}

	switch (status) {
		case 403:
			return {
				status,
				message,
				userMessage: `You don't have permission to ${operation}. Contact your administrator.`,
				showContactSupport: true
			};
		case 404:
			return {
				status,
				message,
				userMessage: `The requested resource was not found.`,
				showContactSupport: false
			};
		case 400:
			return {
				status,
				message,
				userMessage: `Invalid request: ${message}`,
				showContactSupport: false
			};
		case 401:
			return {
				status,
				message,
				userMessage: 'Your session has expired. Please log in again.',
				showContactSupport: false
			};
		default:
			return {
				status,
				message,
				userMessage: `An error occurred while trying to ${operation}. Please try again.`,
				showContactSupport: true
			};
	}
}
