# Zitadel POC

Local Zitadel instance for evaluating OIDC/RBAC integration with Flowplane.

## Start

```bash
cd poc/zitadel
docker compose up -d --wait
```

First startup takes 1-2 minutes while Zitadel initializes the database.

## Verify

```bash
# Health check
curl -s http://localhost:8080/debug/healthz

# OpenID configuration
curl -s http://localhost:8080/.well-known/openid-configuration | jq .
```

## Admin Console

Open http://localhost:8080/ui/console in a browser.

Login credentials:
- **Username**: `zitadel-admin@zitadel.zitadel`
- **Password**: `Password1!`

You will be prompted to change the password on first login.

## Get Admin PAT

1. Log into the console at http://localhost:8080/ui/console
2. Go to **Users** > select the admin user
3. Under **Personal Access Tokens**, click **New**
4. Copy the generated token

Or use the API after login:

```bash
# Get an OAuth token first
TOKEN=$(curl -s -X POST http://localhost:8080/oauth/v2/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=password&username=zitadel-admin@zitadel.zitadel&password=<YOUR_NEW_PASSWORD>&scope=openid urn:zitadel:iam:org:project:id:zitadel:aud" \
  | jq -r '.access_token')

echo $TOKEN
```

## Ports

| Service      | Port |
|-------------|------|
| Zitadel     | 8080 |
| PostgreSQL  | 5433 |

PostgreSQL uses port 5433 to avoid conflict with Flowplane's database on 5432.

## Stop

```bash
docker compose down        # stop, keep data
docker compose down -v     # stop and delete all data
```
