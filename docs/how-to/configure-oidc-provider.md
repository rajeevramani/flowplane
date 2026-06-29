# How-to: configure an OIDC provider

> Audience: operators, platform-engineers · Status: stable

Flowplane accepts user tokens from any OpenID Connect provider that publishes a discovery document and JWKS. The control plane validates identity; authorization still comes from Flowplane org membership, team membership, and grants.

Use this page before bootstrapping the first platform admin. You need two OIDC integrations:

- a control-plane API audience that Flowplane validates on every request;
- a public CLI client that users can use with PKCE and, if supported by your IdP, device authorization.

Do not use Flowplane dev mode for production identity. In non-dev deployments, authenticated endpoints fail closed when OIDC is missing or invalid.

## 1. Create the API audience

In your IdP, create or identify the API/resource that will issue tokens for Flowplane.

Record:

| Value | Flowplane setting | Notes |
|-------|-------------------|-------|
| Issuer URL | `FLOWPLANE_OIDC_ISSUER` | The exact issuer value in tokens and discovery metadata, for example `https://idp.example.com/realms/platform`. |
| Audience | `FLOWPLANE_OIDC_AUDIENCE` | The expected `aud` claim for Flowplane API tokens. |
| JWKS URI | `FLOWPLANE_OIDC_JWKS_URI` | Optional. Leave unset when discovery returns the correct `jwks_uri`. Set only when your IdP requires an override. |
| CA bundle | `FLOWPLANE_OIDC_CA_BUNDLE` | Optional PEM bundle path for private enterprise roots or TLS-intercepting egress proxies. Invalid bundles fail startup closed. |

The issuer and audience must be configured together on the control plane. With neither configured and dev mode off, authenticated endpoints return `503`; with only one configured, startup fails.

## 2. Create the CLI client

Create an OIDC/OAuth client for the `flowplane auth login` command.

Required client properties:

| Property | Value |
|----------|-------|
| Client type | Public/native client. Do not require a client secret for CLI login. |
| Redirect URI | `http://127.0.0.1:8976/callback` unless you set `FLOWPLANE_OIDC_CALLBACK_URL` and register that exact callback. |
| Grant types | Authorization code with PKCE. Device authorization is optional but recommended for headless operators. |
| Scopes | At least `openid`; `email profile` are recommended so Flowplane can display useful user metadata. |
| Audience/resource | Configure your IdP so issued tokens include the Flowplane API audience from step 1. |

Record the CLI client id as `FLOWPLANE_OIDC_CLIENT_ID`. The CLI reads:

```bash
export FLOWPLANE_OIDC_ISSUER="https://idp.example.com/realms/platform"
export FLOWPLANE_OIDC_CLIENT_ID="<flowplane-cli-client-id>"
export FLOWPLANE_OIDC_SCOPE="openid email profile"
export FLOWPLANE_OIDC_CALLBACK_URL="http://127.0.0.1:8976/callback"
```

Use device flow when your IdP advertises `device_authorization_endpoint`:

```bash
flowplane auth login --device-code \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
  --scope "$FLOWPLANE_OIDC_SCOPE"
```

Use PKCE browser login otherwise:

```bash
flowplane auth login --pkce \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
  --callback-url "$FLOWPLANE_OIDC_CALLBACK_URL" \
  --scope "$FLOWPLANE_OIDC_SCOPE"
```

## 3. Configure the control plane

Set OIDC on every control-plane replica:

```bash
export FLOWPLANE_OIDC_ISSUER="https://idp.example.com/realms/platform"
export FLOWPLANE_OIDC_AUDIENCE="<flowplane-api-audience>"
```

Optional overrides:

```bash
export FLOWPLANE_OIDC_JWKS_URI="https://idp.example.com/realms/platform/protocol/openid-connect/certs"
export FLOWPLANE_OIDC_CA_BUNDLE="/etc/flowplane/trust/enterprise-ca.pem"
```

Only set `FLOWPLANE_OIDC_CA_BUNDLE` when the control plane must trust a private or intercepted TLS path to fetch OIDC discovery and JWKS. The bundle adds trust; it does not disable normal certificate validation.

## 4. Find the first admin's OIDC subject

`admin_subject` is the immutable OIDC `sub` claim for the first platform admin. It is not an email address, display name, username, or group name.

Use one of these safe methods:

- In your IdP admin UI, open the user profile and copy the stable user identifier that the IdP emits as `sub`.
- Run a temporary OIDC login for the intended admin and inspect that user's own ID token with a local JWT decoder. Decode only tokens from your own account; do not paste production tokens into third-party websites.
- If your IdP has a userinfo endpoint, call it with the intended admin's own access token and copy the `sub` field from the response.

Keep a note of the exact string for bootstrap:

```text
admin_subject = "<oidc-sub-of-first-admin>"
```

The value is safe to store in configuration management as an identifier, but it is still account metadata. Do not use a real subject in public examples.

## 5. Verify after bootstrap

After [bootstrapping the first platform admin](bootstrap-platform.md), log in as that same user and confirm Flowplane resolved the platform-admin identity:

```bash
flowplane auth login --device-code \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID"

flowplane auth whoami
```

If login succeeds but `whoami` does not show platform-admin access, re-check the exact `sub` used during bootstrap. Email and display-name changes do not affect `sub`; using email as `admin_subject` will not match the OIDC identity.

## Troubleshooting

- **Authenticated endpoints return `503`:** the control plane is running without a complete OIDC issuer/audience pair and dev mode is off. Configure both `FLOWPLANE_OIDC_ISSUER` and `FLOWPLANE_OIDC_AUDIENCE`.
- **Startup fails on OIDC config:** check that issuer and audience are set together and that `FLOWPLANE_OIDC_CA_BUNDLE`, if set, points to a readable PEM bundle.
- **CLI says the provider has no device endpoint:** use `flowplane auth login --pkce`, or enable device authorization for the CLI client in your IdP.
- **Tokens validate at the IdP but Flowplane rejects them:** confirm the token issuer equals `FLOWPLANE_OIDC_ISSUER`, the audience includes `FLOWPLANE_OIDC_AUDIENCE`, and the control plane can fetch discovery and JWKS.
