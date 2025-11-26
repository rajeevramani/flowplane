# Implementation Plan: Eliminate api_definitions and api_routes Tables

**Status**: In Progress
**Started**: 2025-11-20
**Goal**: Simplify architecture by storing all routes in native tables, eliminating dual materialization

---

## Executive Summary

This refactoring eliminates the `api_definitions` and `api_routes` tables, replacing them with a simplified architecture where ALL routes (native + imported) are stored in the `routes` table. This removes the complex dual materialization strategy and merge logic in xDS generation.

### Key Benefits
- ✅ Single source of truth for routes (no more api_routes + routes duality)
- ✅ Eliminates complex merge logic in xDS resource building
- ✅ Simpler database schema (fewer tables)
- ✅ Maintains provenance tracking via `import_id`
- ✅ Preserves cluster deduplication across imports
- ✅ No breaking changes to xDS protocol or Envoy sync

---

## Phase 1: Schema & Repository Updates ✅ COMPLETED

### 1.1 Database Migrations ✅
**Files Created:**
- `migrations/20251120000001_create_import_metadata_table.sql`
- `migrations/20251120000002_create_cluster_references_table.sql`
- `migrations/20251120000003_add_import_tracking_to_native_tables.sql`
- `migrations/20251120000004_drop_api_definitions_tables.sql`

**New Tables:**
```sql
import_metadata (
    id, spec_name, spec_version, spec_checksum, team,
    source_content, imported_at, updated_at
)

cluster_references (
    cluster_id, import_id, route_count, created_at
)
```

**Enhanced Native Tables:**
```sql
routes: + import_id, route_order, headers
clusters: + import_id
listeners: + import_id
```

### 1.2 New Repositories ✅
**Files Created:**
- `src/storage/repositories/import_metadata.rs`
- `src/storage/repositories/cluster_references.rs`

**Capabilities:**
- Import metadata CRUD operations
- Cluster reference counting for deduplication
- Orphaned cluster detection
- Team-based import filtering

### 1.3 Model Updates ✅ PARTIALLY COMPLETE
**Files Updated:**
- `src/storage/repositories/route.rs` - Added import_id, route_order, headers
- `src/storage/repositories/cluster.rs` - Added import_id (PARTIAL - queries need updating)
- `src/storage/repositories/mod.rs` - Exported new repositories

**Remaining:**
- Update ALL cluster SELECT queries to include import_id
- Update cluster CREATE to accept import_id
- Update listener repository similarly

---

## Phase 2: XDS Refactoring (CRITICAL) ⚠️ NOT STARTED

### 2.1 Remove Dynamic Route Generation
**File:** `src/xds/resources.rs`

**Current State:**
```rust
// Lines 142-303: resources_from_api_definitions()
// Dynamically generates RouteConfiguration from api_routes table
// This entire function becomes OBSOLETE
```

**Changes Required:**
1. **DELETE** `resources_from_api_definitions()` function (lines 142-303)
2. **DELETE** helper functions:
   - `parse_upstream_targets()` (lines 312-348)
   - `build_route_match()` (related to api_routes)
3. **KEEP** `routes_from_database_entries()` - works with native routes table
4. **KEEP** `clusters_from_database_entries()` - works with native clusters table
5. **KEEP** `listeners_from_database_entries()` - works with native listeners table

**Impact:**
- xDS will ONLY read from native routes table
- No more dynamic generation at xDS request time
- Simpler, single-path resource building

### 2.2 Remove Merge Logic
**File:** `src/xds/services/database.rs`

**Current State (lines 239-306):**
```rust
// Gets native routes
let built = routes_from_database_entries(routes, "native")?;

// Gets platform API routes dynamically
if let Some(api_repo) = &self.state.api_definition_repository {
    let platform_resources = resources_from_api_definitions(...)?;
    built.extend(platform_routes); // MERGE HERE
}
```

**Changes Required:**
1. **DELETE** lines 258-306 (entire platform API merge block)
2. **SIMPLIFY** to single path:
```rust
let routes = self.route_repository.list_by_teams(&teams, true, None, None).await?;
let built = routes_from_database_entries(routes, "all")?;
// Done - no merge needed!
```

**Impact:**
- Eliminates dual materialization complexity
- Single database query instead of two
- Faster xDS response generation

### 2.3 Remove Platform API Refresh
**File:** `src/xds/state.rs`

**Current State:**
```rust
pub async fn refresh_platform_api_resources(&self) -> Result<()> {
    // Reads api_definitions + api_routes
    // Merges with native routes
    // Updates cache
}
```

**Changes Required:**
1. **DELETE** `refresh_platform_api_resources()` method (lines ~400-500)
2. **UPDATE** `refresh_routes_from_repository()` to handle ALL routes (no merge)
3. **REMOVE** `api_definition_repository` field from `XdsState` struct
4. **REMOVE** special-case logic in resource cache updates

