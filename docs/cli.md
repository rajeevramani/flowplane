# Flowplane CLI

The Flowplane CLI (`flowplane-cli`) provides command-line access for managing resources, authentication, database operations, and configuration.

## Installation

The CLI binary is included in release archives:

```bash
# After extracting the release
./flowplane-cli --help
```

Build from source:

```bash
cargo build --release --bin flowplane-cli
./target/release/flowplane-cli --help
```

## Configuration

The CLI stores configuration at `~/.flowplane/config.toml`.

### Initialize Configuration

```bash
flowplane-cli config init
flowplane-cli config init --force  # Overwrite existing
```

### Set Configuration Values

```bash
# API token
flowplane-cli config set token fp_pat_xxxxx

# API base URL (default: http://localhost:8080)
flowplane-cli config set base_url https://api.example.com

# Request timeout in seconds (default: 30)
flowplane-cli config set timeout 60
```

### View Configuration

```bash
flowplane-cli config show
flowplane-cli config show --output json
flowplane-cli config path  # Show config file location
```

### Configuration File Format

```toml
token = "fp_pat_xxxxx"
base_url = "https://api.example.com"
timeout = 60
```

## Global Options

All commands support these flags:

| Flag | Description |
|------|-------------|
| `--token <TOKEN>` | API token (overrides config file) |
| `--token-file <PATH>` | Path to file containing token |
| `--base-url <URL>` | API base URL (overrides config file) |
| `--timeout <SECONDS>` | Request timeout |
| `-v, --verbose` | Enable debug logging |

### Resolution Priority

**Token**: `--token` flag > `--token-file` flag > config file > `FLOWPLANE_TOKEN` env var

**Base URL**: `--base-url` flag > config file > `FLOWPLANE_BASE_URL` env var > `http://localhost:8080`

**Timeout**: `--timeout` flag > config file > 30 seconds

## Commands

### Database Management

Manage database schema and migrations.

```bash
# Run pending migrations
flowplane-cli database migrate

# Preview migrations without applying
flowplane-cli database migrate --dry-run

# Check migration status
flowplane-cli database status

# List applied migrations
flowplane-cli database list

# Validate database schema
flowplane-cli database validate
```

### Authentication & Tokens

Manage system bootstrap and personal access tokens.

#### Bootstrap

Initialize the system with an admin user:

```bash
flowplane-cli auth bootstrap \
  --email admin@example.com \
  --password secure123 \
  --name "Admin User" \
  --api-url http://localhost:8080
```

Environment variables can also be used:
- `FLOWPLANE_ADMIN_EMAIL`
- `FLOWPLANE_ADMIN_PASSWORD`
- `FLOWPLANE_ADMIN_NAME`
- `FLOWPLANE_BASE_URL`

#### Create Token

Generate a new API token with specific scopes:

```bash
flowplane-cli auth create-token \
  --name ci-token \
  --description "CI/CD pipeline token" \
  --scope clusters:read \
  --scope routes:write \
  --expires-in 90d \
  --created-by ci-system
```

Options:
- `--name <NAME>` (required) - Token name (3-64 alphanumeric chars)
- `--description <DESC>` - Human-readable description
- `--scope <SCOPE>` (required, repeatable) - Token scopes
- `--expires-at <RFC3339>` - Absolute expiration timestamp
- `--expires-in <DURATION>` - Relative expiration (e.g., `90d`, `12h`, `30m`)
- `--created-by <ID>` - Creator identifier for audit
- `--api-url <URL>` - Use API mode instead of direct database access

#### List Tokens

```bash
flowplane-cli auth list-tokens
flowplane-cli auth list-tokens --limit 10 --offset 0
flowplane-cli auth list-tokens --api-url http://api.example.com
```

#### Revoke Token

Permanently disable a token:

```bash
flowplane-cli auth revoke-token <TOKEN_ID>
```

#### Rotate Token

Generate a new secret value (old secret is invalidated):

```bash
flowplane-cli auth rotate-token <TOKEN_ID>
```

### Clusters

Manage Envoy cluster configurations (upstream service endpoints).

#### Create

```bash
flowplane-cli cluster create --file cluster.json
flowplane-cli cluster create --file cluster.json --output yaml
```

#### List

```bash
flowplane-cli cluster list
flowplane-cli cluster list --service backend-api
flowplane-cli cluster list --limit 10 --offset 0 --output table
```

