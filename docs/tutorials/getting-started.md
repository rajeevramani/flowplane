# Getting Started with Flowplane

> Audience: newcomers · Status: stable

This tutorial walks you through one happy path from a clean checkout to a working gateway: you start the Flowplane control plane in **dev mode**, expose a single HTTP upstream through it, connect a local Envoy, and finish with a `curl` that reaches your upstream *through the gateway*.

Follow the steps in order. Every command here is copy-pasteable.

Dev mode runs an in-process identity issuer, seeds local resources, and serves the API and xDS over plaintext. It is for local exploration only — never production.

> **Just want to try Flowplane?** You don't need this tutorial. The fastest path uses the published
> evaluation image and a single `docker compose` file — no clone, no Rust toolchain — and routes a
> real request in four commands. See **[Quick Start (no clone)](../../README.md#quick-start-no-clone-no-rust-toolchain)**.
> This tutorial is the **from-source / contributor** path: build the binary yourself and drive the
> control plane directly, which is what you want when hacking on Flowplane itself.

---

## 1. Prerequisites

You need:

- **PostgreSQL** reachable on your machine. This tutorial uses `postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev`. On Linux / containers with a `postgres` superuser, `scripts/ensure-postgres.sh` creates the `flowplane_dev` database and sets the `postgres` password to `postgres`. On **macOS / Homebrew** there is no `postgres` role by default, so create it first (`createuser -s postgres` → `ALTER USER postgres PASSWORD 'postgres'` → `createdb -O postgres flowplane_dev`), or point `FLOWPLANE_DATABASE_URL` at your own superuser.
- **Envoy installed** — a local `envoy` binary on your `PATH`. You use it in step 6 to actually route traffic. On macOS, prefer the local binary over Docker.
- **A Rust toolchain via [rustup](https://rustup.rs).** The repo pins its toolchain in `rust-toolchain.toml`, which only takes effect when the build is driven through rustup — it selects the pinned version automatically on first `cargo` invocation. Do **not** use a distro-packaged `cargo` (e.g. `apt-get install cargo`): an older system cargo cannot read this repo's version-4 `Cargo.lock` and fails before the build starts.
- **The `flowplane` binary.** Build it from the repo (rustup picks the pinned toolchain):

  ```bash
  cargo build --bin flowplane
  ```

  Dev mode requires a binary built **with the `dev-oidc` feature**. That feature is on by default, so a plain `cargo build --bin flowplane` (or `cargo run`) includes it — both debug and local release builds (`cargo build --release`) are fine. The commands below use `./target/debug/flowplane`.

  The published release container (`Containerfile.release`) is built `--no-default-features`, so it does **not** include `dev-oidc` and rejects dev mode entirely. Dev mode is for local builds only. A separate evaluation image (`Containerfile.eval`, published to GHCR as `ghcr.io/rajeevramani/flowplane:<ver>-eval` — never `latest`) keeps `dev-oidc` on for the no-clone evaluator bundle; it is for local evaluation only and is never an operator base.

---

## 2. Start the control plane in dev mode

Run this in its own terminal. These environment variables are the minimal set that lets the server start without an external OIDC provider, with the xDS listener enabled so Envoy can connect later:

```bash
FLOWPLANE_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev \
  FLOWPLANE_DEV_MODE=true \
  FLOWPLANE_API_INSECURE=true \
  FLOWPLANE_API_ADDR=127.0.0.1:8096 \
  FLOWPLANE_XDS_ADDR=0.0.0.0:18000 \
  ./target/debug/flowplane serve
```

What each variable does:

- `FLOWPLANE_DATABASE_URL` — required; the server refuses to start without it. Migrations are applied automatically on startup.
- `FLOWPLANE_DEV_MODE=true` — enables the in-process issuer and seeds dev resources. Mutually exclusive with a configured OIDC issuer + audience pair.
- `FLOWPLANE_API_INSECURE=true` — opt in to a plaintext API listener (decision D-008). Without TLS material *and* without this flag, the server refuses to start. Acceptable only for local dev or behind a TLS-terminating proxy.
- `FLOWPLANE_API_ADDR` — where the REST API listens (defaults to `0.0.0.0:8080`; this tutorial uses `127.0.0.1:8096`).
- `FLOWPLANE_XDS_ADDR` — where the xDS gRPC listener binds (defaults to `0.0.0.0:18000`). Envoy connects here in step 6. In dev mode this listener is plaintext.

> **Dev mode is triple-gated.** It only runs when (1) the binary was built with
> the `dev-oidc` feature, (2) `FLOWPLANE_DEV_MODE=true`, and (3) — in
> release/optimized builds only — you also set
> `FLOWPLANE_DEV_MODE_ACK=yes-this-is-not-production`. The debug build above
> satisfies (1) and needs only (2).

Watch the startup logs for these signals:

- `database connected and migrations applied`
- `DEV MODE: in-process identity, seeded resources — never production`
- `dev resources seeded`
- a warning line containing `dev_token` (you will copy this in the next step)
- `API listener starting`

Dev mode seeds one organization, team, and user:

| Resource | Value     |
| -------- | --------- |
| Org      | `dev-org` |
| Team     | `default` |
| User     | `dev-user`|

---

## 3. Get a token and confirm authentication

Dev mode mints a bearer token at startup and logs it once. Find the log line that looks like:

```text
WARN ... dev_token=<long-token-string> dev bearer token (valid 1h, this boot only)
```

The token is valid for this control-plane process only and expires after one hour. If you restart the server, grab the new token.

In a second terminal, point the CLI at the running server and export the token:

```bash
export FLOWPLANE_SERVER=http://127.0.0.1:8096
export FLOWPLANE_ORG=dev-org
export FLOWPLANE_TEAM=default
export FLOWPLANE_TOKEN='<paste the full dev_token here>'
```

Confirm authentication works:

```bash
./target/debug/flowplane auth whoami
```

This calls `GET /api/v1/auth/whoami` and echoes the principal the server sees. A successful response shows your `user_id`, org membership, and grant count.

If you get `401 token validation failed`, check that:

- the token was copied in full (no missing trailing character),
- it came from the *currently running* server process, and
- `FLOWPLANE_SERVER` points at that same process.

---

## 4. Expose one HTTP upstream

First, start a trivial upstream in a third terminal so there is something to route to. It must serve the body `hello-flowplane` so you can recognize it later:

```bash
mkdir -p /tmp/fp-upstream
cd /tmp/fp-upstream
printf 'hello-flowplane\n' > index.html
python3 -m http.server 3001
```

Now use the `expose` shortcut. In one command it creates a cluster, a route config, and a listener that route to the upstream:

```bash
./target/debug/flowplane expose http://127.0.0.1:3001 \
  --name local \
  --path / \
  --port 10001 \
  --public-base-url http://127.0.0.1:10001
```

The flags map directly to the request the server receives:

- the positional argument is the **upstream** URL,
- `--name` names the exposed service (and the resources it creates),
- `--path` is the route match prefix (defaults to `/`),
- `--port` is the listener port — this is the port Envoy will listen on, and it must match the `curl` you run in step 7 (`10001` here),
- `--public-base-url` is the address clients use to reach the listener; keep it consistent with `--port` (`http://127.0.0.1:10001`). For real deployments set it to the dataplane listener address clients can actually reach, or omit it.

On success the command prints a table describing the created resources, including a `curl_url` of `http://127.0.0.1:10001/` and the `cluster`, `route_config`, and `listener` it created.

(Optional intermediate check — confirm the control plane holds the resources:)

```bash
./target/debug/flowplane cluster list
./target/debug/flowplane listener list
./target/debug/flowplane route list
```

These print a table at your terminal. Piped into another tool they switch to JSON automatically, so
`./target/debug/flowplane cluster list | jq '.data'` works without any extra flag. To script
Flowplane this way — structured output, exit codes, safe deletes — see
[Script Flowplane from a shell or agent](../how-to/script-the-cli.md).

---

## 5. Create a dataplane record

Envoy connects to the control plane as a registered dataplane. The bootstrap generator needs this record to stamp a stable Envoy `node.id`:

```bash
./target/debug/flowplane dataplane create dp-local --description "local Envoy"
```

---

## 6. Generate the Envoy bootstrap and start Envoy

Generate the dev (plaintext) Envoy bootstrap config. Note that `--out` is a **global** flag and must come *before* the `dataplane bootstrap` subcommand:

```bash
./target/debug/flowplane --out /tmp/flowplane-envoy.yaml \
  dataplane bootstrap dp-local \
  --mode dev \
  --xds-host 127.0.0.1 \
  --xds-port 18000 \
  --admin-port 9901
```

This points Envoy at the plaintext xDS listener you enabled in step 2 (`FLOWPLANE_XDS_ADDR=0.0.0.0:18000`).

Start Envoy with the generated config (in its own terminal):

```bash
envoy -c /tmp/flowplane-envoy.yaml --log-level info
```

> On Linux hosts where Docker host networking works, you can instead run the
> `envoyproxy/envoy` image with `--network host` mounting the same config. On
> macOS, prefer the local `envoy` binary as shown above.

Wait until Envoy connects to xDS and warms the listener (you will see it pull the cluster, route, and listener from the control plane).

---

## 7. Verify success: a request through the gateway

Send a request to Envoy's listener port (`10001`):

```bash
curl -i http://127.0.0.1:10001/
```

You should get a `200 OK` whose body is:

```text
hello-flowplane
```

That response came from your upstream (`:3001`) **through Envoy** (`:10001`), configured entirely by the Flowplane control plane. That is your first success.

If traffic does not flow, check, in order, that the control plane logs show Envoy connected to xDS, that the upstream still answers at `http://127.0.0.1:3001/`, and that port `10001` is not already in use.

To tear down what you created:

```bash
./target/debug/flowplane unexpose local
```

`unexpose` is destructive, so on an interactive terminal it asks for `[y/N]` confirmation before
acting. Answer `y`, or pass `--yes` to skip the prompt (required when running non-interactively).

---

## You now have a working gateway

You started Flowplane in dev mode, authenticated with a dev token, exposed an upstream, connected a local Envoy, and reached the upstream through the gateway. From here:

- **Securing the dataplane with mTLS:** [`../how-to/register-dataplane-mtls.md`](../how-to/register-dataplane-mtls.md)
- **Every CLI command and flag:** [`../reference/cli.md`](../reference/cli.md)
- **Every configuration environment variable:** [`../reference/configuration.md`](../reference/configuration.md)