**Impact:**
- Simpler state management
- No dual refresh logic
- Single cache update path

### 2.4 Update Database Watchers
**File:** `src/xds/services/database.rs`

**Current State:**
- Separate watchers for routes vs api_routes
- Different polling logic

**Changes Required:**
1. **KEEP** existing routes watcher (lines 596-640)
2. **DELETE** any api_routes/api_definitions watchers
3. **VERIFY** watcher SQL still works:
```sql
SELECT COUNT(*), MAX(updated_at) FROM routes
-- This should detect changes to imported routes too
```

**Impact:**
- Watchers automatically detect imported route changes
- No special-case polling needed

---

## Phase 3: OpenAPI Import Refactoring ⚠️ NOT STARTED

### 3.1 Create New Import Handler
**File:** `src/api/handlers/openapi_import.rs` (NEW)

**Purpose:** Replace `api_definitions.rs` handler with direct route creation

**Implementation Strategy:**
```rust
pub async fn import_openapi_spec(
    spec: OpenApiDocument,
    team: String,
    import_metadata_repo: ImportMetadataRepository,
    route_repo: RouteRepository,
    cluster_repo: ClusterRepository,
    cluster_refs_repo: ClusterReferencesRepository,
) -> Result<ImportResult> {
    // Step 1: Create import_metadata record
    let import_id = import_metadata_repo.create(CreateImportMetadataRequest {
        spec_name: extract_spec_name(&spec),
        spec_version: extract_version(&spec),
        spec_checksum: compute_sha256(&spec),
        team: team.clone(),
        source_content: Some(serde_json::to_string(&spec)?),
    }).await?;

    // Step 2: Parse OpenAPI spec
    let parsed_routes = parse_openapi_routes(&spec)?;

    // Step 3: Deduplicate and create clusters
    let mut endpoint_to_cluster = HashMap::new();
    for route in &parsed_routes {
        for endpoint in &route.upstream_endpoints {
            if !endpoint_to_cluster.contains_key(endpoint) {
                // Check if cluster already exists from another import
                let cluster_name = generate_cluster_name(endpoint);
                let existing = cluster_repo.get_by_name(&cluster_name).await?;

                let cluster_id = if let Some(cluster) = existing {
                    // Reuse existing cluster
                    cluster.id
                } else {
                    // Create new cluster
                    let cluster = cluster_repo.create(CreateClusterRequest {
                        name: cluster_name.clone(),
                        service_name: cluster_name.clone(),
                        configuration: build_cluster_config(endpoint),
                        team: Some(team.clone()),
                        import_id: Some(import_id.clone()),
                    }).await?;
                    cluster.id
                };

                // Track reference
                cluster_refs_repo.add_reference(&cluster_id, &import_id, 1).await?;
                endpoint_to_cluster.insert(endpoint.clone(), cluster_id);
            }
        }
    }

    // Step 4: Create routes directly in routes table
    let mut created_routes = vec![];
    for (idx, parsed_route) in parsed_routes.iter().enumerate() {
        let cluster_id = endpoint_to_cluster.get(&parsed_route.upstream_endpoints[0])?;

        let route = route_repo.create(CreateRouteRequest {
            name: format!("openapi-{}-{}", import_id, idx),
            path_prefix: parsed_route.path.clone(),
            cluster_name: parsed_route.cluster_name.clone(),
            configuration: build_route_config(parsed_route),
            team: Some(team.clone()),
            import_id: Some(import_id.clone()),
            route_order: Some(idx as i64),
            headers: parsed_route.headers.clone(),
        }).await?;

        created_routes.push(route);
    }

    // Step 5: Optionally create isolated listener
    // (if spec requires dedicated listener)

    // Step 6: Trigger xDS refresh
    xds_state.refresh_clusters_from_repository().await?;
    xds_state.refresh_routes_from_repository().await?;

    Ok(ImportResult {
        import_id,
        routes_created: created_routes.len(),
        clusters_created: endpoint_to_cluster.len(),
    })
}
```

**Key Design Decisions:**

1. **Cluster Deduplication:**
   - Check if cluster with same name exists across ALL imports
   - Use `cluster_references` to track which imports use which clusters
   - Safe to delete cluster only when all referencing imports are deleted

2. **Route Naming:**
   - Auto-generated: `openapi-{import_id}-{route_index}`
   - Ensures uniqueness within import
   - Easy to identify imported routes

3. **Route Ordering:**
   - Store in `route_order` field
   - xDS builder sorts by route_order before generating Envoy config
   - Preserves OpenAPI path precedence

4. **Headers Matching:**
   - Store as JSON in `headers` field
   - xDS builder parses and applies header matchers
   - Supports HTTP method matching (GET, POST, etc.)

### 3.2 Update/Re-import Handler
**File:** Same as above

