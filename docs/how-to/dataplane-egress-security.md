# Dataplane Egress Security (SSRF / DNS-Rebind Posture)

> Audience: operators, platform-engineers, security teams · Status: required reading for production

Flowplane lets tenants point gateway upstreams — clusters, AI provider `base_url`s, `expose`
targets, and learned/generated routes — at **any public or private host they choose**. That is a
product guarantee: private-VPC backends, Kubernetes Services, headless/rotating DNS, and
ephemeral upstreams all work, and Flowplane will never blanket-deny private (RFC 1918 / IPv6 ULA)
destinations or pin DNS at the control plane.

The flip side: a tenant-authored hostname becomes an Envoy `STRICT_DNS` cluster that re-resolves
at connect time. A tenant who controls that DNS record can **rebind it after creation** to a
destination the dataplane host can reach — cloud metadata services, the dataplane's own loopback,
or Flowplane's control-plane infrastructure. No control-plane code can stop a post-write rebind.

**The enforcement boundary is the dataplane node's network.** Whoever operates the dataplane host
owns this control. This page is the required posture.

## Shared responsibility

| Deployment | Who enforces | Status of the SSRF/rebind findings |
| --- | --- | --- |
| Customer-hosted dataplane (all deployments today) | **You**, at the dataplane node's network | **Mitigated only for deployments that apply this posture.** A dataplane running without it is an unsupported, insecure configuration. |
| Flowplane-hosted dataplane (no such offering exists yet) | Flowplane, fail-closed, via first-party packaging | Any future hosted offering ships enforcement **before launch**; until then no hosted mitigation is claimed. |

The control plane's write-time **egress advisory** (below) is defense-in-depth only. It rejects
hosts that *currently* resolve to a protected destination; it **cannot** stop a rebind that
happens after the write. Do not treat it as the mitigation.

## What must be unreachable from the dataplane host

Three target classes, each with a different mechanism:

| Target class | Destinations | Why SGs/NACLs are not enough |
| --- | --- | --- |
| Cloud metadata / credential endpoints | `169.254.169.254` (AWS/GCP/Azure IMDS), `169.254.170.2` (ECS task credentials), `fd00:ec2::254` (EC2 IPv6 IMDS), `168.63.129.16` (Azure wireserver), plus your platform's equivalents | Link-local traffic never traverses VPC routing — security groups and NACLs cannot see it. The control must be **host/instance-local**. |
| Dataplane loopback | `127.0.0.0/8`, `::1` | Same: loopback never leaves the host. An Envoy upstream rebound to loopback reaches the Envoy admin port and anything else bound locally. |
| Routed Flowplane infrastructure | The control-plane API and xDS addresses, PostgreSQL, and the rate-limit service (RLS) — your specific CIDRs | These *are* network-routable, so network-layer controls work — but you must enumerate your own address ranges. |

Envoy resolves upstream DNS itself and re-resolves per TTL (`STRICT_DNS`). Also cover
IPv4-embedding IPv6 forms if your network is dual-stack: 6to4 (`2002::/16`) and NAT64
(`64:ff9b::/96`) can smuggle an IPv4 destination inside an IPv6 answer.

## Required posture per platform

### AWS (EC2 / ECS)

- **Metadata:** require IMDSv2 with `--metadata-options HttpTokens=required,HttpPutResponseHopLimit=1`.
  Hop-limit 1 stops containers one network hop away, **but AWS documents container-networking
  modes where this breaks IMDS for legitimate host agents or does not isolate the container** —
  so it is not the sole control. Add a host/task-local firewall reject (iptables/nftables) for
  `169.254.169.254`, `169.254.170.2`, and `fd00:ec2::254` from the Envoy process/namespace. Use
  task roles / instance profiles with least privilege so a leaked credential has minimal reach.
- **Loopback:** host-local firewall — reject Envoy-originated connections to `127.0.0.0/8` and
  `::1` except Envoy's own admin loopback usage by `fp-agent` (which runs on the same host and is
  the only sanctioned admin consumer).
