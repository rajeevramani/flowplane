import { goto } from '$app/navigation';

/**
 * Options for the form submission hook
 */
export interface FormSubmitOptions<T> {
	/** Validation function that returns an error message or null if valid */
	validate: () => string | null;
	/** Async function that performs the actual submission */
	submit: () => Promise<T>;
	/** Path to navigate to on successful submission */
	successPath: string;
	/** Optional callback on successful submission */
	onSuccess?: (result: T) => void;
	/** Optional callback on error */
	onError?: (error: Error) => void;
}

/**
 * State returned by the form submission hook
 */
export interface FormSubmitState {
	error: string | null;
	isSubmitting: boolean;
}

/**
 * Actions returned by the form submission hook
 */
export interface FormSubmitActions {
	handleSubmit: () => Promise<void>;
	handleCancel: () => void;
	clearError: () => void;
	setError: (message: string) => void;
}

/**
 * Creates form submission handlers with standardized error handling and loading states.
 *
 * This hook consolidates the common pattern of:
 * 1. Validate form data
 * 2. Set loading state
 * 3. Call API
 * 4. Navigate on success or show error
 *
 * @example
 * ```typescript
 * let formState = $state<FormSubmitState>({ error: null, isSubmitting: false });
 *
 * const { handleSubmit, handleCancel, setError } = createFormSubmit({
 *   validate: () => {
 *     if (!name) return 'Name is required';
 *     return null;
 *   },
 *   submit: async () => {
 *     return await apiClient.createResource({ name });
 *   },
 *   successPath: '/resources'
 * }, formState, (newState) => formState = newState);
 * ```
 */
export function createFormSubmit<T>(
	options: FormSubmitOptions<T>,
	state: FormSubmitState,
	setState: (state: FormSubmitState) => void
): FormSubmitActions {
	const handleSubmit = async (): Promise<void> => {
		// Clear previous error
		setState({ ...state, error: null });

		// Validate form
		const validationError = options.validate();
		if (validationError) {
			setState({ ...state, error: validationError, isSubmitting: false });
			return;
		}

		// Set submitting state
		setState({ ...state, error: null, isSubmitting: true });

		try {
			const result = await options.submit();

			// Call success callback if provided
			if (options.onSuccess) {
				options.onSuccess(result);
			}

			// Navigate to success path
			goto(options.successPath);
		} catch (e) {
			console.error('Form submission failed:', e);
			const errorMessage = e instanceof Error ? e.message : 'An unexpected error occurred';

			// Call error callback if provided
			if (options.onError) {
				options.onError(e instanceof Error ? e : new Error(errorMessage));
			}

			setState({ ...state, error: errorMessage, isSubmitting: false });
		}
	};

	const handleCancel = (): void => {
		goto(options.successPath);
	};

	const clearError = (): void => {
		setState({ ...state, error: null });
	};

	const setError = (message: string): void => {
		setState({ ...state, error: message });
	};

	return {
		handleSubmit,
		handleCancel,
		clearError,
		setError
	};
}

/**
 * Simplified version for Svelte 5 runes - returns initial state and actions
 *
 * @example
 * ```typescript
 * let { state, actions } = useFormSubmit({
 *   validate: () => !name ? 'Name is required' : null,
 *   submit: () => apiClient.createResource({ name }),
 *   successPath: '/resources'
 * });
 *
 * // In template:
 * <ErrorAlert message={state.error} />
 * <FormActions isSubmitting={state.isSubmitting} onSubmit={actions.handleSubmit} ... />
 * ```
 */
export function useFormSubmit<T>(options: FormSubmitOptions<T>) {
	// Create reactive state
	let state = $state<FormSubmitState>({
		error: null,
		isSubmitting: false
	});

	const updateState = (newState: FormSubmitState) => {
		state = newState;
	};

	const actions = createFormSubmit(options, state, updateState);

	return {
		get state() { return state; },
		actions
	};
}