**Implementation:**
```rust
pub async fn reimport_openapi_spec(
    spec_name: String,
    team: String,
    new_spec: OpenApiDocument,
    repos: AllRepositories,
) -> Result<ImportResult> {
    // Step 1: Find existing import
    let existing = import_metadata_repo.get_by_team_and_spec(&team, &spec_name).await?;

    if let Some(existing_import) = existing {
        // Step 2: Delete old import (CASCADE deletes routes/listeners)
        delete_import(&existing_import.id, repos).await?;

        // Step 3: Decrement cluster references
        let refs = cluster_refs_repo.get_by_import(&existing_import.id).await?;
        for ref in refs {
            cluster_refs_repo.decrement_reference(&ref.cluster_id, &existing_import.id, ref.route_count).await?;

            // Check if cluster is now orphaned
            if !cluster_refs_repo.is_cluster_referenced(&ref.cluster_id).await? {
                cluster_repo.delete(&ref.cluster_id).await?;
            }
        }
    }

    // Step 4: Create new import (same as initial import)
    import_openapi_spec(new_spec, team, repos).await
}
```

### 3.3 Delete Import Handler
**Implementation:**
```rust
pub async fn delete_import(
    import_id: &str,
    repos: AllRepositories,
) -> Result<()> {
    // Step 1: Get cluster references before deletion
    let refs = cluster_refs_repo.get_by_import(import_id).await?;

    // Step 2: Delete import (CASCADE deletes routes/listeners via FK)
    import_metadata_repo.delete(import_id).await?;

    // Step 3: Delete cluster references (CASCADE via FK)
    // This happens automatically

    // Step 4: Delete orphaned clusters
    for ref in refs {
        if !cluster_refs_repo.is_cluster_referenced(&ref.cluster_id).await? {
            cluster_repo.delete(&ref.cluster_id).await?;
        }
    }

    // Step 5: Trigger xDS refresh
    xds_state.refresh_clusters_from_repository().await?;
    xds_state.refresh_routes_from_repository().await?;

    Ok(())
}
```

---

## Phase 4: Delete Platform API Module ⚠️ NOT STARTED

### 4.1 Files to Delete
**Delete Entire Directory:**
```bash
rm -rf src/platform_api/
```

**Specific Files:**
- `src/platform_api/materializer.rs` (core materialization logic)
- `src/platform_api/bootstrap.rs` (bootstrap generation)
- `src/platform_api/mod.rs`

**Update:**
- `src/lib.rs` - Remove `pub mod platform_api;`

### 4.2 Delete Platform API Handlers
**File:** `src/api/handlers/api_definitions.rs`

**Action:** DELETE ENTIRE FILE

**Functions Being Removed:**
- `create_api_definition()`
- `update_api_definition()`
- `delete_api_definition()`
- `get_api_definition()`
- `list_api_definitions()`
- `import_from_openapi()`
- `get_bootstrap_config()`

**Replacement:** New handlers in `openapi_import.rs`

### 4.3 Update API Routes
**File:** `src/api/routes.rs`

**Current:**
```rust
.route("/api/v1/api-definitions", post(create_api_definition))
.route("/api/v1/api-definitions/:id", get(get_api_definition))
.route("/api/v1/api-definitions/from-openapi", post(import_from_openapi))
// etc.
```

**After:**
```rust
.route("/api/v1/openapi/import", post(import_openapi))
.route("/api/v1/imports/:id", get(get_import))
.route("/api/v1/imports/:id", delete(delete_import))
.route("/api/v1/imports/:id/reimport", put(reimport_openapi))
```

**Impact:**
- API endpoints change (breaking change for clients)
- Simpler, more focused endpoints
- No backward compatibility needed (no customers yet)

### 4.4 Update AppState
**File:** `src/main.rs` or wherever AppState is defined

**Current:**
```rust
pub struct AppState {
    pub cluster_repository: Arc<ClusterRepository>,
    pub route_repository: Arc<RouteRepository>,
    pub listener_repository: Arc<ListenerRepository>,
    pub api_definition_repository: Arc<ApiDefinitionRepository>, // REMOVE
    pub xds_state: Arc<XdsState>,
}
```

**After:**
```rust
pub struct AppState {
    pub cluster_repository: Arc<ClusterRepository>,
    pub route_repository: Arc<RouteRepository>,
    pub listener_repository: Arc<ListenerRepository>,
    pub import_metadata_repository: Arc<ImportMetadataRepository>, // ADD
    pub cluster_references_repository: Arc<ClusterReferencesRepository>, // ADD
    pub xds_state: Arc<XdsState>,
}
```

---

## Phase 5: Update All Tests ⚠️ NOT STARTED

### 5.1 Platform API Tests to Migrate
**Files to Update:**
- `tests/platform_api.rs` → Rewrite to use new import API
- `tests/platform_api/test_openapi_import.rs` → Complete rewrite
- `tests/platform_api/test_api_definition_creates_native_resources.rs` → DELETE or rewrite