#### Get

```bash
flowplane-cli cluster get my-cluster
flowplane-cli cluster get my-cluster --output yaml
```

#### Update

```bash
flowplane-cli cluster update my-cluster --file updated.json
```

#### Delete

```bash
flowplane-cli cluster delete my-cluster
flowplane-cli cluster delete my-cluster --yes  # Skip confirmation
```

### Listeners

Manage Envoy listener configurations (network listeners for incoming connections).

#### Create

```bash
flowplane-cli listener create --file listener.json
```

#### List

```bash
flowplane-cli listener list
flowplane-cli listener list --protocol http
flowplane-cli listener list --limit 10 --output table
```

#### Get

```bash
flowplane-cli listener get http-listener
flowplane-cli listener get http-listener --output yaml
```

#### Update

```bash
flowplane-cli listener update http-listener --file updated.json
```

#### Delete

```bash
flowplane-cli listener delete http-listener
flowplane-cli listener delete http-listener --yes
```

### Routes

Manage route configurations (path matching and routing rules).

#### Create

```bash
flowplane-cli route create --file route.json
```

#### List

```bash
flowplane-cli route list
flowplane-cli route list --cluster backend-api
flowplane-cli route list --limit 10 --output table
```

#### Get

```bash
flowplane-cli route get api-routes
flowplane-cli route get api-routes --output yaml
```

#### Update

```bash
flowplane-cli route update api-routes --file updated.json
```

#### Delete

```bash
flowplane-cli route delete api-routes
flowplane-cli route delete api-routes --yes
```

### Teams

Manage teams for multi-tenant resource isolation.

#### Create

```bash
flowplane-cli team create --file team.json
```

#### List

```bash
flowplane-cli team list
flowplane-cli team list --limit 10 --output table
```

#### Get

```bash
flowplane-cli team get team-123
flowplane-cli team get team-123 --output yaml
```

#### Update

```bash
flowplane-cli team update team-123 --file updated.json
```

#### Delete

```bash
flowplane-cli team delete team-123
flowplane-cli team delete team-123 --yes
```

Note: Delete fails if the team owns resources due to foreign key constraints.

## Output Formats

Resource commands support three output formats via `-o, --output`:

| Format | Description | Default For |
|--------|-------------|-------------|
| `json` | JSON output | create, get, update |
| `yaml` | YAML output for readability | - |
| `table` | Tabular output | list |

## Token Scopes

Available scopes for API tokens:

| Scope | Description |
|-------|-------------|
| `admin:all` | Full administrative access |
| `clusters:read` | Read cluster configurations |
| `clusters:write` | Create/update/delete clusters |
| `routes:read` | Read route configurations |
| `routes:write` | Create/update/delete routes |
| `listeners:read` | Read listener configurations |
| `listeners:write` | Create/update/delete listeners |
| `teams:read` | Read team information |
| `teams:write` | Create/update/delete teams |
| `secrets:read` | Read secrets |
| `secrets:write` | Create/update/delete secrets |

## Examples

### Complete Workflow

```bash
# 1. Initialize CLI config
flowplane-cli config init

# 2. Bootstrap admin user (first-time setup)
flowplane-cli auth bootstrap \
  --email admin@example.com \
  --password secure123 \
  --name "Admin"

# 3. Create an API token
flowplane-cli auth create-token \
  --name admin-token \
  --scope admin:all \
  --expires-in 30d

# 4. Save token to config
flowplane-cli config set token fp_pat_xxxxx

# 5. Create resources
flowplane-cli cluster create --file cluster.json
flowplane-cli route create --file route.json
flowplane-cli listener create --file listener.json

# 6. Verify
flowplane-cli cluster list --output table
flowplane-cli listener list --output table
```

### CI/CD Integration

```bash
# Use environment variable for token
export FLOWPLANE_TOKEN="fp_pat_xxxxx"
export FLOWPLANE_BASE_URL="https://api.example.com"

# Deploy configuration
flowplane-cli cluster create --file clusters/production.json
flowplane-cli route create --file routes/production.json
flowplane-cli listener create --file listeners/production.json
```

### Scripting with JSON Output

```bash
# Get cluster and parse with jq
flowplane-cli cluster get my-cluster --output json | jq '.config.endpoints[0].host'

# List all cluster names
flowplane-cli cluster list --output json | jq -r '.[].name'
```
