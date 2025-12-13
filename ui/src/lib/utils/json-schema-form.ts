/**
 * JSON Schema to Form Field Generator
 *
 * Converts JSON Schema definitions into form field configurations
 * that can be used to dynamically render filter configuration forms.
 */

import type { JSONSchema7, FilterTypeUiHints, FilterTypeFormSection } from '$lib/api/types';

/** Supported form field types */
export type FormFieldType = 'string' | 'number' | 'integer' | 'boolean' | 'object' | 'array' | 'enum' | 'unknown';

/** Validation rules for a form field */
export interface ValidationRule {
	type: 'required' | 'minLength' | 'maxLength' | 'minimum' | 'maximum' | 'pattern';
	value?: string | number;
	message: string;
}

/** Select option for enum fields */
export interface SelectOption {
	value: string | number | boolean | null;
	label: string;
}

/** Form field definition generated from JSON Schema */
export interface FormField {
	/** Field name (property key in the schema) */
	name: string;
	/** Display label for the field */
	label: string;
	/** Field type */
	type: FormFieldType;
	/** Optional description */
	description?: string;
	/** Whether the field is required */
	required: boolean;
	/** Default value if any */
	defaultValue?: unknown;
	/** Validation rules */
	validation: ValidationRule[];
	/** For enum fields, the available options */
	options?: SelectOption[];
	/** For object fields, nested form fields */
	nested?: FormField[];
	/** For array fields, the item schema */
	itemSchema?: FormField;
	/** Original JSON Schema (for complex cases) */
	originalSchema: JSONSchema7;
	/** UI hint: placeholder text */
	placeholder?: string;
	/** UI hint: input format (e.g., 'uri', 'email') */
	format?: string;
}

/** Form section for grouped fields */
export interface FormSection {
	name: string;
	fields: FormField[];
	collapsible: boolean;
	collapsedByDefault: boolean;
}

/** Complete form configuration */
export interface FormConfig {
	layout: 'flat' | 'sections' | 'tabs';
	sections: FormSection[];
	allFields: FormField[];
}

/**
 * Convert a property name to a human-readable label
 */
function toLabel(name: string): string {
	return name
		.replace(/_/g, ' ')
		.replace(/([A-Z])/g, ' $1')
		.replace(/^./, (str) => str.toUpperCase())
		.trim();
}

/**
 * Extract validation rules from a JSON Schema
 */
function extractValidationRules(schema: JSONSchema7, required: boolean): ValidationRule[] {
	const rules: ValidationRule[] = [];

	if (required) {
		rules.push({
			type: 'required',
			message: 'This field is required'
		});
	}

	if (schema.minLength !== undefined) {
		rules.push({
			type: 'minLength',
			value: schema.minLength,
			message: `Minimum length is ${schema.minLength}`
		});
	}

	if (schema.maxLength !== undefined) {
		rules.push({
			type: 'maxLength',
			value: schema.maxLength,
			message: `Maximum length is ${schema.maxLength}`
		});
	}

	if (schema.minimum !== undefined) {
		rules.push({
			type: 'minimum',
			value: schema.minimum,
			message: `Minimum value is ${schema.minimum}`
		});
	}

	if (schema.maximum !== undefined) {
		rules.push({
			type: 'maximum',
			value: schema.maximum,
			message: `Maximum value is ${schema.maximum}`
		});
	}

	if (schema.pattern) {
		rules.push({
			type: 'pattern',
			value: schema.pattern,
			message: `Must match pattern: ${schema.pattern}`
		});
	}

	return rules;
}

/**
 * Determine the form field type from a JSON Schema type
 */
function determineFieldType(schema: JSONSchema7): FormFieldType {
	if (schema.enum) {
		return 'enum';
	}

	const schemaType = Array.isArray(schema.type) ? schema.type[0] : schema.type;

	switch (schemaType) {
		case 'string':
			return 'string';
		case 'number':
			return 'number';
		case 'integer':
			return 'integer';
		case 'boolean':
			return 'boolean';
		case 'object':
			return 'object';
		case 'array':
			return 'array';
		default:
			return 'unknown';
	}
}

/**
 * Convert enum values to select options
 */
function enumToOptions(enumValues: (string | number | boolean | null)[]): SelectOption[] {
	return enumValues.map((value) => ({
		value,
		label: value === null ? '(None)' : toLabel(String(value))
	}));
}

/**
 * Generate a form field from a JSON Schema property
 */
function schemaToFormField(
	name: string,
	schema: JSONSchema7,
	requiredFields: string[]
): FormField {
	const isRequired = requiredFields.includes(name);
	const fieldType = determineFieldType(schema);

	const field: FormField = {
		name,
		label: schema.title || toLabel(name),
		type: fieldType,
		description: schema.description,
		required: isRequired,
		defaultValue: schema.default,
		validation: extractValidationRules(schema, isRequired),
		originalSchema: schema,
		format: schema.format
	};

	// Handle enum fields
	if (schema.enum) {
		field.options = enumToOptions(schema.enum);
	}

	// Handle object fields - recursively generate nested fields
	if (fieldType === 'object' && schema.properties) {
		field.nested = Object.entries(schema.properties).map(([propName, propSchema]) =>
			schemaToFormField(propName, propSchema as JSONSchema7, schema.required || [])
		);
	}

	// Handle array fields - generate item schema
	if (fieldType === 'array' && schema.items) {
		const itemSchema = schema.items as JSONSchema7;
		field.itemSchema = schemaToFormField('item', itemSchema, []);
	}

	return field;
}

/**
 * Generate form fields from a JSON Schema
 */
