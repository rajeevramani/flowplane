# Zod Schemas

This directory contains Zod validation schemas for form validation across the application.

## Available Schemas

### Authentication (`auth.ts`)
- `loginSchema` - Login form validation
- `bootstrapSchema` - Initial setup form with password confirmation

### Route Configuration (`route-config.ts`)
Comprehensive schemas for route configuration forms:

- `RouteNameSchema` - Reusable identifier validation (lowercase, alphanumeric, dashes)
- `PathSchema` - Route path validation (must start with `/`)
- `DomainSchema` - Domain name validation (supports wildcards like `*.example.com`)
- `RouteFormSchema` - Individual route configuration
- `VirtualHostFormSchema` - Virtual host with domains and routes
- `RouteConfigFormSchema` - Complete route configuration
- `McpConfigSchema` - MCP tool configuration

## Usage with Superforms

```typescript
import { RouteFormSchema, type RouteFormData } from '$lib/schemas/route-config';
import { superForm } from 'sveltekit-superforms/client';
import { zod } from 'sveltekit-superforms/adapters';

// Initialize form with validation
const { form, errors, enhance } = superForm<RouteFormData>(
  initialData,
  {
    validators: zod(RouteFormSchema)
  }
);
```

## Key Features

1. **User-Friendly Error Messages** - All schemas include clear, actionable error messages
2. **Cross-Field Validation** - Uses `.refine()` for complex validation rules
3. **Data Transformation** - Automatically trims strings and normalizes input
4. **Type Safety** - Full TypeScript support with inferred types
5. **Reusable Components** - Shared schemas like `RouteNameSchema` for consistency

## Validation Rules

### Route Names
- Lowercase only
- Alphanumeric characters and dashes
- Must start and end with alphanumeric character
- Pattern: `/^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/`

### Paths
- Must start with `/`
- Non-empty
- Automatically trimmed

### Domains
- Valid domain format (e.g., `example.com`)
- Supports wildcards (e.g., `*.example.com`)
- No duplicates allowed within a virtual host

### Route Configuration
- At least one virtual host required
- Each virtual host needs at least one domain
- Each virtual host needs at least one route
- No duplicate names across resources
- Retry policy requires all related fields when enabled

## Examples

See `route-config.test.ts` for comprehensive usage examples including:
- Basic field validation
- Complex object validation
- Retry policy configuration
- MCP tool setup
- Error handling patterns

## Centralized Export

Import schemas from the main index file:

```typescript
import {
  RouteFormSchema,
  type RouteFormData
} from '$lib/schemas';
```

## Testing

The schemas include extensive validation logic. Test files demonstrate:
- Valid and invalid input examples
- Error message verification
- Type inference usage
- Integration with Superforms
