# Flowplane v2

Flowplane v2 is a ground-up Rust/PostgreSQL rewrite of the Flowplane control plane. V1 remains
reference material for product outcomes; V2 keeps the architecture cleaner: PostgreSQL is the
source of truth, Envoy is the only dataplane, xDS/SDS is the config channel, and product mutations
go through `fp-core` services.

The current focus is **S7.7 Core gateway parity before learning**. Before S8 learning resumes, the
basic gateway loop must be usable:

```text
start control plane -> start dataplane -> expose upstream -> curl through Envoy -> inspect status
```

## Current Developer Path

For the current manual dev workflow, see:

- [docs/dev-dataplane.md](docs/dev-dataplane.md)

That guide covers starting PostgreSQL, running the CP in dev mode, exporting the dev token, starting
a local Envoy dataplane, creating gateway resources, and checking stats/NACK diagnostics.

## Useful Commands

Build:

```bash
cargo build --bin flowplane
```

Run tests for the main binary:

```bash
cargo test -p flowplane
```

Run the full suite with PostgreSQL-backed tests enabled:

```bash
FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/flowplane_dev \
  cargo test --workspace --all-features
```

Run the live Envoy smoke test:

```bash
scripts/e2e-envoy.sh
```

Print the generated REST contract:

```bash
./target/debug/flowplane openapi
```

## Architecture References

- [spec/10-v2-architecture.md](spec/10-v2-architecture.md) — target architecture
- [spec/13-basics-before-learning-mindmap.md](spec/13-basics-before-learning-mindmap.md) — core gateway parity plan
- [spec/14-architecture-integrity.md](spec/14-architecture-integrity.md) — domain ownership and seam rules
