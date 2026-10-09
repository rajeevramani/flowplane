# Install and verify Flowplane

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

**Outcome:** Start the evaluation infrastructure and verify that no APIs are exposed.

**Prerequisites and starting state:** Docker with Compose, or a Podman VM with Docker Compose configured against its engine socket; `curl` and a POSIX shell. The commands use `docker compose`. Start in a fresh directory with no existing evaluation state.

Run commands individually and inspect each result. **Stop on failures or unexpected results; do not continue blindly.** These are manual steps, not an onboarding script.

**Release boundary:** matching v3.2.0 container images are not yet published. This draft is not packaged-candidate or cross-platform qualification. See the [evaluation learning path](evaluate-no-clone.md) for the shared release and platform limitations.

The local-only dev bundle seeds identities, supplies a demo backend, and automatically registers and bootstraps the dataplane over xDS mTLS, **without exposing any API**. Host ports bind to `127.0.0.1`; this is not a production deployment. Older `3.1.x` bundles seed demo traffic and do not implement this journey.

## Download and start

Run each command separately and inspect its result. **Stop if a command fails; do not continue until you understand and resolve the error.** Do not enable global `set -e` in your interactive terminal. These steps are manual, not a script to paste and run in one batch.

Use a new directory; do not overwrite another stack's Compose file or reuse its volumes.

```sh
mkdir flowplane-evaluation
```

```sh
cd flowplane-evaluation
```

```sh
VER=3.2.0
```

```sh
curl -fsSLO "https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml"
```

Expected: `compose.eval.yml` is downloaded. If the version is not published or the download fails, stop here.

```sh
export FLOWPLANE_EVAL_IMAGE="ghcr.io/rajeevramani/flowplane:${VER}-eval"
```

```sh
docker compose -f compose.eval.yml up -d --no-build
```

Expected: Compose starts the services using the published image, without building source. Do not proceed if Compose reports an error.

### Define the CLI helper

Define `fp` in this terminal. It reads the token inside the container without printing it and forwards your CLI arguments. It does not install services, wait for readiness or validate results. Keep this terminal open and remain in the evaluation directory.

```sh
fp() {
  docker compose -f compose.eval.yml exec -T flowplane-eval \
    sh -ec 'export FLOWPLANE_SERVER=http://127.0.0.1:8080 FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default; FLOWPLANE_TOKEN="$(cat /shared/dev-token)"; export FLOWPLANE_TOKEN; exec flowplane "$@"' sh "$@"
}
```

`-T` avoids allocating a terminal and supports the explicit file-transfer command later. File arguments passed through `fp` refer to container paths, not paths on your host.

### Inspect readiness

```sh
docker compose -f compose.eval.yml ps -a
```

Expected: long-running services are running and setup/verification services have completed with exit code `0`. `Exited (0)` is normal for successful one-shot setup services. Inspect unhealthy services and nonzero setup exits before continuing.

```sh
fp auth whoami
```

Expected: authenticated identity and organization information.

```sh
fp -o json dataplane get dp-eval
```

Inspect `data.last_heartbeat_at`: it must be non-null and recent. Wait a few seconds and run the same read again to see it advance. A previously recorded, stale heartbeat is not readiness. If authentication fails or the timestamp remains missing/stale, **stop before exposure** and inspect the setup and agent logs:

```sh
docker compose -f compose.eval.yml logs flowplane-eval init flowplane-agent envoy
```

**Sensitive diagnostic output:** control-plane boot logs include the dev management bearer token (`dev_token`). Redact it, dashboard nonce URLs and other credentials before copying, archiving or sharing. Never use Envoy admin as an operator API. See [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md) for diagnosis.

### Inspect the empty gateway

```sh
fp cluster list
```

```sh
fp route list
```

```sh
fp listener list
```

Expected: all three inventories are empty on a fresh installation. If they are not, stop and inspect: you may be resuming existing state. Do not delete resources just to make this check pass.

Before exposure, try the gateway once:

```sh
curl --max-time 3 -sS http://127.0.0.1:10000/
```

Expected: connection refusal/reset, not a successful sample response. A refusal is not an infrastructure failure here: publishing a container port does not create an Envoy listener. If you receive the sample body, stop and inspect which stack and gateway you are calling.

## Continue

[Next: Expose and call your first API](eval-expose-first-api.md). For failures, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md); do not retry mutations as readiness checks.
