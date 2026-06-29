# How-to: create a tenant org and a team

> Audience: operators · Status: stable

A freshly bootstrapped control plane has exactly one organization — the **platform
org** — and one platform admin. The platform org is for **governance only**: it
**cannot host tenant teams, dataplanes, or gateway config**. Before you can register
a dataplane or define any gateway resource, you create a **tenant org** and a **team**
inside it.

This page takes you from "platform admin, nothing else" to "a team you can create
dataplanes under." Every value you need is here.

## Prerequisites

- The control plane is initialized and you are logged in as the **platform admin**
  (see [bootstrap the first platform admin](bootstrap-platform.md)). Confirm with:

  ```bash
  flowplane auth whoami      # platform_admin: true
  ```

- Your CLI points at the control plane (`--server` / `FLOWPLANE_SERVER` / a saved
  context — see [CLI auth and contexts](cli-auth-and-contexts.md)).

> **Using dev mode from Getting Started?** The seeded `dev-user` is an owner of
> `dev-org` and a member of `default`, but it is **not** a platform admin and cannot run
> `org create`. Use the seeded `dev-org` / `default` context for local gateway
> exploration. This guide is for a non-dev context where you hold a real
> platform-admin identity and need to provision an additional tenant org and team.

## 1. Create the tenant org

Only a platform admin can create an organization. `name` is the immutable handle;
`--display-name` is an optional label.

```bash
flowplane org create edgeco --display-name "EdgeCo"
```

This calls `POST /api/v1/orgs`. Creating the org does **not** make you a member of it
— org membership is a separate, explicit step.

## 2. Add the org's first owner

A tenant org needs an **owner** before anyone can administer it. A platform admin may
add the **first** member of an org (and only while the org still has no owner); after
that, the org's own owners/admins manage membership.

Identify the user by their immutable OIDC subject (preferred), or by email:

```bash
flowplane org member add edgeco --role owner --subject <oidc-sub-of-first-owner>
# or:  flowplane org member add edgeco --role owner --email you@example.com
```

This calls `POST /api/v1/orgs/{org}/members`. Pass exactly one of `--subject`,
`--email`, or `--user-id`.

> **Running a local non-dev bootstrap?** Add **your own** subject as the owner. You
> are then both the platform admin *and* the owner of `edgeco`; the team step below
> authorizes through your `edgeco` org membership, not your platform role — the
> platform role never grants tenant access. This is separate from dev mode's
> seeded `dev-user`, which is not a platform admin.

## 3. Create a team in the tenant org

Teams own gateway resources. Create one while **selecting the tenant org** with the
global `--org` flag (it is sent as the `X-Flowplane-Org` request context):

```bash
flowplane team create payments --org edgeco --display-name "Payments"
```

This calls `POST /api/v1/teams` in the `edgeco` org context. `--org` is a **global**
flag — it selects the active org for the request; there is no per-command `--org` on
`team create`.

## 4. Verify

```bash
flowplane team list --org edgeco        # payments is listed
```

You can now create dataplanes and gateway config under `edgeco` / `payments` — for
example [register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md).

## Why `--org platform` is rejected

If you try to host a team in the platform org — `--org platform`, or no org selector
when you only belong to the platform org — the request fails closed:

```text
HTTP 400  org_selector_required
```

This is intentional ([D-014](../../DECISIONS.md)). The platform org is **never** a
selectable tenant context, and the platform admin role is governance-only — it cannot
see inside or host tenant resources. The same error appears when you belong to several
tenant orgs and send no selector; the fix is the same: name the tenant org with
`--org <org>` (or set an active context).

## Next step

- [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md)
  — create a dataplane under the `edgeco` / `payments` org+team you just made.
- [Manage users, teams, and grants](manage-users-teams-and-grants.md)
  — add API-team users and grant least-privilege access inside the tenant org.
