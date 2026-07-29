# Dashboard UI framework

The local dashboard is a read-only server-rendered application. Its UI is built from shared presentation and interaction primitives; feature names belong in data and behavior hooks, not in reusable visual rules.

## Layers

1. **Design tokens and foundations** live in `assets/dashboard.css`: color, typography, spacing, focus, surfaces, and responsive behavior.
2. **Reusable presentation primitives** are composed by every template. Their class names describe a UI role, never a dashboard page or API collection.
3. **Shared DOM helpers** live in `assets/ui.js`. They own reusable tab state and filtering/pagination arithmetic without acquiring data.
4. **Feature adapters** such as `resources.js` and `apis.js` may own domain behavior—topology expansion, cluster highlighting, or API master/detail loading—but must call shared helpers for generic interaction behavior.
5. **Network acquisition belongs to htmx.** Dashboard scripts must not use `fetch`, credentials, storage, HTML string injection, or inline event handlers.

## Shell

All seven pages include `templates/dashboard/_shell.html`. A page sets `active_tab` before including it. Header, context chips, navigation order, links, and active-state rendering must not be copied into page templates.

## Core primitives

### Existing stable components

- `.topbar`, `.tabs`, `.chip`: application shell.
- `.cards`, `.card`: responsive summary-card grid.
- `.panel`, `.panel-body`, `.panel-subheading`: bordered content container, inset body, and explicit nested heading.
- `.tablewrap`: horizontal overflow owner for every table.
- `.pill` plus semantic variants: compact status vocabulary.
- `.banner`: shared notice treatment.
- `.placeholder`, `.hint`, `.muted`, `.mono`: text roles.

### Generic compositional primitives

- `.ui-stack`: vertical flow.
- `.ui-inline`: wrapping inline metadata.
- `.ui-panel--clip`: panel overflow clipping for composed content.
- `.ui-segmented`, `.ui-view`: tab-like view switch and panel.
- `.ui-toolbar`, `.ui-toolbar--bottom`, `.ui-search`, `.ui-button-group`: collection controls.
- `.ui-button`, `.ui-button--panel`: secondary actions.
- `.ui-pager`, `.ui-range`: pagination controls and range.
- `.ui-empty-state`: intentionally empty collection or filtered result.
- `.ui-disclosure-list`, `.ui-disclosure-row`, `.ui-disclosure-summary`, `.ui-disclosure-trigger`, `.ui-disclosure-icon`, `.ui-secondary-disclosure`: accessible disclosure composition.
- `.ui-master-table`, `.ui-master-row`, `.ui-master-primary`, `.ui-detail-row`, `.ui-detail-row--indented`, `.ui-detail-content`, `.ui-metadata`: table master/detail composition.
- `.ui-record-list`, `.ui-record-row`, `.ui-record-note`: compact records outside a table.
- `.ui-tree-item`, `.ui-tree-trigger`, `.ui-tree-title`, `.ui-tree-meta`, `.ui-tree-arrow`, `.ui-tree-branch`, `.ui-tree-node`, `.ui-tree-node--nested`, `.ui-tree-children`, `.ui-tree-leaf`, `.ui-tree-leaf-label`: hierarchical content.
- `.ui-entity-chip`, `.ui-entity-chip-name`, `.ui-entity-chip-weight`, `.ui-entity-chip-meta`: rich references inside a hierarchy or record.
- `.ui-timeline`, `.ui-timeline-item`, `.ui-timeline-name`, `.ui-timeline-meta`, `.ui-timeline-outcome`: ordered execution or history timeline.
- `.ui-steps`: compact ordered process or lifecycle steps.
- `.meter`: native progress visualization.

### State modifiers

State modifiers are reusable and may be toggled by adapters: `.is-active`, `.is-open`, `.is-selected`, `.is-highlighted`, `.is-warning`, and `.is-critical`.

A feature adapter may use `data-*` attributes for semantic behavior. CSS must not style a feature-specific data attribute unless it represents a truly unique domain visualization rather than a generic control or state.

## Interaction API

`window.FlowplaneUI` exposes:

- `paginate(items, query, requestedPage, pageSize, searchableText)`: returns matched and visible items plus normalized page/range metadata. It does not mutate the DOM.
- `selectTab(tabs, panels, name, focus)`: applies active, ARIA, tabindex, focus, and hidden state to elements using `data-ui-tab` and `data-ui-tab-panel`.

Adapters remain responsible for domain consequences. For example, the API adapter collapses a detail row when its master row leaves the visible page, while the Resources adapter dispatches the existing htmx-owned lazy-load event when the Tables view is first selected.

## Rules for new UI

1. Search this document and `dashboard.css` before adding a class.
2. Compose existing primitives before creating a new component.
3. Name a new presentation class for its reusable UI role, not its page, endpoint, resource type, or current text.
4. Keep feature identity in ids, `data-*` hooks, htmx URLs, and server-side data—not in spacing, border, layout, or control classes.
5. Use `.panel-body` rather than nesting a second `.panel` for inset content.
6. Use `.panel-subheading` explicitly; do not style all direct `h3` elements.
7. Wrap every table in `.tablewrap`; do not add table-specific cell padding.
8. Keep top-level spacing in `<main>` and nested fragment spacing in the shared fragment rule; do not add page-specific panel margins.
9. Extend `assets/ui.js` only for behavior used by more than one feature adapter. Keep domain transitions in the adapter.
10. Preserve native semantics and keyboard support: buttons for actions, `<details>/<summary>` for disclosures where appropriate, ARIA tabs for view switches, and visible `:focus-visible` outlines.

## Verification contract

Changes must pass:

- embedded framework contracts in `src/cli/dashboard/mod.rs`;
- all dashboard integration, UI-style, and security suites;
- `cargo nextest run --workspace --no-fail-fast` and workspace doctests;
- format, clippy with warnings denied, and `git diff --check`;
- the deterministic 1440×1000 cross-screen fixture matrix for Overview, Resources, APIs, AI, Learning, MCP, and Operations;
- independent review checking that page-specific presentation has not been reintroduced.