**Migration Strategy:**

**Before (api_definitions):**
```rust
let response = client.post("/api/v1/api-definitions")
    .json(&CreateApiDefinitionRequest { ... })
    .send().await?;

// Verify api_definitions table
let def = api_def_repo.get_by_id(&def_id).await?;

// Verify generated clusters
let cluster = cluster_repo.get_by_id(&def.generated_cluster_id).await?;
```

**After (openapi_import):**
```rust
let response = client.post("/api/v1/openapi/import?team=test-team")
    .json(&openapi_spec)
    .send().await?;

// Verify import_metadata table
let import = import_metadata_repo.get_by_team_and_spec("test-team", "payment-api").await?;

// Verify routes in routes table
let routes = route_repo.list_by_import(&import.id).await?;
assert_eq!(routes.len(), 3);

// Verify clusters
let clusters = cluster_repo.list_by_import(&import.id).await?;

// Verify cluster references
let refs = cluster_refs_repo.get_by_import(&import.id).await?;
```

### 5.2 Update Unit Tests

**Files with api_definition/api_routes dependencies:**
```bash
# Find all test files referencing api_definitions
grep -r "api_definition" tests/
grep -r "ApiDefinitionRepository" tests/
grep -r "platform_api" tests/
```

**For Each Test:**
1. Replace ApiDefinitionRepository with ImportMetadataRepository
2. Replace api_routes queries with routes queries (filtered by import_id)
3. Update assertions to check routes table instead of api_routes
4. Verify xDS responses contain correct route configs

### 5.3 Integration Test Updates

**OpenAPI Import E2E Test:**
```rust
#[tokio::test]
async fn test_openapi_import_creates_routes_and_clusters() {
    let pool = setup_test_db().await;
    let import_repo = ImportMetadataRepository::new(pool.clone());
    let route_repo = RouteRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());

    // Import OpenAPI spec
    let spec = load_test_openapi_spec();
    let result = import_openapi_spec(spec, "test-team".to_string(), ...).await.unwrap();

    // Verify import metadata created
    let import = import_repo.get_by_id(&result.import_id).await.unwrap().unwrap();
    assert_eq!(import.spec_name, "payment-api");
    assert_eq!(import.team, "test-team");

    // Verify routes created in routes table
    let routes = route_repo.list_by_import(&import.id).await.unwrap();
    assert_eq!(routes.len(), 5);
    assert!(routes.iter().all(|r| r.import_id == Some(import.id.clone())));

    // Verify route ordering
    let mut route_orders: Vec<_> = routes.iter().map(|r| r.route_order.unwrap()).collect();
    route_orders.sort();
    assert_eq!(route_orders, vec![0, 1, 2, 3, 4]);

    // Verify clusters created
    let clusters = cluster_repo.list_by_import(&import.id).await.unwrap();
    assert_eq!(clusters.len(), 3); // Assuming 5 routes deduplicate to 3 clusters

    // Verify cluster references
    let refs = cluster_refs_repo.get_by_import(&import.id).await.unwrap();
    assert_eq!(refs.len(), 3);

    // Verify xDS sync works
    let xds_routes = fetch_routes_from_xds(&xds_state, "test-team").await.unwrap();
    assert_eq!(xds_routes.len(), 5);
}
```

**Re-import Test:**
```rust
#[tokio::test]
async fn test_reimport_replaces_routes() {
    // Initial import
    let result1 = import_openapi_spec(spec_v1, "test-team", ...).await.unwrap();
    let routes_v1 = route_repo.list_by_import(&result1.import_id).await.unwrap();
    assert_eq!(routes_v1.len(), 3);

    // Re-import with updated spec
    let result2 = reimport_openapi_spec("payment-api", "test-team", spec_v2, ...).await.unwrap();
    let routes_v2 = route_repo.list_by_import(&result2.import_id).await.unwrap();
    assert_eq!(routes_v2.len(), 5); // Spec v2 has more routes

    // Verify old routes deleted
    let old_routes = route_repo.list_by_import(&result1.import_id).await.unwrap();
    assert_eq!(old_routes.len(), 0);
}
```

**Delete Import Test:**
```rust
#[tokio::test]
async fn test_delete_import_cascades_correctly() {
    let result = import_openapi_spec(spec, "test-team", ...).await.unwrap();

    // Verify resources exist
    let routes_before = route_repo.list_by_import(&result.import_id).await.unwrap();
    assert_eq!(routes_before.len(), 3);

    // Delete import
    delete_import(&result.import_id, repos).await.unwrap();

    // Verify import deleted
    let import = import_repo.get_by_id(&result.import_id).await.unwrap();
    assert!(import.is_none());

    // Verify routes deleted (CASCADE)
    let routes_after = route_repo.list_by_import(&result.import_id).await.unwrap();
    assert_eq!(routes_after.len(), 0);

    // Verify orphaned clusters deleted
    // (clusters with no references should be gone)
}
```

