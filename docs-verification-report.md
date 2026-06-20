# Docs Verification Report

Run date: 2026-06-20

Config:

- Epic issue: #100
- Label: `docs-verification`
- Marker: `[docs-verify]`

## Preconditions

- `gh auth status`: pass. Account `rajeevramani`, token scopes include `repo`.
- Build/toolchain: pass. `cargo 1.94.1`; `cargo build --bin flowplane` completed.
- Executable docs: partially verified by execution. PostgreSQL docs/scripts that rely on `postgres://postgres:postgres@127.0.0.1:5432/...` were blocked because the local PostgreSQL server rejected that role.

## Documentation Set

Actionable:

- `README.md` - reference/how-to command list
- `docs/dev-dataplane.md` - how-to
- `docs/tutorials/getting-started.md` - tutorial
- `docs/how-to/cli-auth-and-contexts.md` - how-to
- `docs/how-to/register-dataplane-mtls.md` - how-to
- `docs/how-to/ai-gateway-route-budget.md` - how-to
- `docs/how-to/learn-and-publish-api-spec.md` - how-to
- `docs/aws-secure-deployment.md` - how-to
- `docs/production-readiness.md` - how-to/runbook
- `docs/release-packaging.md` - how-to
- `deploy/aws/README.md` - how-to/reference
- `internal/auth0-local-runbook.md` - runbook
- `internal/user-onboarding.md` - how-to
- `internal/issue-fix-workflow.md` - process how-to

Reference with runnable examples:

- `docs/reference/cli.md`
- `docs/reference/configuration.md`
- `docs/reference/rest-api.md`
- `docs/reference/errors.md`
- `docs/reference/filters.md`

Explanation/fact-check:

- `docs/README.md`
- `docs/concepts/tenancy-grants-xds.md`
- `docs/observability-alerts.md`
- `docs/secret-kek-rotation.md`
- `docs/production-readiness.md`
- `docs/failure-mode-matrix.md`
- `docs/adversarial-surface-map.md`
- `spec/*.md`
- `internal/*.md`
- `scripts/e2e/CERTIFICATION-REPORT.md`
- `scripts/e2e/COVERAGE.md`
- `DECISIONS.md`
- `REWRITE-REPORT.md`
- `docs-epic-review-log.md`

## Proof Doc: `README.md`

### Useful Commands / Build

Command:

```bash
cargo build --bin flowplane
```

Result: pass. Exit 0.

Excerpt:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s)
```

### Useful Commands / Main Binary Tests

Command:

```bash
cargo test -p flowplane
```

Result: pass. Exit 0.

Excerpt:

```text
test result: ok. 26 passed; 0 failed
```

### Useful Commands / Live Envoy Smoke Test

Command:

```bash
scripts/e2e-envoy.sh
```

Result: discrepancy. Exit 1.

Excerpt:

```text
postgres failed to start
```

Related issue: filed as PostgreSQL helper/runbook blocker.

### Useful Commands / OpenAPI

Command:

```bash
./target/debug/flowplane openapi
```

Result: pass. Exit 0.

Excerpt:

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "Flowplane"
  }
}
```

## Remaining Actionable Docs

### `docs/dev-dataplane.md` / Step 1

Command:

```bash
scripts/ensure-postgres.sh
```

Result: discrepancy. Exit 1.

Excerpt:

```text
postgres failed to start
```

Independent check:

```bash
psql postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev -c 'select 1'
```

Excerpt:

```text
FATAL:  role "postgres" does not exist
```

Workaround: none applied for DB-backed end-to-end docs. Later DB steps were inspection-only.

### `docs/tutorials/getting-started.md` / Step 1

Same PostgreSQL precondition as `docs/dev-dataplane.md`; blocked by the same role/precondition failure.

### `docs/aws-secure-deployment.md` / Local Dataplane Smoke

Command:

```bash
./target/debug/flowplane --out .local/aws-dp-cert.json cert issue edge-local --team demo
```

Result: discrepancy. Exit 2.

Excerpt:

```text
error: unrecognized subcommand 'cert'
Usage: flowplane [OPTIONS] <COMMAND>
```

Classification check:

```bash
./target/debug/flowplane dataplane cert --help
```

Excerpt:

```text
Commands:
  list
  register
  issue
  revoke
```

### `docs/how-to/cli-auth-and-contexts.md`

The built binary accepts the documented `config set-context` and `config get-contexts` commands. Full end-to-end auth verification requires a reachable control plane and token.

### `docs/how-to/ai-gateway-route-budget.md`

Inspection-only beyond CLI shape. The guide requires an authenticated team context, an existing secret, and JSON files created from prose blocks.

### `docs/how-to/learn-and-publish-api-spec.md`

Inspection-only beyond CLI shape. The guide requires an authenticated context, writable team, configured route, existing API definition, captured traffic, and a running control plane.

## Explanation / Policy Fact-Check

### `docs/README.md` / Cardinal Rule

Claim:

```text
Content pages here must not cite spec/ or internal/ as required reading.
```

Actual: user-facing docs contain deep links to `spec/` content, including:

- `docs/how-to/register-dataplane-mtls.md` -> `../../spec/05-auth.md`, `../../spec/04-xds.md`
- `docs/how-to/learn-and-publish-api-spec.md` -> `../../spec/06-learning.md`
- `docs/concepts/tenancy-grants-xds.md` -> multiple deep `../../spec/*.md` links
- `docs/aws-secure-deployment.md` references `internal/.env.prod-local`

Result: discrepancy. Filed as docs-policy/user-doc standalone bug.

## Issues Raised Or Updated

- #118: `[docs-verify] docs/README.md — user docs violate standalone spec/internal link policy`
  - URL: https://github.com/rajeevramani/flowplane-v2/issues/118
  - Severity: `major`
  - Classification: `doc-defect`
- #119: `[docs-verify] README.md — live Envoy smoke path fails on documented PostgreSQL helper`
  - URL: https://github.com/rajeevramani/flowplane-v2/issues/119
  - Severity: `blocker`
  - Classification: `doc-defect`
- #120: `[docs-verify] docs/aws-secure-deployment.md — cert issue command uses nonexistent top-level subcommand`
  - URL: https://github.com/rajeevramani/flowplane-v2/issues/120
  - Severity: `blocker`
  - Classification: `doc-defect`

## Counts

- Issues raised: 3
- By severity:
  - `blocker`: 2
  - `major`: 1
  - `minor`: 0
- By classification:
  - `doc-defect`: 3
  - `code-defect`: 0
  - `ambiguous`: 0
- Inspection-only / blocked docs: DB-backed end-to-end docs that require the documented local `postgres` role and database URL.
- Workarounds applied: none for DB-backed docs.
