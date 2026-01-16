# Filter Modal Bug Fix

## Issue
No filters were appearing in the "Available Filters" section of the FilterSelectorModal when opened from the route-config edit page, even though filters existed in the system.

## Root Cause
In `/Users/rajeevramani/workspace/projects/flowplane/ui/src/routes/(authenticated)/route-configs/[id]/edit/+page.svelte`:

**Line 371-389**: The `attachedFilterIds` variable was defined as a **derived function**:
```typescript
let attachedFilterIds = $derived(() => {
    // ... returns array of filter IDs
});
```

This Svelte 5 runes pattern creates a **function** that needs to be called with `()` to get the actual array value.

**Line 1036** (before fix): The modal was invoked with:
```svelte
<FilterSelectorModal
    alreadyAttachedIds={attachedFilterIds}  // Passes the FUNCTION
    ...
/>
```

This passed the **function itself** instead of calling it to get the array.

## Impact
In `FilterSelectorModal.svelte`:
- **Line 87**: `alreadyAttachedIds.includes(filter.id)` tried to call `.includes()` on a **function** instead of an array
- This caused all filters to be incorrectly categorized as "incompatible"
- The categorization logic at lines 71-109 failed silently

## Fix
Changed line 1036 to call the derived function:
```svelte
<FilterSelectorModal
    alreadyAttachedIds={attachedFilterIds()}  // Call the function to get the array
    ...
/>
```

## Additional Changes
Added debug logging to help diagnose similar issues in the future:
- `handleOpenConfigFilterModal()` - logs available filters and attached IDs
- `handleOpenVirtualHostFilterModal()` - logs VH filter context
- `handleOpenRouteFilterModal()` - logs route filter context

## Svelte 5 Pattern Note
In Svelte 5 runes:
- `$derived(expression)` - creates a reactive value (access directly)
- `$derived(() => { ... })` - creates a reactive **function** (must call with `()`)

When passing derived functions as props, always call them with `()` to get the actual value.

## Testing
To verify the fix:
1. Navigate to route-configs edit page
2. Click "Configure Filter" button
3. Verify filters appear in "Available Filters" section
4. Check browser console for debug logs showing filter counts
5. Verify filters can be selected and attached

## Related Files
- `/Users/rajeevramani/workspace/projects/flowplane/ui/src/routes/(authenticated)/route-configs/[id]/edit/+page.svelte`
- `/Users/rajeevramani/workspace/projects/flowplane/ui/src/lib/components/filters/FilterSelectorModal.svelte`
- `/Users/rajeevramani/workspace/projects/flowplane/ui/src/lib/utils/filter-attachment.ts`
