/**
 * Filter Form Registry
 *
 * Maps filter types to custom form components. When a custom form is
 * registered for a filter type, the create/edit pages render it instead
 * of the generic DynamicFilterForm.
 *
 * Custom forms provide better UX for complex filter types with
 * field-level inputs, contextual help, and Zod validation.
 */

/**
 * Filter types that have custom form components.
 * The create/edit pages check this to decide whether to render
 * a custom form or the generic DynamicFilterForm.
 */
export const CUSTOM_FORM_FILTER_TYPES = new Set([
	'cors',
	'rate_limit'
]);

/**
 * Check if a filter type has a custom form component.
 */
export function hasCustomForm(filterType: string): boolean {
	return CUSTOM_FORM_FILTER_TYPES.has(filterType);
}
