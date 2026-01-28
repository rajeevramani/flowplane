# FilterOverridePanel Component

A reusable Svelte component for managing per-route filter overrides in the route edit page.

## Overview

The FilterOverridePanel component handles the 3-level filter hierarchy in flowplane:
- **Route Config** (top level)
- **Virtual Host** (middle level)
- **Route** (specific route level)

This component allows users to override filter settings at the route level, even when the filter is inherited from a parent level (route config or virtual host).

## Features

- **Visual indicators**: Type badges, inheritance status, override status
- **Three override behaviors**:
  - `use_base`: Apply the inherited filter configuration
  - `disable`: Skip this filter for this route
  - `override`: Use custom configuration for this route
- **Dynamic forms**: Automatically generates appropriate configuration forms based on filter type
- **Support for different override modes**:
  - `full_config`: Complete configuration override (e.g., rate limit)
  - `reference_only`: Reference to a named requirement (e.g., JWT auth)
  - `disable_only`: Can only disable, not configure (e.g., some filters)
  - `not_supported`: No per-route override available

## Props

```typescript
interface Props {
  /** The filter being configured */
  filter: FilterResponse;

  /** Schema and UI hints for the filter */
  filterTypeInfo: FilterTypeInfo;

  /** Current override settings (null if using base config) */
  settings: PerRouteSettings | null;

  /** Whether filter is inherited from parent level */
  isInherited?: boolean;

  /** Callback when settings change */
  onSettingsChange: (settings: PerRouteSettings | null) => void;

  /** Callback when user wants to detach/remove the filter */
  onRemove?: () => void;
}
```

## Usage Example

### Basic Usage

```svelte
<script lang="ts">
  import { FilterOverridePanel } from '$lib/components/filters';
  import type { FilterResponse, FilterTypeInfo, PerRouteSettings } from '$lib/api/types';

  let filter: FilterResponse = {
    id: 'filter-123',
    name: 'My Rate Limit',
    filterType: 'local_rate_limit',
    description: 'Protect API from abuse',
    config: { /* ... */ },
    // ... other fields
  };

  let filterTypeInfo: FilterTypeInfo = {
    name: 'local_rate_limit',
    displayName: 'Local Rate Limit',
    perRouteBehavior: 'full_config',
    configSchema: { /* JSON Schema */ },
    // ... other fields
  };

  let settings: PerRouteSettings | null = null;

  function handleSettingsChange(newSettings: PerRouteSettings | null) {
    settings = newSettings;
    // Save to backend
    console.log('New settings:', newSettings);
  }

  function handleRemove() {
    // Detach filter from route
    console.log('Remove filter');
  }
</script>

<FilterOverridePanel
  {filter}
  {filterTypeInfo}
  {settings}
  isInherited={false}
  onSettingsChange={handleSettingsChange}
  onRemove={handleRemove}
/>
```

### With Inherited Filter

```svelte
<FilterOverridePanel
  {filter}
  {filterTypeInfo}
  {settings}
  isInherited={true}
  onSettingsChange={handleSettingsChange}
  {onRemove}
/>
```

