/**
 * Filter Form Registry
 *
 * All filters now use dynamic forms generated from JSON Schema.
 * Custom form components are no longer needed.
 *
 * This registry is kept for backward compatibility but returns empty/false
 * to ensure all filters use DynamicFilterForm.
 */

/**
 * Registry of custom form components for specific filter types.
 * Empty - all filters now use DynamicFilterForm.
 */
export const CUSTOM_FORM_REGISTRY: Record<string, never> = {};

/**
 * Check if a filter type has a custom form component.
 * Always returns false - all filters now use dynamic forms.
 */
export function hasCustomForm(_filterType: string): boolean {
	return false;
}

/**
 * Get the custom form component for a filter type.
 * Always returns null - all filters now use dynamic forms.
 */
export function getCustomForm(_filterType: string): null {
	return null;
}

/**
 * List all filter types that have custom forms.
 * Returns empty array - all filters now use dynamic forms.
 */
export function listCustomFormTypes(): string[] {
	return [];
}
