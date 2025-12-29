/**
 * Common form validation utilities for DRY principle compliance.
 * Consolidates duplicate validation patterns across create/edit pages.
 */

/**
 * Validates that a required string field is not empty
 * @param value - The value to validate
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validateRequired(value: string | undefined | null, fieldName: string): string | null {
	if (!value || !value.trim()) {
		return `${fieldName} is required`;
	}
	return null;
}

/**
 * Validates that a string does not exceed a maximum length
 * @param value - The value to validate
 * @param maxLength - Maximum allowed length
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validateMaxLength(value: string, maxLength: number, fieldName: string): string | null {
	if (value.length > maxLength) {
		return `${fieldName} must be ${maxLength} characters or less`;
	}
	return null;
}

/**
 * Validates that a string has at least a minimum length
 * @param value - The value to validate
 * @param minLength - Minimum required length
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validateMinLength(value: string, minLength: number, fieldName: string): string | null {
	if (value.length < minLength) {
		return `${fieldName} must be at least ${minLength} characters`;
	}
	return null;
}

/**
 * Validates that a value matches a specific pattern (lowercase, alphanumeric, hyphens)
 * Common for resource identifiers like cluster names, listener names, etc.
 * @param value - The value to validate
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validateIdentifier(value: string, fieldName: string): string | null {
	const pattern = /^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/;
	if (!pattern.test(value)) {
		return `${fieldName} must be lowercase, start and end with alphanumeric characters, and may contain hyphens`;
	}
	return null;
}

/**
 * Validates email format
 * @param value - The email to validate
 * @returns Error message or null if valid
 */
export function validateEmail(value: string): string | null {
	const emailPattern = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
	if (!emailPattern.test(value)) {
		return 'Please enter a valid email address';
	}
	return null;
}

/**
 * Validates password strength
 * Checks for minimum length and character requirements
 * @param password - The password to validate
 * @param options - Validation options
 * @returns Error message or null if valid
 */
export function validatePassword(
	password: string,
	options: {
		minLength?: number;
		requireUppercase?: boolean;
		requireLowercase?: boolean;
		requireNumber?: boolean;
		requireSpecial?: boolean;
	} = {}
): string | null {
	const {
		minLength = 8,
		requireUppercase = true,
		requireLowercase = true,
		requireNumber = true,
		requireSpecial = false
	} = options;

	if (password.length < minLength) {
		return `Password must be at least ${minLength} characters`;
	}

	if (requireUppercase && !/[A-Z]/.test(password)) {
		return 'Password must contain at least one uppercase letter';
	}

	if (requireLowercase && !/[a-z]/.test(password)) {
		return 'Password must contain at least one lowercase letter';
	}

	if (requireNumber && !/\d/.test(password)) {
		return 'Password must contain at least one number';
	}

	if (requireSpecial && !/[!@#$%^&*(),.?":{}|<>]/.test(password)) {
		return 'Password must contain at least one special character';
	}

	return null;
}

/**
 * Validates that two password fields match
 * @param password - The password
 * @param confirmPassword - The confirmation password
 * @returns Error message or null if valid
 */
export function validatePasswordMatch(password: string, confirmPassword: string): string | null {
	if (password !== confirmPassword) {
		return 'Passwords do not match';
	}
	return null;
}

/**
 * Validates a number is within a range
 * @param value - The number to validate
 * @param min - Minimum value (inclusive)
 * @param max - Maximum value (inclusive)
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validateRange(value: number, min: number, max: number, fieldName: string): string | null {
	if (value < min || value > max) {
		return `${fieldName} must be between ${min} and ${max}`;
	}
	return null;
}

/**
 * Validates a positive integer
 * @param value - The value to validate
 * @param fieldName - Human-readable field name for error message
 * @returns Error message or null if valid
 */
export function validatePositiveInteger(value: number, fieldName: string): string | null {
	if (!Number.isInteger(value) || value <= 0) {
		return `${fieldName} must be a positive integer`;
	}
	return null;
}

/**
 * Validates a port number (1-65535)
 * @param value - The port number to validate
 * @returns Error message or null if valid
 */
export function validatePort(value: number): string | null {
	if (!Number.isInteger(value) || value < 1 || value > 65535) {
		return 'Port must be between 1 and 65535';
	}
	return null;
}

/**
 * Validates a URL format
 * @param value - The URL to validate
 * @param requireHttps - Whether to require HTTPS
 * @returns Error message or null if valid
 */
export function validateUrl(value: string, requireHttps = false): string | null {
	try {
		const url = new URL(value);
		if (requireHttps && url.protocol !== 'https:') {
			return 'URL must use HTTPS';
		}
		return null;
	} catch {
		return 'Please enter a valid URL';
	}
}

/**
 * Runs multiple validators and returns the first error
 * @param validators - Array of validator functions to run
 * @returns First error message or null if all pass
 *
 * @example
 * ```typescript
 * const error = runValidators([
 *   () => validateRequired(name, 'Name'),
 *   () => validateMaxLength(name, 255, 'Name'),
 *   () => validateIdentifier(name, 'Name')
 * ]);
 * ```
 */
export function runValidators(validators: Array<() => string | null>): string | null {
	for (const validator of validators) {
		const error = validator();
		if (error) {
			return error;
		}
	}
	return null;
}
