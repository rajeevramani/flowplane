## Magaya Envoy Control Plane
This project is named `mägaya rom`. The Gupapuyngu word mägaya rom, meaning still/quiet, pronounced `**Mah-gi-ya rom**`.

When you see the ocean lying still, and everything is quiet and still, that is like `**magaya rom**`.
`Rom` is the `Yolŋu` word for law or way or lore. When everything is in a state of balance according to the law, you have mägaya rom. Another way of looking at it is like the Japanese concept of Ying/Yang, with these two elements being in balance.

The Aim of the CP is initially to provide a resultful interface for Envoy. We will then try to extend this capability to support A2A and MCP protocols.

## Running the server

```
MAGAYA_XDS_PORT=18003 \
MAGAYA_CLUSTER_NAME=my_cluster \
MAGAYA_BACKEND_PORT=9090 \
MAGAYA_LISTENER_PORT=8080 \
MAGAYA_DATABASE_URL=sqlite://./data/magaya.db \
cargo run --bin magaya
```


## Working with listeners

- Create listeners via `POST /api/v1/listeners` using camelCase fields (see `scripts/smoke-listener.sh` for a ready-made example). The control plane automatically injects Envoy's router filter, so you only need to supply route/cluster information.
- Optional listener features are supported:
  * `tlsContext` populates Envoy's downstream TLS context (certificate chain, private key, CA bundle, and client-auth requirement).
  * `accessLog` maps to Envoy's file access logger (with an optional text format string).
  * `tracing` configures the HTTP connection manager tracing provider (name plus key/value options).
- Lists of listeners: `GET /api/v1/listeners`
- Delete a listener: `DELETE /api/v1/listeners/{name}`

### Smoke test

With the server running locally:

```
scripts/smoke-listener.sh
```

The script provisions a TLS-enabled cluster pointing at `httpbin.org`, registers a listener, publishes a simple route, and curls Envoy at `http://localhost:10000/status/200`.