### 5.4 XDS Integration Tests

**Test:** Verify routes from routes table are correctly sent to Envoy

```rust
#[tokio::test]
async fn test_xds_serves_imported_routes() {
    // Import OpenAPI spec
    let result = import_openapi_spec(spec, "test-team", ...).await.unwrap();

    // Create mock Envoy connection
    let mut envoy_client = create_mock_envoy_client();

    // Request routes via xDS
    let discovery_request = DiscoveryRequest {
        type_url: ROUTE_TYPE_URL.to_string(),
        node: Some(Node {
            metadata: create_team_metadata("test-team"),
            ..Default::default()
        }),
        ..Default::default()
    };

    let response = xds_service.stream_aggregated_resources(discovery_request).await.unwrap();

    // Verify response contains routes
    assert_eq!(response.resources.len(), 3);

    // Decode and verify route configs
    for resource in response.resources {
        let route_config = RouteConfiguration::decode(&resource.value[..]).unwrap();
        // Verify route names, virtual hosts, etc.
    }
}
```

---

## Phase 6: Cleanup & Verification ⚠️ NOT STARTED

### 6.1 Remove Unused Code

**Search for references:**
```bash
# Find all references to api_definitions/api_routes
rg "api_definition" --type rust
rg "ApiDefinitionRepository" --type rust
rg "ApiRouteData" --type rust
rg "platform_api" --type rust
```

**Files that may need updates:**
- Error messages referencing api_definitions
- Documentation mentioning platform API
- Example code in comments
- CLI commands (if any)

### 6.2 Update Database Schema Documentation

**File:** `docs/database-schema.md` (if exists)

**Update ERD to reflect:**
- Removed: api_definitions, api_routes
- Added: import_metadata, cluster_references
- Updated: routes (new columns), clusters (new columns)

### 6.3 Final Verification Checklist

**Database:**
- [ ] All migrations run cleanly on fresh database
- [ ] All foreign keys correctly defined
- [ ] All indexes created
- [ ] No orphaned data in tables

