# User Onboarding (internal)

How a human user goes from "exists in the IdP" to "can operate Flowplane resources". Derived from the auth code paths (`crates/fp-api/src/auth.rs`, `crates/fp-core/src/services/orgs.rs`, `crates/fp-domain/src/identity.rs`) and the CLI (`crates/flowplane/src/cli`).

## Identity model (read this first)

- **Identity = OIDC `sub`.** Flowplane trusts any compliant IdP (Q-004); we run Auth0. A user is keyed by their `sub` (e.g. `auth0|6a34…`), not their email.
- **The Flowplane user row is created lazily (JIT).** `flowplane auth login` only talks to the IdP — it does **not** create anything in Flowplane. The row is created on the user's **first authenticated request to the control plane** (`auth.rs` → `upsert_user_by_subject`). In practice: one `flowplane auth whoami` after login.
- **Authorization is org/team scoped (D-014).** Being a platform admin lets you run platform operations (create orgs/teams, add members) but does **not** grant context to read or write org/team-scoped resources. For those you must hold an actual membership.

## Roles

**Org roles** (`OrgRole`, hierarchical — each includes the powers below it):

| role     | notes |
|----------|-------|
| `viewer` | read-only |
| `member` | standard |
| `admin`  | org admin — **implicit access to every team in the org** (spec/05 §3.1) |
| `owner`  | org admin + ownership |

**Teams** have flat membership (no per-member role at the CLI). Capabilities beyond membership are assigned with `flowplane team grant …`. Org `admin`/`owner` already reach every team in their org.

## Onboarding a new user — steps

Assume the operator is the platform admin with a valid session (`flowplane auth whoami` shows `PLATFORM ADMIN true`), and `FLOWPLANE_SERVER` points at the CP.

### 1. Create the user in Auth0 (with verified email)
- Auth0 Dashboard → User Management → Users → Create User (Database connection), or via the Management API.
- The dashboard "Create User" form does **not** set `email_verified` → it defaults to `false`. Verify it one of two ways:
  - User → `…` → **Send Verification Email** (user clicks the link), or
  - Management API: `PATCH /api/v2/users/{id}` body `{"email_verified": true}`
    (URL-encode the `|` in the id as `%7C`).
- Flowplane itself does **not** require `email_verified` (the validator checks `iss/aud/exp/sub` only), but verify it anyway for hygiene and to avoid IdP-side prompts during login.

### 2. User signs in once (creates the Flowplane row)
The new user, on their own machine:
```bash
export FLOWPLANE_SERVER=https://<cp-host>
flowplane auth login --device \
  --issuer https://<tenant>.us.auth0.com/ \
  --client-id <auth0-app-client-id>
flowplane auth whoami     # <-- this authenticated call creates their user row
```
After this, `whoami` prints their `USER ID` and subject. (Concrete demo values live in the gitignored `internal/.env.prod-local`.)

### 3. Platform admin adds the user to an org
```bash
flowplane org member add <org> --role <viewer|member|admin|owner> \
  --subject "auth0|…"        # or --email <addr>, or --user-id <flowplane-uuid>
```
- Pick **one** identifier. `--subject` = OIDC sub, `--email` = email, `--user-id` = Flowplane internal **UUID** (from `whoami`/`org member list`). Passing an `auth0|…` value to `--user-id` fails with HTTP 422 (UUID parse error).
- The target user row must already exist (step 2), else `not_found: must sign in once`.

### 4. (If they need resource access) add to a team
Resources (clusters, listeners, routes, …) are team-scoped.
```bash
flowplane team create <team> --display-name "…"      # if the team doesn't exist
flowplane team member add --team <team> <email>      # flat membership, no role arg
flowplane team grant …                               # optional extra capabilities
```
Org `admin`/`owner` members implicitly reach every team and can skip explicit team membership.

### 5. User selects their context
Org/team-scoped commands need a selector. Either pass flags per call or set a context:
```bash
flowplane cluster list --org <org> --team <team>
# or persist it:
flowplane config set-context <name> --server https://<cp-host> --org <org> --team <team>
flowplane config use-context <name>
```

## Common errors → fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `not_found: … must sign in once before being added` | target user has no row | user runs `auth login` **+** `whoami` (step 2) first |
| HTTP 422 `UUID parsing failed … found 'u'` | `auth0|…` passed to `--user-id` | use `--subject` for the OIDC sub |
| `org_selector_required` (even with `--org`) | caller is not a **member** of that org; platform-admin alone is not org context (D-014); the platform org doesn't count | add the caller to the org (`org member add`), then retry |
| `team_selector_required` | resource is team-scoped, no team in context | pass `--team <team>` (or be org admin/owner) |
| `whoami` shows the wrong identity / `PLATFORM ADMIN false` | `auth login` **clobbers** the stored token | use separate CLI contexts per identity (`config set-context` / `use-context`), or re-login as the needed identity |
| `Could not resolve host` for the CP hostname | local resolver serving a stale/blocked record | `/etc/hosts` entry to the ALB/NLB IP, or `curl --resolve` (CLI uses the OS resolver) |

## Quick reference: the moving parts

- **Platform admin** is created once by bootstrap (`POST /api/v1/bootstrap/initialize`) bound to an OIDC subject.
- **One IdP user ⇒ one Flowplane user row**, created JIT on first authenticated CP call.
- **Org membership** grants org context + (admin/owner) all teams.
- **Team membership/grants** scope resource access.
- **Selectors** (`--org`, `--team`, or a saved context) put the request in the right tenant.