export function generateFormFields(schema: JSONSchema7): FormField[] {
	if (schema.type !== 'object' || !schema.properties) {
		return [];
	}

	const requiredFields = schema.required || [];

	return Object.entries(schema.properties).map(([name, propSchema]) =>
		schemaToFormField(name, propSchema as JSONSchema7, requiredFields)
	);
}

/**
 * Get a nested field by dot-notation path (e.g., "token_endpoint.uri")
 */
export function getFieldByPath(fields: FormField[], path: string): FormField | undefined {
	const parts = path.split('.');
	if (parts.length === 1) {
		return fields.find((f) => f.name === path);
	}

	// Find the top-level field
	const topLevelName = parts[0];
	const topLevelField = fields.find((f) => f.name === topLevelName);
	if (!topLevelField || !topLevelField.nested) {
		return undefined;
	}

	// Recursively search nested fields
	const remainingPath = parts.slice(1).join('.');
	return getFieldByPath(topLevelField.nested, remainingPath);
}

/**
 * Organize form fields into sections based on UI hints
 * Supports dot-notation paths for nested fields (e.g., "token_endpoint.uri")
 */
function organizeFieldsIntoSections(
	fields: FormField[],
	uiHints?: FilterTypeUiHints
): FormSection[] {
	if (!uiHints || !uiHints.sections || uiHints.sections.length === 0) {
		// No sections defined - put all fields in a single section
		return [
			{
				name: 'Configuration',
				fields,
				collapsible: false,
				collapsedByDefault: false
			}
		];
	}

	const usedTopLevelFields = new Set<string>();

	const sections: FormSection[] = uiHints.sections.map((section: FilterTypeFormSection) => {
		const sectionFields = section.fields
			.map((fieldPath: string) => {
				// Track top-level field usage for "Other" section
				const topLevelName = fieldPath.split('.')[0];
				usedTopLevelFields.add(topLevelName);

				// Get field by path (supports nested paths)
				return getFieldByPath(fields, fieldPath);
			})
			.filter((f): f is FormField => f !== undefined);

		return {
			name: section.name,
			fields: sectionFields,
			collapsible: section.collapsible,
			collapsedByDefault: section.collapsedByDefault
		};
	});

	// Add any top-level fields not assigned to a section
	const unassignedFields = fields.filter((f) => !usedTopLevelFields.has(f.name));
	if (unassignedFields.length > 0) {
		sections.push({
			name: 'Other',
			fields: unassignedFields,
			collapsible: true,
			collapsedByDefault: true
		});
	}

	return sections;
}

/**
 * Generate a complete form configuration from a JSON Schema and UI hints
 */
export function generateForm(schema: JSONSchema7, uiHints?: FilterTypeUiHints): FormConfig {
	const allFields = generateFormFields(schema);
	const layout = uiHints?.formLayout || 'flat';
	const sections = organizeFieldsIntoSections(allFields, uiHints);

	return {
		layout,
		sections,
		allFields
	};
}

/**
 * Get the initial value for a form field based on its schema
 */
export function getFieldDefaultValue(field: FormField): unknown {
	if (field.defaultValue !== undefined) {
		return field.defaultValue;
	}

	switch (field.type) {
		case 'string':
			return '';
		case 'number':
		case 'integer':
			return 0;
		case 'boolean':
			return false;
		case 'object':
			if (field.nested) {
				const obj: Record<string, unknown> = {};
				for (const nestedField of field.nested) {
					obj[nestedField.name] = getFieldDefaultValue(nestedField);
				}
				return obj;
			}
			return {};
		case 'array':
			return [];
		case 'enum':
			return field.options?.[0]?.value ?? null;
		default:
			return null;
	}
}

/**
 * Generate default values for all fields in a form
 */
export function generateDefaultValues(fields: FormField[]): Record<string, unknown> {
	const values: Record<string, unknown> = {};
	for (const field of fields) {
		values[field.name] = getFieldDefaultValue(field);
	}
	return values;
}

/**
 * Validate a value against a form field's validation rules
 */
export function validateField(field: FormField, value: unknown): string[] {
	const errors: string[] = [];

	for (const rule of field.validation) {
		switch (rule.type) {
			case 'required':
				if (value === undefined || value === null || value === '') {
					errors.push(rule.message);
				}
				break;
			case 'minLength':
				if (typeof value === 'string' && value.length < (rule.value as number)) {
					errors.push(rule.message);
				}
				break;
			case 'maxLength':
				if (typeof value === 'string' && value.length > (rule.value as number)) {
					errors.push(rule.message);
				}
				break;
			case 'minimum':
				if (typeof value === 'number' && value < (rule.value as number)) {
					errors.push(rule.message);
				}
				break;
			case 'maximum':
				if (typeof value === 'number' && value > (rule.value as number)) {
					errors.push(rule.message);
				}
				break;
			case 'pattern':
				if (typeof value === 'string' && !new RegExp(rule.value as string).test(value)) {
					errors.push(rule.message);
				}
				break;
		}
	}

	return errors;
}

/**
 * Validate all fields in a form
 */
export function validateForm(
	fields: FormField[],
	values: Record<string, unknown>
): Record<string, string[]> {
	const errors: Record<string, string[]> = {};

	for (const field of fields) {
		const fieldErrors = validateField(field, values[field.name]);
		if (fieldErrors.length > 0) {
			errors[field.name] = fieldErrors;
		}

		// Validate nested fields for objects
		if (field.type === 'object' && field.nested && values[field.name]) {
			const nestedErrors = validateForm(
				field.nested,
				values[field.name] as Record<string, unknown>
			);
			for (const [nestedName, nestedFieldErrors] of Object.entries(nestedErrors)) {
				errors[`${field.name}.${nestedName}`] = nestedFieldErrors;
			}
		}
	}

	return errors;
}
