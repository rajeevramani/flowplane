# Flowplane

Flowplane is a control plane for [Envoy proxy](https://www.envoyproxy.io/) that lets you configure API gateways through a CLI, REST API, or MCP server. It manages clusters (upstream services), listeners (ports), routes (path matching), and filters (rate limiting, JWT auth, CORS, and more) — all stored in PostgreSQL and pushed to Envoy via xDS. AI agents can drive the entire workflow through 60+ MCP tools.

## Quickstart

```bash
git clone https://github.com/rajeevramani/flowplane.git && cd flowplane
cargo install --path . --locked                                     # Build the CLI
flowplane init --with-envoy --with-httpbin                 # Start everything
flowplane expose http://httpbin:80 --name demo             # Expose httpbin
curl http://localhost:10001/get                             # Verify traffic
```

| Service | URL |
|---------|-----|
| API     | http://localhost:8080/api/v1/ |
| UI      | http://localhost:8080/ |
| httpbin | http://localhost:8000 |

Exposed services are available on auto-assigned ports in the 10001–10020 range. The `expose` command prints the assigned port.

## Documentation

- [Getting Started](docs/getting-started.md) — Install, expose a service, add rate limiting
- [CLI Reference](docs/cli-reference.md) — Every command, flag, and example
- [Filters](docs/filters.md) — Rate limiting, JWT auth, CORS, and more
- [MCP Server](docs/mcp.md) — Use Flowplane as an MCP server with Claude Code

## Production Mode

For multi-user deployments with Zitadel authentication:

```bash
make up HTTPBIN=1 ENVOY=1    # Full stack with Zitadel
make seed                     # Create demo org and credentials
flowplane auth login          # OIDC login
```

See [Quickstart](docs/quickstart.md) for the full walkthrough.

## Requirements

- Docker (or Podman)
- Rust 1.92+ (for building from source)
- Node.js 18+ (for the UI)

## License

MIT
