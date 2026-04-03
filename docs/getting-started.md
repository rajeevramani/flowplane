# Getting Started

This guide walks you through setting up Flowplane in dev mode and exposing your first service through Envoy.

## Prerequisites

- **Docker** (Docker Desktop, OrbStack, Rancher Desktop, or Podman)
- **Rust** (stable, 1.75+) with cargo

## Install

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
cargo install --path . --locked
```

This installs the `flowplane` CLI binary.

## Boot the stack

```bash
flowplane init --with-envoy --with-httpbin
```

This starts PostgreSQL, the Flowplane control plane, Envoy, and httpbin in Docker containers. A dev token is generated and saved to `~/.flowplane/credentials` automatically.

Services after boot:

| Service | Address |
|---|---|
| Control plane API | http://localhost:8080 |
| Swagger UI | http://localhost:8080/swagger-ui/ |
| xDS server | localhost:18000 (gRPC) |
| Envoy proxy | localhost:10000 (admin), ports 10001-10020 (listeners) |
| httpbin | http://localhost:8000 |

> Envoy listeners use ports **10001-10020**. The `expose` command auto-assigns the next available port in this range.

## Verify

```bash
flowplane status
curl http://localhost:8080/health
```

`flowplane status` shows a system overview (control plane, Envoy, database). The `/health` endpoint returns `200 OK` when the control plane is ready.

## Expose a service

The `expose` command creates all the gateway resources (cluster, route, listener) in one step:

```bash
flowplane expose http://httpbin:80 --name demo
```

```
Exposed 'demo' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```

This created a cluster pointing to `httpbin:80`, a route config forwarding `/` to that cluster, and a listener on port `10001`. The port is auto-assigned from the range 10001-10020.

### Test it

Send a request through the Envoy gateway:

```bash
curl http://localhost:10001/get
```

```json
{
  "args": {},
  "headers": {
    "Accept": "*/*",
    "Host": "localhost:10001",
    "User-Agent": "curl/8.7.1",
    "X-Envoy-Expected-Rq-Timeout-Ms": "15000"
  },
  "origin": "10.89.0.13",
  "url": "http://localhost:10001/get"
}
```

The `X-Envoy-Expected-Rq-Timeout-Ms` header confirms the request went through Envoy.

Try other httpbin endpoints:

```bash
curl http://localhost:10001/status/200    # Returns 200 OK
curl http://localhost:10001/headers       # Echo request headers
curl -X POST http://localhost:10001/post -d '{"key":"value"}'  # Echo POST body
```

### Check what's exposed

```bash
flowplane list
```

```
Name                           Port     Protocol
-------------------------------------------------------
demo                           10001    HTTP
```

```bash
flowplane status
```

```
Flowplane Status (team: default)
----------------------------------------
Listeners:  1
Clusters:   1
Filters:    0
```

### Expose options

| Flag | Description |
|---|---|
| `--name <NAME>` | Service name (auto-generated from URL if omitted) |
| `--path <PATH>` | Path prefix to route (repeatable, defaults to `/`) |
| `--port <PORT>` | Port override (auto-assigned from 10001-10020 if omitted) |

### Remove an exposed service

```bash
flowplane unexpose demo
```

```
Removed exposed service 'demo'
```

This tears down the listener, route config, and cluster in one step.