- **Routed infra:** use a **deny-capable** control. Security groups are allow-only — they cannot
  express "allow everything except these CIDRs", and a gateway dataplane normally needs broad
  egress (tenants target arbitrary hosts), so an SG with a `0.0.0.0/0` egress rule still permits
  the CP/DB/RLS destinations. Required: **NACL deny entries** for your CP/DB/RLS CIDRs on the
  dataplane subnets, or a **host firewall reject** for those CIDRs (or an egress proxy that
  enforces the deny). SG-only enforcement is acceptable **only** when the dataplane's egress can
  be a complete allowlist that never overlaps the protected ranges — rare for a gateway. The
  `deploy/aws` stack in this repo provisions the **control plane** only; its security groups do
  not govern your dataplane hosts.

### Kubernetes

- **Metadata + loopback:** host-local (node) controls as above; a NetworkPolicy cannot filter
  link-local or loopback. On managed platforms prefer the provider's metadata-concealment
  mechanism where offered.
- **Routed infra:** a NetworkPolicy with egress `ipBlock` + `except` covering your CP/DB/RLS
  CIDRs — **CNI-qualified**: behavior for Service VIPs, headless Services, and rewritten traffic
  varies by plugin. Verify with your CNI (Calico, Cilium, and Antrea enforce egress `ipBlock`;
  some cloud-default CNIs do not) and test it, don't assume it.

### Bare metal / VMs / plain containers

- Host firewall (nftables/iptables) on the dataplane host: reject the metadata set, loopback
  (from the Envoy service context), and your infra CIDRs; allow everything else so tenant
  public/private upstreams keep working.

**Acceptance check (any platform):** from the dataplane host's Envoy context —
metadata endpoints, loopback, and each infra CIDR are unreachable; an arbitrary public address,
a tenant private address, and a rotating-DNS upstream all still connect.

## The control-plane egress advisory (defense-in-depth)

On every tenant upstream write (cluster create/update, AI provider create/update, `expose`,
route-generation apply) the control plane resolves the authored host and rejects the write if
**any** DNS answer is a protected destination. Rejections return a validation error and write an
`egress_advisory.denied` audit record with the hostname and the full resolved-address set.

| Knob | Default | Meaning |
| --- | --- | --- |
| `FLOWPLANE_EGRESS_ADVISORY_ENABLED` | `true`; `false` in dev mode | Operator-only. Defaults **off** under `FLOWPLANE_DEV_MODE=true` (single-host: loopback upstreams are legitimate there); set it explicitly to force either way. Disabling logs a startup warning; tenants cannot override. |
| `FLOWPLANE_EGRESS_ADVISORY_DENIED_CIDRS` | unset | **Set this in production.** The advisory derives the database and RLS addresses from server configuration automatically, but the control plane's own API/xDS listeners usually bind `0.0.0.0` — the CP's routable CIDRs **must** be supplied here or the advisory does not cover them. |

Built-in deny set (always active when enabled): the metadata/credential endpoints listed above,
loopback, link-local, multicast/unspecified/broadcast, and 6to4/NAT64 forms whose embedded IPv4
address is protected. Private ranges are **not** denied — tenant private upstreams are
legitimate.

Residual risk, stated plainly: the advisory checks at write time only. A host that resolves
publicly at creation and is rebound afterwards passes the advisory and is stopped only by the
dataplane network posture above. If `FLOWPLANE_EGRESS_ADVISORY_DENIED_CIDRS` is unset, writes
targeting the CP's own routable addresses are not advisory-rejected either.

## Related

- [`production-readiness.md`](production-readiness.md) — overall production posture
- [`aws-secure-deployment.md`](aws-secure-deployment.md) — control-plane AWS deployment
- [`../reference/configuration.md`](../reference/configuration.md) — all configuration knobs