**Code:**
- [ ] No remaining references to api_definitions/api_routes
- [ ] All tests passing (`cargo test`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Code formatted (`cargo fmt`)

**XDS Sync:**
- [ ] Routes correctly synced to Envoy from routes table
- [ ] Clusters correctly synced from clusters table
- [ ] Listeners correctly synced from listeners table
- [ ] Database watchers detect changes
- [ ] Version increments correctly trigger updates
- [ ] Team isolation still works

**Functionality:**
- [ ] Can import OpenAPI spec
- [ ] Routes created in routes table
- [ ] Clusters deduplicated correctly
- [ ] Can re-import updated spec
- [ ] Old resources cleaned up on re-import
- [ ] Can delete import
- [ ] Cascade delete works correctly
- [ ] Orphaned clusters deleted

**Performance:**
- [ ] xDS response time improved (no merge logic)
- [ ] Database queries optimized
- [ ] No N+1 query issues

---

## Risk Assessment & Mitigation

### High Risk Areas

**1. XDS Sync Breaking**
- **Risk:** Incorrect SQL queries cause xDS to not find routes
- **Mitigation:** Extensive integration tests with real Envoy
- **Rollback:** Keep api_definitions migration separate, can revert

**2. Cluster Deduplication Logic**
- **Risk:** Deleting shared clusters breaks other imports
- **Mitigation:** Thorough testing of cluster_references logic
- **Test:** Import 2 specs sharing same backend, delete 1, verify cluster remains

**3. Route Ordering**
- **Risk:** Incorrect route_order causes wrong Envoy matching
- **Mitigation:** Explicit sorting in xDS builder, test with overlapping paths

**4. Migration Data Loss**
- **Risk:** No existing data to migrate (no customers yet)
- **Mitigation:** N/A - clean slate

### Medium Risk Areas

**1. Test Coverage Gaps**
- **Risk:** Missing edge cases in new import logic
- **Mitigation:** Comprehensive test plan (see Phase 5)

**2. Performance Regression**
- **Risk:** New queries slower than old
- **Mitigation:** Benchmark before/after, index optimization

---

## Estimated Effort

**Phase 1 (Schema):** ✅ 2 hours (COMPLETED)
**Phase 2 (XDS):** ⚠️ 3-4 hours (CRITICAL PATH)
**Phase 3 (Import):** ⚠️ 4-5 hours (COMPLEX LOGIC)
**Phase 4 (Cleanup):** ⚠️ 1-2 hours
**Phase 5 (Tests):** ⚠️ 4-6 hours (THOROUGH TESTING NEEDED)
**Phase 6 (Verification):** ⚠️ 2-3 hours

**Total:** ~16-22 hours remaining

---

## Next Steps

**Immediate (Today):**
1. Complete cluster repository query updates (30 min)
2. Update listener repository (30 min)
3. Start Phase 2: XDS refactoring (2 hours)

**Tomorrow:**
1. Complete XDS refactoring
2. Begin OpenAPI import handler
3. Write core import logic with tests

**Day 3:**
1. Complete import/reimport/delete handlers
2. Update all tests
3. Run full test suite

**Day 4:**
1. Cleanup and verification
2. Performance testing
3. Documentation

---

## Success Criteria

✅ **Code Quality:**
- All tests pass
- No clippy warnings
- No unwrap/expect in production code

✅ **Functionality:**
- Can import OpenAPI spec → routes appear in routes table
- Re-import updates routes correctly
- Delete cascades correctly
- Cluster deduplication works across imports
- xDS serves routes to Envoy correctly

✅ **Performance:**
- xDS response time ≤ current performance
- Database queries optimized
- No regression in sync latency

✅ **Architecture:**
- Single source of truth for routes
- No dual materialization
- No merge logic
- Simpler codebase

---

## Open Questions

1. **Should we keep bootstrap config generation?**
   - Current: `GET /api/v1/teams/{team}/bootstrap`
   - Depends on api_definitions.bootstrap_uri field
   - **Decision Needed:** Keep as team-scoped or remove?

2. **Should we support filtering routes by import in Native API?**
   - Example: `GET /api/v1/routes?import_id=xyz`
   - Useful for debugging
   - **Decision:** Yes, add to RouteRepository

3. **How to handle listener creation in OpenAPI import?**
   - Current: api_definitions.listener_isolation determines if listener created
   - **Decision Needed:** Parse from OpenAPI spec? User parameter?

4. **Should import_metadata store full OpenAPI spec?**
   - Current plan: Yes (source_content field)
   - Pro: Can re-parse later, show diff on reimport
   - Con: Large JSON blobs in database
   - **Decision:** Optional, leave NULL for now

---

## Phase 7: UI Updates ⚠️ NOT STARTED

### 7.1 Update Admin UI Components

**Files to Update:**
- `ui/src/routes/(authenticated)/api-definitions/` → Rename to `openapi-imports/`
- `ui/src/lib/api/client.ts` - Update API client methods
- `ui/src/lib/api/types.ts` - Update TypeScript types

**Current UI Flow:**
```
Dashboard → API Definitions → Create/Edit/Delete
                           → Import from OpenAPI
```

**New UI Flow:**
```
Dashboard → OpenAPI Imports → Import Spec
                           → View Imports
                           → Re-import/Delete
                           → View Generated Routes
```

### 7.2 API Client Updates

**File:** `ui/src/lib/api/client.ts`

**Current Methods (TO REMOVE):**
```typescript
async createApiDefinition(req: CreateApiDefinitionRequest)
async updateApiDefinition(id: string, req: UpdateApiDefinitionRequest)
async deleteApiDefinition(id: string)
async getApiDefinition(id: string)
async listApiDefinitions()
async importFromOpenApi(team: string, spec: OpenApiSpec)
```

**New Methods (TO ADD):**
```typescript
async importOpenApiSpec(team: string, spec: OpenApiSpec): Promise<ImportResult>
async reimportOpenApiSpec(specName: string, team: string, spec: OpenApiSpec): Promise<ImportResult>
async deleteImport(importId: string): Promise<void>
async getImport(importId: string): Promise<ImportMetadata>
async listImports(team: string): Promise<ImportMetadata[]>
async getImportRoutes(importId: string): Promise<Route[]>
async getImportClusters(importId: string): Promise<Cluster[]>
```

### 7.3 TypeScript Type Definitions

**File:** `ui/src/lib/api/types.ts`

**Remove:**
```typescript
interface ApiDefinition {
  id: string;
  team: string;
  domain: string;
  listener_isolation: boolean;
  tls_config?: TlsConfig;
  bootstrap_uri?: string;
  generated_listener_id?: string;
}

interface ApiRoute {
  id: string;
  api_definition_id: string;
  match_type: string;
  match_value: string;
  upstream_targets: UpstreamTargets;
  generated_cluster_id?: string;
}
```

**Add:**
```typescript
interface ImportMetadata {
  id: string;
  spec_name: string;
  spec_version?: string;
  spec_checksum?: string;
  team: string;
  imported_at: string;
  updated_at: string;
}

interface ImportResult {
  import_id: string;
  routes_created: number;
  clusters_created: number;
}

interface Route {
  id: string;
  name: string;
  path_prefix: string;
  cluster_name: string;
  team?: string;
  import_id?: string;
  route_order?: number;
  headers?: Record<string, string>;
  created_at: string;
  updated_at: string;
}
```

### 7.4 Update Import UI Page

**File:** `ui/src/routes/(authenticated)/openapi-imports/+page.svelte` (NEW)

**Features:**
- Upload OpenAPI spec (JSON/YAML)
- Paste OpenAPI spec content
- Select team
- Import button
- Show import history

**Component Structure:**
```svelte
<script lang="ts">
  import { importOpenApiSpec, listImports } from '$lib/api/client';
  import { goto } from '$app/navigation';

  let specContent = '';
  let selectedTeam = '';
  let imports: ImportMetadata[] = [];
  let uploading = false;

  async function handleImport() {
    uploading = true;
    try {
      const spec = JSON.parse(specContent);
      const result = await importOpenApiSpec(selectedTeam, spec);
      await goto(`/openapi-imports/${result.import_id}`);
    } catch (error) {
      // Handle error
    } finally {
      uploading = false;
    }
  }

  onMount(async () => {
    imports = await listImports(selectedTeam);
  });
</script>

<div class="container">
  <h1>Import OpenAPI Specification</h1>

  <div class="import-form">
    <label>Team</label>
    <select bind:value={selectedTeam}>
      <!-- Team options -->
    </select>

    <label>OpenAPI Spec</label>
    <textarea bind:value={specContent} rows="20" />

    <button on:click={handleImport} disabled={uploading}>
      {uploading ? 'Importing...' : 'Import'}
    </button>
  </div>

  <div class="import-history">
    <h2>Import History</h2>
    <table>
      <thead>
        <tr>
          <th>Spec Name</th>
          <th>Version</th>
          <th>Team</th>
          <th>Imported</th>
          <th>Actions</th>
        </tr>
      </thead>
      <tbody>
        {#each imports as import}
          <tr>
            <td>{import.spec_name}</td>
            <td>{import.spec_version || 'N/A'}</td>
            <td>{import.team}</td>
            <td>{formatDate(import.imported_at)}</td>
            <td>
              <button on:click={() => viewImport(import.id)}>View</button>
              <button on:click={() => deleteImport(import.id)}>Delete</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>
```

### 7.5 Import Detail Page

**File:** `ui/src/routes/(authenticated)/openapi-imports/[id]/+page.svelte` (NEW)

**Features:**
- Show import metadata
- List generated routes
- List generated clusters
- Re-import button
- Delete import button
- Link to view routes in Routes page (filtered by import_id)

**Component:**
```svelte
<script lang="ts">
  import { page } from '$app/stores';
  import { getImport, getImportRoutes, getImportClusters, deleteImport } from '$lib/api/client';

  let importData: ImportMetadata;
  let routes: Route[];
  let clusters: Cluster[];

  onMount(async () => {
    const importId = $page.params.id;
    importData = await getImport(importId);
    routes = await getImportRoutes(importId);
    clusters = await getImportClusters(importId);
  });
</script>

<div class="import-detail">
  <h1>Import: {importData.spec_name}</h1>

  <div class="metadata">
    <p>Team: {importData.team}</p>
    <p>Version: {importData.spec_version || 'N/A'}</p>
    <p>Imported: {formatDate(importData.imported_at)}</p>
    <p>Last Updated: {formatDate(importData.updated_at)}</p>
  </div>

  <div class="actions">
    <button on:click={handleReimport}>Re-import</button>
    <button on:click={handleDelete} class="danger">Delete Import</button>
  </div>

  <div class="routes-section">
    <h2>Generated Routes ({routes.length})</h2>
    <ResourceTable items={routes} type="route" />
  </div>

  <div class="clusters-section">
    <h2>Generated Clusters ({clusters.length})</h2>
    <ResourceTable items={clusters} type="cluster" />
  </div>
</div>
```

### 7.6 Update Navigation

**File:** `ui/src/lib/components/Navigation.svelte`

**Current:**
```svelte
<nav>
  <a href="/dashboard">Dashboard</a>
  <a href="/api-definitions">API Definitions</a>
  <a href="/resources">Resources</a>
</nav>
```

**Updated:**
```svelte
<nav>
  <a href="/dashboard">Dashboard</a>
  <a href="/openapi-imports">OpenAPI Imports</a>
  <a href="/resources">Resources</a>
</nav>
```

### 7.7 Update Resources Page Filtering

**File:** `ui/src/routes/(authenticated)/resources/+page.svelte`

**Add filtering by import:**
```svelte
<script lang="ts">
  let filterByImport: string | null = null;

  async function loadRoutes() {
    const params = filterByImport ? { import_id: filterByImport } : {};
    routes = await listRoutes(params);
  }
</script>

<div class="filters">
  <label>Filter by Import:</label>
  <select bind:value={filterByImport} on:change={loadRoutes}>
    <option value={null}>All Routes</option>
    {#each imports as import}
      <option value={import.id}>{import.spec_name}</option>
    {/each}
  </select>
</div>
```

### 7.8 Update Dashboard Statistics

**File:** `ui/src/routes/(authenticated)/dashboard/+page.svelte`

**Current stats:**
```svelte
- API Definitions: {apiDefinitions.length}
- Routes: {routes.length}
- Clusters: {clusters.length}
```

**Updated stats:**
```svelte
- OpenAPI Imports: {imports.length}
- Routes: {routes.length} ({importedRoutes.length} from imports)
- Clusters: {clusters.length} ({importedClusters.length} from imports)
```

### 7.9 Error Handling & User Feedback

**Add toast notifications for:**
- Import success: "OpenAPI spec imported successfully. Created X routes and Y clusters."
- Re-import success: "Spec re-imported. Updated X routes."
- Delete success: "Import deleted. Removed X routes and Y clusters."
- Import error: "Failed to import spec: {error message}"

**Validation:**
- Check OpenAPI spec is valid JSON/YAML before submitting
- Show parse errors clearly
- Warn before deleting (show impact: "This will delete X routes and Y clusters")

### 7.10 Testing UI Changes

**Manual Testing Checklist:**
- [ ] Can upload and import OpenAPI spec
- [ ] Import appears in history list
- [ ] Can view import details
- [ ] Generated routes shown correctly
- [ ] Generated clusters shown correctly
- [ ] Can filter routes by import_id
- [ ] Can delete import
- [ ] Deletion removes routes and clusters
- [ ] Can re-import updated spec
- [ ] Navigation works correctly
- [ ] Responsive design maintained

**E2E Tests (Playwright/Cypress):**
```typescript
test('import OpenAPI spec flow', async ({ page }) => {
  await page.goto('/openapi-imports');

  // Select team
  await page.selectOption('select[name="team"]', 'test-team');

  // Paste OpenAPI spec
  const spec = await readFile('./fixtures/payment-api.json');
  await page.fill('textarea', spec);

  // Import
  await page.click('button:has-text("Import")');

  // Wait for redirect to detail page
  await page.waitForURL(/\/openapi-imports\/[a-f0-9-]+/);

  // Verify routes table
  const routes = await page.locator('table tbody tr').count();
  expect(routes).toBeGreaterThan(0);

  // Delete import
  await page.click('button:has-text("Delete Import")');
  await page.click('button:has-text("Confirm")');

  // Verify redirect to list
  await page.waitForURL('/openapi-imports');
});
```

---

## Estimated Effort (Updated)

**Phase 1 (Schema):** ✅ 2 hours (COMPLETED)
**Phase 2 (XDS):** ⚠️ 3-4 hours (CRITICAL PATH)
**Phase 3 (Import):** ⚠️ 4-5 hours (COMPLEX LOGIC)
**Phase 4 (Cleanup):** ⚠️ 1-2 hours
**Phase 5 (Tests):** ⚠️ 4-6 hours (THOROUGH TESTING NEEDED)
**Phase 6 (Verification):** ⚠️ 2-3 hours
**Phase 7 (UI Updates):** ⚠️ 3-4 hours (NEW)

**Total:** ~19-26 hours remaining (from 2 hours completed)

---

## Appendix: Key File Locations

**Migrations:**
- `/Users/rajeevramani/workspace/projects/flowplane/migrations/20251120000001_create_import_metadata_table.sql`
- `/Users/rajeevramani/workspace/projects/flowplane/migrations/20251120000002_create_cluster_references_table.sql`
- `/Users/rajeevramani/workspace/projects/flowplane/migrations/20251120000003_add_import_tracking_to_native_tables.sql`
- `/Users/rajeevramani/workspace/projects/flowplane/migrations/20251120000004_drop_api_definitions_tables.sql`

**Repositories:**
- `/Users/rajeevramani/workspace/projects/flowplane/src/storage/repositories/import_metadata.rs`
- `/Users/rajeevramani/workspace/projects/flowplane/src/storage/repositories/cluster_references.rs`
- `/Users/rajeevramani/workspace/projects/flowplane/src/storage/repositories/route.rs` (UPDATED)
- `/Users/rajeevramani/workspace/projects/flowplane/src/storage/repositories/cluster.rs` (PARTIAL)

**XDS (TO UPDATE):**
- `/Users/rajeevramani/workspace/projects/flowplane/src/xds/resources.rs` - Remove resources_from_api_definitions()
- `/Users/rajeevramani/workspace/projects/flowplane/src/xds/services/database.rs` - Remove merge logic
- `/Users/rajeevramani/workspace/projects/flowplane/src/xds/state.rs` - Remove refresh_platform_api_resources()

**Platform API (TO DELETE):**
- `/Users/rajeevramani/workspace/projects/flowplane/src/platform_api/` (entire directory)
- `/Users/rajeevramani/workspace/projects/flowplane/src/api/handlers/api_definitions.rs`

**Tests (TO UPDATE):**
- `/Users/rajeevramani/workspace/projects/flowplane/tests/platform_api.rs`
- `/Users/rajeevramani/workspace/projects/flowplane/tests/platform_api/test_openapi_import.rs`

---

**Document Version:** 1.0
**Last Updated:** 2025-11-20
**Author:** Claude Code
**Status:** Ready for Review
