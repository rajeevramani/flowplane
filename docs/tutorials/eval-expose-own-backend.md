# Expose your own backend

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

**Outcome:** Attach a host backend to the existing listener and verify that both routes work.

**Prerequisites and starting state:** Complete [Expose and call your first API](eval-expose-first-api.md). Start with listener `demo` on port `10000` and the working sample route `/`. Keep `fp` available in your evaluation terminal. Python 3 is required only if you use the sample host backend below.

Run commands individually and inspect each result. **Stop on failures or unexpected results; do not continue blindly.** These are manual steps, not an onboarding script.

**Release boundary:** matching v3.2.0 container images are not yet published. This draft is not packaged-candidate or cross-platform qualification. See the [evaluation learning path](evaluate-no-clone.md) for the shared release and platform limitations.

The actually exercised host-backend variant used Docker Compose connected to a **Linux-arm64 Podman VM**, `host.containers.internal`, and a Python backend bound to host loopback. This is source-bound local supporting evidence, not packaged 3.2.0 or Docker Desktop qualification. Docker Desktop/macOS and Linux Docker host-gateway alternatives below are **unverified in this draft**; select and verify your engine's setup before exposing. Docker and Podman qualification are not interchangeable.

Use an existing reachable API, or start this harmless backend in a **second terminal**. The loopback variant is the one exercised locally:

```sh
mkdir flowplane-own-backend
```

```sh
cd flowplane-own-backend
```

```sh
printf 'hello from my API\n' > hello
```

```sh
python3 -m http.server 3001 --bind 127.0.0.1
```

Stop it with Ctrl-C when finished. Reachability is from **Envoy**, not your laptop's CLI:

| Backend location | Upstream URL / qualification boundary |
|---|---|
| Locally exercised Podman VM host | `http://host.containers.internal:3001`; loopback worked in that VM. Verify it on another Podman setup; this is not a universal guarantee. |
| Docker Desktop/macOS host — unverified here | `http://host.docker.internal:3001`; verify that Docker can reach your backend before attachment. |
| Linux Docker host — unverified here | Add the override below, recreate Envoy/agent, then use `http://host.docker.internal:3001`. |
| Service on the Compose network | `http://<service-name>:<container-port>`; attach your service to that network. |

**Host-backend warning:** if another engine cannot reach a loopback-only backend, use an interface reachable from that engine and a firewall restriction appropriate for the test. Binding `0.0.0.0` makes it reachable beyond host loopback; that alternative was not exercised here. Serve only harmless sample data and stop the server afterwards. Never use `http://127.0.0.1:3001` as a host upstream when Envoy is containerized: it means the Envoy container itself.

For the **unverified Linux Docker alternative**, save this additional file as `compose.host.yml` and apply it **before** the attachment command:

```yaml
services:
  envoy:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

Remove only the agent container first: it shares Envoy's network namespace and can otherwise block Envoy replacement. Recreate both services with the override; retain the database/shared volumes. Existing evaluation traffic may briefly pause:

```sh
docker compose -f compose.eval.yml rm -sf flowplane-agent
```

```sh
docker compose -f compose.eval.yml -f compose.host.yml up -d --no-build envoy flowplane-agent
```

In the **first terminal**, attach the locally exercised Podman backend. On a verified Docker setup, substitute `http://host.docker.internal:3001`; do not run both variants.

```sh
fp expose http://host.containers.internal:3001 --name own --path /hello --listener demo
```

Expected: the result reports attachment to the existing listener/config; no second gateway port is allocated. If it fails, stop and inspect reachability and resources before retrying.

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/hello
```

Expected body: `hello from my API`.

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/
```

Expected body: `hello from the flowplane eval demo upstream`. This checks the original route still works. If configuration has not arrived yet, wait briefly and rerun the reads manually; stop if either body remains wrong. `/hello` is forwarded unchanged and the sample backend implements that path. TLS/remote backends must satisfy their own network, trust and authentication requirements.

`--listener demo` attaches to the actual listener and its route config; it does not allocate a second port. The narrower `/hello` Prefix is inserted before `/`, with existing relative order preserved. A Prefix `/hello` also matches `/helloworld`; an earlier Exact exception can take precedence. Attachment supports a same-team user-owned HTTP listener with one wildcard virtual host and simple Prefix/Exact routes; unsupported shapes fail rather than guess.

A route-config edit affects **every listener sharing the named route config**. Inspect the reported resource names before changing a shared config. Do not combine `--listener` with `--port` or `--public-base-url`.

## Continue

[Next: Add a local rate limit](eval-local-rate-limit.md). For failures, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md); do not retry mutations as readiness checks.
