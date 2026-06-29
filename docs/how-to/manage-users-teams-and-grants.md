# How-to: manage users, teams, and grants

> Audience: platform-engineers, operators · Status: stable

Use this runbook to move from a tenant org to users, team membership, and least-privilege grants. It is the lifecycle companion to [Create a tenant org and a team](create-tenant-org-and-team.md) and [Onboard an API team](onboard-api-team.md).

## Ownership model

Flowplane has three separate administration layers:

- platform admin creates tenant orgs and can seed the first tenant owner;
- tenant org owners/admins manage org members, teams, team members, and team grants inside their own org;
- API-team users exercise only the team grants they have been given, unless they are also org owners/admins.

The platform org is governance-only. It cannot host tenant teams, dataplanes, or gateway resources, and platform-admin status does not grant tenant-resource access.

## 1. Add users to the tenant org

The user must sign in once before they can be added by email or subject. Add them to the tenant org with the smallest org role that fits:

```bash
flowplane org member add edgeco --email api-dev@example.com --role member
flowplane org member list edgeco
```

Use `--subject <oidc-sub>` instead of `--email` when you need to bind the exact immutable OIDC subject:

```bash
flowplane org member add edgeco --subject <oidc-sub-of-api-dev> --role member
```

Use `admin` or `owner` only for people who should manage teams and grants across the tenant org. Org admins and owners have implicit access to tenant resources in their org; ordinary API-team users should use explicit team grants.

## 2. Create or select the team

Create the team while selecting the tenant org:

```bash
flowplane team create payments --org edgeco --display-name "Payments"
flowplane team list --org edgeco
```

Team names are tenant-scoped. Keep API teams in tenant orgs, never in the platform org.

## 3. Add team members

Add org users to the team:

```bash
flowplane team member add api-dev@example.com --org edgeco --team payments
flowplane team member list --org edgeco --team payments
```

Team membership lets the team roster be managed and inspected. Resource access still comes from org-admin status or explicit grants.

## 4. Grant the API-team workflow

Grant the smallest resource/action set for the task. For a user who imports an OpenAPI document, publishes it, and verifies MCP tools:

```bash
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action update
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource mcp-tools --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource mcp-tools --action update
```

Add gateway modeling grants only if the API team will create or update the route/listener resources themselves:

```bash
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource clusters --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource clusters --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource route-configs --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource route-configs --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource listeners --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource listeners --action read
```

For learning/discovery, add the learning-session actions the workflow needs:

```bash
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource learning-sessions --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource learning-sessions --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource learning-sessions --action execute
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource learning-sessions --action delete
```

The grant vocabulary is closed. Tenant resource strings are `clusters`, `route-configs`, `listeners`, `filters`, `secrets`, `dataplanes`, `proxy-certificates`, `agents`, `grants`, `api-definitions`, `learning-sessions`, `mcp-tools`, `rate-limits`, `ai-providers`, `ai-routes`, `ai-budgets`, `ai-usage`, and `stats`. Actions are `read`, `create`, `update`, `delete`, and `execute`. Governance resources such as `organizations`, `users`, `teams`, `audit`, and `platform` cannot be granted at team scope.

## 5. Audit and revoke

List grants before and after changes:

```bash
flowplane team grant list --org edgeco --team payments
```

Revoke by grant id:

```bash
flowplane team grant remove <grant-id> --org edgeco --team payments
```

Remove team membership when the user no longer belongs on the team:

```bash
flowplane team member remove <user-id> --org edgeco --team payments
```

Remove org membership only after checking that you are not removing the last owner:

```bash
flowplane org member remove edgeco <user-id>
```

## 6. Handoff to the API team

Give the API team:

- control-plane URL;
- tenant org and team names;
- OIDC CLI login values;
- the grants they received;
- whether a listener binding already exists;
- which guide to follow next: [Onboard an API team](onboard-api-team.md).

Do not send bootstrap tokens, bearer tokens, private keys, provider API keys, or certificate PEM bodies in the handoff.
