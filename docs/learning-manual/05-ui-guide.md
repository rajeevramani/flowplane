# Learning Feature UI Guide

This guide provides a walkthrough of the Flowplane Learning Feature user interface.

## Table of Contents

1. [Accessing the Learning Feature](#accessing-the-learning-feature)
2. [Learning Sessions Page](#learning-sessions-page)
3. [Creating a New Session](#creating-a-new-session)
4. [Session Details Page](#session-details-page)
5. [Discovered Schemas Page](#discovered-schemas-page)
6. [Schema Details Page](#schema-details-page)
7. [Exporting Schemas](#exporting-schemas)

---

## Accessing the Learning Feature

The Learning Feature is accessible from the main navigation sidebar under **API Discovery**.

### Navigation Path

1. Log in to the Flowplane UI
2. Locate the left sidebar
3. Find the **API Discovery** section
4. Two options:
   - **Learning Sessions** - Manage traffic capture sessions
   - **Discovered Schemas** - Browse and export learned schemas

---

## Learning Sessions Page

**Route:** `/learning`

### Stats Cards Overview

Four summary cards at the top:

| Card | Icon | Description |
|------|------|-------------|
| Total Sessions | BookOpen (Blue) | All learning sessions |
| Active | Play (Blue) | Currently capturing traffic |
| Completed | CheckCircle (Green) | Successfully finished |
| Failed/Cancelled | XCircle (Gray) | Combined count |

### Session Table

| Column | Description |
|--------|-------------|
| Route Pattern | Regex pattern (monospace) + cluster/methods |
| Status | Badge with state (pulsing animation for active) |
| Progress | Progress bar with sample counts |
| Created | Relative time (e.g., "2h ago") |
| Actions | View (Eye) / Cancel (XCircle) |

### Search and Filter

**Search Bar:**
- Search by route pattern, session ID, or cluster name
- Case-insensitive matching

**Status Filter (dropdown):**
- All Statuses
- Pending / Active / Completing / Completed / Cancelled / Failed

### Auto-Polling

When active sessions exist:
- Refreshes every 5 seconds
- Shows "Auto-refreshing..." indicator
- Stops when all sessions are terminal

### Create Session Button

- Top left, next to page title
- Blue button with Plus icon
- Requires `write:learning-sessions` permission

---

## Creating a New Session

**Route:** `/learning/create`

### Form Sections

#### 1. Traffic Matching

**Route Pattern (Required):**
- Text field with monospace font
- Real-time regex validation
- Examples: `^/api/.*`, `^/users/[0-9]+`

**Cluster Name (Optional):**
- Only capture traffic to this cluster

**HTTP Methods (Optional):**
- Multi-select button group
- GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
- Leave empty to capture all methods

#### 2. Session Configuration

**Target Sample Count (Required):**
- Default: 100
- Range: 1 to 100,000

**Maximum Duration (Optional):**
- Minimum: 60 seconds
- Session times out after this duration

#### 3. Metadata (Optional)

**Triggered By:**
- Track what initiated the session

**Deployment Version:**
- Associate with a specific API version

### Action Buttons

- **Cancel**: Returns to `/learning`
- **Create Session**: Submits form, redirects to session details on success

### Validation

- Route pattern must be valid regex
- Target sample count: 1-100,000
- Write permission required

---

## Session Details Page

**Route:** `/learning/{sessionId}`

### Page Header

- **Title**: "Learning Session"
- **Status Badge**: Large badge showing current state
- **Session ID**: Monospace, gray text
- **Cancel Button**: For Active/Pending sessions

### Auto-Refresh Indicator

For active/completing sessions:
- Blue banner with spinning RefreshCw icon
- "Auto-refreshing every 3 seconds..."

### Progress Card

**Progress Bar:**
- Shows "X / Y samples - Z%"
- Color coding: Blue (0-75%), Green (100%)
- Pulse animation when active

**Error Message (if failed):**
- Red alert box with error details

### Details Grid

**Left Column - Traffic Matching:**
- Route Pattern (monospace, gray background)
- Cluster (if specified)
- HTTP Methods (badge pills or "All methods")

**Right Column - Timeline:**
- Created (Calendar icon)
- Started (Play icon, blue)
- Duration (Clock icon)
- Timeout (AlertTriangle, yellow)
- Completed (CheckCircle, green)

### Metadata Section

Displayed if `triggeredBy` or `deploymentVersion` was specified.

---

## Discovered Schemas Page

**Route:** `/learning/schemas`

### Stats Cards

| Card | Icon | Description |
|------|------|-------------|
| Total Schemas | FileCode (Blue) | All discovered schemas |
| High Confidence (90%+) | FileCode (Green) | Confidence ≥ 0.9 |
| With Breaking Changes | AlertTriangle (Orange) | Has breaking changes |

### Filters Row

**Search Bar:**
- Search by API path
- Case-insensitive substring match

**Method Filter (dropdown):**
- All Methods / GET / POST / PUT / DELETE / PATCH

**Export Button:**
- "Export as OpenAPI"
- Opens multi-schema export modal
- Requires `read:schemas` permission

### Schemas Table

| Column | Description |
|--------|-------------|
| Endpoint | API path (monospace) + breaking changes count |
| Method | Colored badge (GET=blue, POST=green, etc.) |
| Confidence | Percentage badge (Green 90%+, Yellow 70-89%, Red <70%) |
| Samples | Total sample count |
| Last Observed | Date of most recent sample |
| Actions | View (Eye) / Export (Download) |

### Breaking Changes Indicator

- Orange text with AlertTriangle icon
- Shows "N breaking change(s)"

---

## Schema Details Page

**Route:** `/learning/schemas/{schemaId}`

### Page Header

- **Method Badge**: Colored badge
- **Path**: Large, bold monospace
- **Metadata**: "Version {version} | Team: {team}"
- **Export Button**: "Export OpenAPI"

### Stats Cards

| Card | Description |
|------|-------------|
| Confidence | Percentage with color coding |
| Sample Count | Total samples captured |
| Version | Schema version number |
| Breaking Changes | Count or "None" |

### Breaking Changes Alert

If breaking changes exist:
- Orange tinted box with AlertTriangle
- Lists each change with type and path

### Schema Tabs

**Tab 1: Request Schema**
- JSON formatted in code block
- "No request schema captured" if empty

**Tab 2: Response Schemas**
- One section per status code
- Status code badge + JSON schema

**Tab 3: Version Compare**
- Only if `previousVersionId` exists
- Shows differences summary
- Breaking changes section

### Timeline Metadata

- First Observed
- Last Observed
- Last Updated

---

## Exporting Schemas

### Single Schema Export

**From Schemas List:**
1. Find schema in table
2. Click Download icon
3. File downloads as `{path}_{method}.openapi.json`

**From Schema Details:**
1. Navigate to schema details
2. Click "Export OpenAPI" button
3. Same filename convention

### Multi-Schema Export (Bulk)

**Opening the Modal:**
1. Go to `/learning/schemas`
2. Click "Export as OpenAPI" button

**Schema Selection:**
- Select All checkbox
- Individual schema checkboxes
- Shows method badge, path, sample count

**Export Options:**
- **Title** (required): OpenAPI info.title
- **Version** (required): OpenAPI info.version
- **Description** (optional): OpenAPI info.description
- **Include Metadata**: x-flowplane-* extensions

**Footer:**
- Cancel button
- "Export N Schema(s)" button

### OpenAPI Export Format

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "User-specified title",
    "version": "User-specified version",
    "description": "Optional description"
  },
  "paths": {...},
  "components": {"schemas": {}}
}
```

**Flowplane Extensions (if enabled):**
- `x-flowplane-confidence`
- `x-flowplane-sample-count`
- `x-flowplane-first-observed`
- `x-flowplane-last-observed`

---

## Visual Design Elements

### Status Badges

| Status | Background | Animation |
|--------|-----------|-----------|
| Pending | Gray | None |
| Active | Blue | Pulsing |
| Completing | Yellow | Pulsing |
| Completed | Green | None |
| Cancelled | Gray | None |
| Failed | Red | None |

### Progress Bars

**Color Progression:**
- 0-50%: Light blue
- 50-75%: Medium blue
- 75-100%: Blue
- 100%: Green

**Features:**
- Smooth transition animation
- Rounded corners
- Optional pulse animation

### HTTP Method Badges

| Method | Color |
|--------|-------|
| GET | Blue |
| POST | Green |
| PUT | Yellow |
| DELETE | Red |
| PATCH | Purple |
| HEAD/OPTIONS | Gray |

### Confidence Score Colors

- 90%+: Green
- 70-89%: Yellow
- <70%: Red

---

## Permissions and Access Control

### Learning Sessions

| Action | Required Scope |
|--------|----------------|
| View | `learning-sessions:read` |
| Create | `learning-sessions:write` |
| Cancel | `learning-sessions:delete` |

### Schemas

| Action | Required Scope |
|--------|----------------|
| View | `schemas:read` |
| Export | `schemas:read` |

### Permission Denied Behavior

- Create/Cancel buttons: Hidden (not disabled)
- Export buttons: Disabled or hidden
- Error messages: "You don't have permission to..."

---

## Tips and Best Practices

### Creating Effective Sessions

1. Start with broad patterns, then narrow down
2. Use reasonable sample counts (100-500)
3. Set timeouts for long-running sessions
4. Add metadata for CI/CD tracking

### Monitoring Sessions

1. Watch auto-refresh indicator
2. Check progress bars for movement
3. Review error messages immediately
4. Cancel stuck sessions

### Working with Schemas

1. Focus on high confidence (90%+)
2. Investigate breaking changes
3. Compare versions for evolution
4. Export regularly for snapshots

### Exporting

1. Single exports for quick testing
2. Bulk exports for comprehensive specs
3. Use descriptive titles/versions
4. Include metadata for traceability

---

## Troubleshooting UI Issues

### Session Not Progressing

- Verify auto-refresh is active (spinning icon)
- Check browser console for errors
- Verify session hasn't timed out

### Cannot Create Session

- Check `write:learning-sessions` permission
- Ensure route pattern is valid regex
- Verify all required fields filled

### Export Button Disabled

- Confirm `read:schemas` permission
- For bulk: Select at least one schema
- For bulk: Fill title and version fields

### Schemas Not Appearing

- Verify sessions completed successfully
- Check sessions captured samples
- Ensure correct team selected
