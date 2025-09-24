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


