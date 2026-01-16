# Route Edit Page - Filter Override Implementation

## Overview
Updated the route edit page (`+page.svelte`) to properly implement filter overrides using the existing `FilterOverridePanel` component through the `RouteFilterCard` wrapper.

## Changes Made

### 1. Component Import Update
**Before:**
```typescript
import { FilterOverridePanel } from '$lib/components/filters';
```

**After:**
```typescript
import { RouteFilterCard } from '$lib/components/filters';
```

Changed to use `RouteFilterCard` which is the proper wrapper component that:
- Lazy loads `FilterTypeInfo` from the API
- Handles loading states and error handling
- Wraps `FilterOverridePanel` with the loaded data
- Manages the complete lifecycle of filter configuration

### 2. Filter List Rendering
**Before:** Simple list items showing filter name and type with a basic "Remove" button

**After:** Full `RouteFilterCard` components for each filter

#### Direct Filters (attached at route level)
```svelte
<RouteFilterCard
  {filter}
  routeConfigName={routeData.config.name}
  virtualHostName={routeData.virtualHost.name}
  routeName={routeData.route.name}
  isInherited={false}
  onRemove={() => detachFilter(filter.id)}
  onSettingsUpdate={() => loadRouteFilters(routeData)}
/>
```

#### Inherited Filters (from VirtualHost or RouteConfig)
```svelte
<RouteFilterCard
  {filter}
  routeConfigName={routeData.config.name}
  virtualHostName={routeData.virtualHost.name}
  routeName={routeData.route.name}
  isInherited={true}
  onSettingsUpdate={() => loadRouteFilters(routeData)}
/>
```
Note: No `onRemove` prop for inherited filters - they cannot be detached, only overridden.

### 3. Removed Unused Code
- Removed `getFilterTypeColor()` function (no longer needed as `RouteFilterCard` handles badge colors)

## Features Now Available

### For Direct Filters (attached at route level)
- **Green indicator** showing it's a direct attachment
- **Configure button** to modify filter settings
- **Remove button** to detach the filter from this route
- **Override options** based on `perRouteBehavior`:
  - `full_config`: Full configuration form
  - `reference_only`: Requirement name field only
  - `disable_only`: Can only use base or disable
  - `not_supported`: Read-only, no override options

### For Inherited Filters (from VH or RouteConfig)
- **Blue indicator** showing it's inherited
- **"inherited" badge** for clear visual distinction
- **Override button** to create route-level overrides (if supported)
- **Configure button** to view/edit override settings
- No remove option (cannot detach inherited filters)

### Override Behaviors
Each filter can have one of three behaviors at the route level:

1. **use_base** (default)
   - Uses the inherited filter configuration
   - No route-level customization
   - Visual: No "overridden" badge

2. **disable**
   - Skips this filter for requests matching this route
   - Available for most filter types
   - Visual: "overridden" badge shown

3. **override**
   - Uses custom configuration for this route
   - Form varies by `perRouteBehavior`:
     - `full_config`: Complete filter configuration form
     - `reference_only`: Just a requirement name field
   - Visual: "overridden" badge shown

## Visual Indicators

### Badges
- **Filter type** (e.g., "JWT Auth", "Rate Limit") - color-coded by type
- **inherited** - gray badge for inherited filters
- **overridable** - blue badge when filter supports route-level overrides
- **overridden** - orange badge when an active override exists (behavior != 'use_base')

### Status Indicators
- **Direct filters**: No special indicator (can be configured or removed)
- **Inherited filters**: "inherited" badge (can only be overridden, not removed)

## API Integration

The implementation uses these API methods:
- `apiClient.getFilterType(filterType)` - Get `FilterTypeInfo` with `perRouteBehavior`
- `apiClient.configureFilter(filterId, { scopeType: 'route', scopeId, settings })` - Save override settings
- `apiClient.removeFilterConfiguration(filterId, 'route', scopeId)` - Remove override (reset to use_base)
- `apiClient.listFilterConfigurations(filterId)` - Get current settings for all scopes

## Component Hierarchy

```
+page.svelte
  └── RouteFilterCard (wrapper component)
        ├── Loads FilterTypeInfo via API
        ├── Loads PerRouteSettings via API
        ├── Manages loading/saving states
        └── FilterOverridePanel (UI component)
              ├── Displays filter info with badges
              ├── Shows behavior selection (use_base/disable/override)
              └── DynamicFilterForm (for full_config overrides)
```

## Testing Notes

To test the implementation:

1. **Direct filters**: Attach a filter to a route, click "Configure" to override settings
2. **Inherited filters**: View a route that inherits filters from VH or RouteConfig, click "Override" to create route-level customization
3. **Different filter types**: Test with filters that have different `perRouteBehavior` values
4. **Reset functionality**: Override a filter, then click "Reset to default" to verify it returns to use_base

## Benefits of This Implementation

1. **Consistent UX**: Same filter override UI across route edit and other pages
2. **Lazy loading**: Filter type info and settings only load when needed (on expand)
3. **Error handling**: Comprehensive error states with retry options
4. **Type safety**: Full TypeScript types for all props and callbacks
5. **Reusability**: Uses existing, tested components instead of duplicating logic