When `isInherited={true}`:
- Shows "inherited" badge
- Hides "Remove" button (can't remove inherited filters)
- Shows "Override" button to create route-level override

### With Active Override

```svelte
<script>
  let settings = {
    behavior: 'override',
    config: {
      stat_prefix: 'custom_rate_limit',
      token_bucket: {
        max_tokens: 100,
        fill_interval_ms: 1000
      }
    }
  };
</script>

<FilterOverridePanel
  {filter}
  {filterTypeInfo}
  {settings}
  onSettingsChange={handleSettingsChange}
  onRemove={handleRemove}
/>
```

When settings are active:
- Shows "overridden" badge
- Override panel is expanded by default
- Shows "Reset to default" button

## Behavior Examples

### 1. Use Base Configuration

```typescript
// Settings = null means "use base"
onSettingsChange(null);
```

This applies the inherited configuration from route config or virtual host.

### 2. Disable Filter

```typescript
onSettingsChange({ behavior: 'disable' });
```

The filter is skipped entirely for this route.

### 3. Override with Full Config

```typescript
onSettingsChange({
  behavior: 'override',
  config: {
    // Custom filter configuration
    stat_prefix: 'my_custom_limit',
    token_bucket: { max_tokens: 50, fill_interval_ms: 1000 }
  }
});
```

Used for filters with `perRouteBehavior: 'full_config'`.

### 4. Override with Requirement Reference (JWT Auth)

```typescript
onSettingsChange({
  behavior: 'override',
  requirementName: 'admin_only'
});
```

Used for JWT auth filters with `perRouteBehavior: 'reference_only'`.

## Visual Design

### Card Layout

```
┌─────────────────────────────────────────────────────────┐
│ Filter Name [Type Badge] [inherited] [overridable]     │
│                                      [Configure ▼] [×]  │
├─────────────────────────────────────────────────────────┤
│ Override Panel (expandable)                             │
│ ┌─ Route-level Override ──────────── Reset to default ─┐│
│ │                                                       ││
│ │ Behavior:                                             ││
│ │ ○ Use base configuration                              ││
│ │ ○ Disable for this route                              ││
│ │ ● Override configuration                              ││
│ │                                                       ││
│ │ ┌─ Override Configuration ────────────────────────┐  ││
│ │ │ [Dynamic form fields based on filter schema]   │  ││
│ │ └────────────────────────────────────────────────┘  ││
│ └───────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────┘
```

### Badge Colors

- **Filter Type Badges**:
  - Rate Limit: Amber (`bg-amber-100 text-amber-800`)
  - JWT Auth: Green (`bg-green-100 text-green-800`)
  - CORS: Purple (`bg-purple-100 text-purple-800`)
  - Header Mutation: Blue (`bg-blue-100 text-blue-800`)
  - External Auth: Red (`bg-red-100 text-red-800`)
  - MCP: Teal (`bg-teal-100 text-teal-800`)

- **Status Badges**:
  - Inherited: Gray (`bg-gray-100 text-gray-600`)
  - Overridable: Blue (`bg-blue-100 text-blue-600`)
  - Overridden: Orange (`bg-orange-100 text-orange-600`)

### Panel Colors

- **Active Override**: Orange background (`bg-orange-50 border-orange-200`)
- **Inherited (no override)**: Blue background (`bg-blue-50 border-blue-200`)

## Integration with Route Edit Page

```svelte
<script lang="ts">
  import { FilterOverridePanel } from '$lib/components/filters';

  // Fetch filter data
  let routeFilters = [/* filters attached to this route */];
  let inheritedFilters = [/* filters from route config or virtual host */];
  let filterTypes = [/* FilterTypeInfo from API */];
  let perRouteSettings = new Map(); // Map<filterId, PerRouteSettings>

  async function handleSettingsChange(filterId: string, settings: PerRouteSettings | null) {
    // Save to backend
    await api.updateRouteFilterSettings(routeId, filterId, settings);
    perRouteSettings.set(filterId, settings);
  }
</script>

<div class="space-y-4">
  <h3>Route Filters</h3>
  {#each routeFilters as filter}
    <FilterOverridePanel
      {filter}
      filterTypeInfo={filterTypes.find(t => t.name === filter.filterType)}
      settings={perRouteSettings.get(filter.id) || null}
      isInherited={false}
      onSettingsChange={(s) => handleSettingsChange(filter.id, s)}
      onRemove={() => detachFilter(filter.id)}
    />
  {/each}

  <h3>Inherited Filters</h3>
  {#each inheritedFilters as filter}
    <FilterOverridePanel
      {filter}
      filterTypeInfo={filterTypes.find(t => t.name === filter.filterType)}
      settings={perRouteSettings.get(filter.id) || null}
      isInherited={true}
      onSettingsChange={(s) => handleSettingsChange(filter.id, s)}
    />
  {/each}
</div>
```

## Related Components

- **PerRouteSettingsEditor**: Lower-level component for just the settings form
- **DynamicFilterForm**: Used internally for full config overrides
- **FilterAttachmentList**: For displaying lists of filters without override capabilities
- **FilterSelectorModal**: For selecting filters to attach

## TypeScript Types

All types are imported from `$lib/api/types`:

```typescript
import type {
  FilterResponse,
  FilterTypeInfo,
  PerRouteSettings,
  FilterConfigBehavior
} from '$lib/api/types';
```

### PerRouteSettings

```typescript
interface PerRouteSettings {
  behavior: 'use_base' | 'disable' | 'override';
  config?: Record<string, unknown>;      // For full_config overrides
  requirementName?: string;              // For reference_only overrides (JWT)
}
```

### FilterTypeInfo

```typescript
interface FilterTypeInfo {
  name: string;
  displayName: string;
  description: string;
  perRouteBehavior: 'full_config' | 'reference_only' | 'disable_only' | 'not_supported';
  configSchema: JSONSchema7;
  uiHints?: FilterTypeUiHints;
  // ... other fields
}
```

## Testing

Example test scenarios:

1. **Initial state** - No override (settings = null)
2. **Disable filter** - Set behavior to 'disable'
3. **Full override** - Configure with custom settings
4. **Reset override** - Clear settings back to null
5. **Inherited filter** - Show/hide correct buttons
6. **Reference override** - JWT requirement name
7. **Different filter types** - Rate limit, JWT, CORS, etc.
