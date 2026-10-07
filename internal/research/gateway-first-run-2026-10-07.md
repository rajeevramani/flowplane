# Open-source gateway first runs: lessons for Flowplane

Research date: **7 October 2026 (Australia/Melbourne)**. Docs-only review; no product was installed or executed. Flowplane snapshot: `67bd6766ebe8d17dfecae6cca1322128bfa88985`, README example `3.1.4`. Research tracked in Beads `fpv2-1ks`.

## 1. Executive summary

**I: Improve the handoff around the existing evaluator before changing packaging architecture.** Flowplane already supplies the laptop path many Envoy peers lack: no clone, bundled PostgreSQL, automatic configuration/secrets, a local demo, and documented amd64/arm64 support. Its documentation scores **15/20**, versus a **10/20 gateway median** under the frozen rubric. These are evidence scores, not tested reliability rankings. [README.md:13–57][FP-readme]

The three largest gaps are **knowing when traffic is ready**, **moving from the demo to one's own API**, and **recovering/repeating the trial**. The README sends a bare curl immediately after detached startup; the tutorial says to wait for healthy services without a command proving gateway readiness. Its continuation demonstrates OpenAPI publishing and tool counts, while explicitly leaving tool invocation unbound. Cleanup is destructive `down -v`, without a nearby preserve/reset distinction. [README.md:33–49][FP-start] [evaluate-no-clone.md:24–36][FP-proof] [evaluate-no-clone.md:123–170][FP-next]

Top three moves: **(1)** replace the first curl with bounded, body-checked retries and put failure commands beside it; **(2)** make “route your own API” the next no-clone exercise, using the existing expose/unexpose surface; **(3)** document ports and separate stop, resume and reset. Borrow APISIX's complete proof/recovery sequence, Traefik/KrakenD's tangible demo target, and LiteLLM/Supabase's state-aware lifecycle. [A2] [F3] [R3] [L-script] [S4] Existing expose semantics are documented at [cli.md:393–415][FP-expose].

I: These changes could lift the score to **19/20** with the same three commands to demo traffic; reaching20 requires verified platform/resource guidance. Adding an embedded datastore would work against the PostgreSQL constitution and buys no current dependency-assembly points. [constitution.md:41–46][FP-constitution]

## Method and count conventions

**D** means stated in inspected docs/source artifacts. **I** means counted, estimated, interpreted or recommended. **U** means unresolved. Absence means “not found in the inspected entry path”, not “the product cannot do it”. All external source dates/versions appear in §6. Table cells use source reference links; counts and times are I throughout.

A command is one shell invocation; multiline requests/heredocs count once. Assignments such as `VER=...` are separately listed setup actions, not executable invocations. Authored files count separately; downloading an existing file is not authoring. UI stages group related selections/forms, so a four-stage wizard is not four clicks. The appendix identifies command sequences and UI stages; exact physical scrolls/click counts within dynamic UI are U. First gateway value is a response **through** the gateway, not startup, health, dashboard login or a tool count. Prerequisite install/account steps without a literal recipe are uncounted lower bounds.

Time estimates assume required tools/credentials exist and ordinary image download speed. They include reading/setup but are not measured and do not establish a15-minute SLA. Existing-cluster estimates exclude cluster creation: the target persona cannot take those routes as written. Benchmarks have their own value moments and supply patterns only; they never enter the13-gateway median.

## 2. Phase 1 — external comparison and frozen standard

### A. Comparison table

|Product|Leading artifact|Components|Prerequisites|Commands/files to value|DB handling|Backend|Troubleshooting|
|---|---|---|---|---|---|---|---|
|Envoy Gateway [EG-home]|Helm [EG-q]|Controller, Envoy, app [EG-q]|Kubernetes [EG-q]|I6/0 after cluster [EG-q]|I no separate DB [EG-q]|Bundled app [EG-q]|Yes, partial: wait/LB/help [EG-q]|
|kgateway [KG-home]|Helm [KG-q]|I4 workload containers +cluster [KG-demo]|Kubernetes/Helm/kubectl [KG-q]|I13/0; kind adds1 [KG-q] [KG-demo]|I no separate DB [KG-q]|httpbin pod [KG-demo]|Yes: Rosetta/debug [KG-demo] [KG-debug]|
|Contour [C-home]|YAML apply [C-q]|I7+ workload containers [C-q]|Kubernetes or kind [C-kind]|I5/0+browser; kind adds2/1 [C-q] [C-kind]|I no separate DB [C-q]|httpbin [C-q]|Yes, linked [C-q]|
|Emissary [E-home]|Helm [E-q]|Gateway+Faces; count U [E-q]|I cluster/Helm/kubectl [E-q]|I6/0+browser [E-q]|I no separate DB [E-q]|Faces chart [E-q]|No repair recipe; wait [E-q]|
|Kong [K1]|Compose [K1]|2 running+2 migrations [K2]|Docker20.10+,I Git/Compose [K3]|3 startup; proxy U [K1]|Bundled Postgres9.5 [K2]|OSS route U [K1]; separate tutorial httpbin [K4]|No repair recipe; status [K3]|
|Tyk [T1]|Compose [T2]|Gateway+Redis [T2]|Docker24+/Compose2.20+ [T2]|I5/2 adapted proxy;2health [T2] [T3]|Bundled Redis8.2 [T2]|Public Petstore [T3]|Yes: ownership fix [T2]|
|APISIX [A1]|Shell installer [A2]|Gateway+etcd [A2]|Docker20.10+/curl, host network [A2]|I5/0 incloptional admincheck [A2]|Bundled etcd [A2]|Public httpbin [A2]|Yes: ports/etcd/401 [A2]|
|KrakenD [R1]|Playground/make [R2] [R3]|12services [R4]|I Docker/Compose/Git/make [R3]|I4/0, clone inferred [R3]|File gateway; ancillary DBs [R4]|Local fakeAPI [R3]|No repair recipe; logs [R3]|
|Traefik [F1]|Compose [F3]|Proxy+whoami [F3]|Docker/socket/80/8080 [F3]|I3/2+dashboard [F3]|I no DB [F3]|Local echo [F3]|No repair recipe; output [F3]|
|Gravitee [G1]|Compose/make [G1] [G2]|7services [G3]|I Docker/Git/make/ports [G1] [G3]|3startup; proxy U [G4]|Mongo+ES bundled [G3]|Public JSONPlaceholder [G4]|Yes: Rancher workaround [G1]|
|LiteLLM [L-home]|Shell installer [L-q]|Gateway+Postgres16 [L-compose]|Docker/Compose/openssl/key [L-script]|I1/0+UI stages [L-q]|Bundled Postgres [L-compose]|Own provider/key [L-q]|Yes: ports/daemon/timeout [L-script]|
|Portkey [P-home]|npx; Docker alternate [P-install]|1container [P-compose]|Node/npm +I Python/key [P-home]|I3/1; local endpoint U [P-home]|I no DB [P-compose]|Own provider [P-home]|No repair recipe; logs [P-install]|
|Envoy AI→Agent Router [AR-home]|Native CLI; Docker alternate [AR-install]|1container; processes U [AR-run]|macOS/Linux/provider key [AR-run]|I2/0 Docker; native acquisition extra [AR-start]|I no DB [AR-install]|Own OpenAI/Ollama [AR-run]|No repair recipe; health/logs [AR-run]|
|Supabase local [S2]|CLI npm tab [S3]|Docker stack, count U [S4]|Docker,Node20+ [S4]|I3/0+Studio [S3]|Bundled Postgres [S4]|Empty service; bootstrap optional [S5]|Yes: port guidance [S4]|
|Tailscale [T-home]|SSO+native client [TS-q]|Per-device client+managed coordination [TS-linux]|SSO+2devices [TS-q]|6UI stages; traffic extra [TS-q]|I managed/hidden [TS-q]|Own devices [TS-q]|Yes, linked [TS-mac]|
|Ollama [O1]|Curl installer; DMG alternative [O2] [O4]|Server/model runner,count U [O5]|Mac14+/hardware/download [O4] [O3]|I2/0+prompt [O2] [O3]|I no external DB [O3]|Downloaded model [O3]|Yes, linked logs/GPU [O6]|
|k3s [KS-home]|Curl installer [KS-q]|Bundled service +pods [KS-q]|Linux/root/systemd oropenrc [KS-req]|I2/0,nodeReady [KS-home]|Embedded SQLite [KS-db]|No workload [KS-q]|Yes, linked requirements [KS-req]|

### B. Assumption map

D Kubernetes dominates the prominent four Envoy-peer paths; I the persona lacks the main prerequisite, regardless of short post-install counts. Envoy Gateway additionally documents experimental standalone Docker; kgateway/Contour use kind; Emissary's archived Docker recipe has unresolved placeholders. [EG-q] [EG-standalone] [KG-q] [C-kind] [E-docker]

D Docker suffices for several mainstream packaging routes, but Git/make, JSON/YAML, socket access and external hostnames recur. I terminal/Compose comfort fits; extra package managers, Kubernetes objects, deployment choice and troubleshooting literacy are not established. [K1] [R3] [F3] [T2]

D AI routes require existing provider credentials or a running local provider; I they evaluate integration more readily than anonymous discovery. LiteLLM's browser reduces SDK work; Portkey assumes Node and a Python client; Agent Router adapts familiar provider env vars. [L-q] [P-home] [AR-run]

D Benchmarks assume different intent: Supabase a project repository, Tailscale SSO/two devices, Ollama capable hardware, k3s a Linux administrator. I only some assumptions hold for a single Docker laptop; none establishes a universal time guarantee. [S3] [TS-q] [O4] [KS-q]

### C. Pattern catalogue

All explanations, costs and fit judgments here are I; implementation evidence is D in the sources.

|Pattern|Evidence/users|Why it helps|Maintenance cost|Where it does not fit|
|---|---|---|---|---|
|P1 One evaluation orchestrator|LiteLLM+Postgres [L-compose]; Supabase CLI [S4]; APISIX+etcd [A2]|Removes dependency assembly|Image health/version/platform upkeep|A full showcase can overgrow, as KrakenD's12services [R4]|
|P2 Ready target, ready request|Envoy example [EG-q]; Traefik echo [F3]; KrakenD fakeAPI [R3]|No app or account needed|Fixture image and expected-output upkeep|Remote examples still need Internet; real AI calls need credentials [L-q]|
|P3 Generated local state|LiteLLM secure retained env [L-script]; Supabase printed credentials [S4]|Avoids cryptographic/setup choices|Persistence and reset semantics|Do not silently transfer evaluation defaults into production|
|P4 Observable proof|APISIX readiness/request [A2]; kgateway200 [KG-demo]; LiteLLM Playground [L-q]|Makes success concrete|Checks/output parity|Health alone proves less, as Tyk's hello [T2]|
|P5 Repair beside failure|LiteLLM daemon/ports/logs [L-script]; APISIX diagnostics [A2]|Rescues the15minute trial|Platform/error-message drift|Generic support links require too much investigation|
|P6 Stop versus reset|Supabase preserves data [S4]; LiteLLM retains keys [L-script]; k3s uninstall [KS-stop]|Supports safe retry|Resource ownership and migration testing|Destructive reset needs clearly named scope|
|P7 Progressive own-use bridge|LiteLLM Get Code+production [L-q] [L-prod]; Supabase workflow [S5]; Agent Router same API [AR-start]|Turns demo into useful adoption|Examples and deployment parity|Different edition or config dialect breaks continuity|
|P8 Acquire then use|Ollama run downloads model [O3]|Hides acquisition choreography|Progress/retry/caching|Large download can consume entire trial|

I anti-patterns observed: cluster-first evaluation; health-only finish; feature-showcase startup; inconsistent ports/versions/placeholders; missing local endpoint in first client; cleanup deferred; deployment or enterprise choices before proof. [KG-q] [T2] [R4] [G1] [G2] [E-docker] [P-home] [K4]

## D. Frozen first-run standard

Ten equally weighted criteria, maximum20. Scores describe the prominent evaluated route, not an undocumented capability. Alternative Docker paths are recorded separately. Benchmarks illustrate patterns but are excluded from gateway medians. Unknown critical steps receive0 as documentation evidence, not product incapability. Counts exclude optional tutorials and teardown; include artifact acquisition and the first value check. Existing-cluster paths exclude unenumerated prerequisite installation and are therefore lower bounds.

|ID|Criterion|0|1|2|External exemplar / derivation|
|---|---|---|---|---|---|
|R1|Findable, coherent entry|Essential continuation unavailable or contradictory|Multiple pages/edition choices, usable with interpretation|One explicit evaluation route to value|APISIX quickstart; contrasting Kong continuity/Emissary archived placeholders|
|R2|Laptop prerequisite fit|Requires absent Kubernetes or incompatible native OS|Docker/native laptop route, architecture/resources unclear or extra runtime|Explicit macOS/Linux and architecture/resource guidance|Ollama platform docs; Docker paths in LiteLLM/APISIX|
|R3|Work to first value|More than8commands or2authored files; incomplete critical path|4–8commands or2authored files|At most3commands and1authored file|Agent Router Docker, Supabase CLI; contrasting kgateway|
|R4|Pre-value configuration and credentials|Manual generation of multiple internal secrets or substantial unexplained config|At most2authored files, or external provider/account credentials|No authored config, internal secrets automated/defaulted, no external credentials|APISIX defaults, LiteLLM generated keys, Ollama local|
|R5|Dependency assembly|User provisions unspecified external dependencies|Dependencies supplied but multiple independent starts/manual assembly|Single startup entry orchestrates all required dependencies (or no dependencies)|LiteLLM bundled Postgres; Supabase CLI; APISIX bundled etcd|
|R6|Ready demonstration workload|Bring own backend/key/account to obtain useful response|Remote public example or sample requiring separate explicit setup|Local demo workload acquired/started with evaluation stack|Envoy Gateway example; Traefik whoami; Ollama run|
|R7|Readiness and meaningful proof|Neither actionable readiness nor meaningful verification|Health/status OR explicit through-system request/expected result|Readiness check AND through-system request with observable expected success|APISIX readiness+httpbin; kgateway HTTP200; LiteLLM Playground|
|R8|Recoverable first-run failure|No actionable recovery on route (generic community link insufficient)|One concrete local remedy or directly linked focused troubleshooting|At least2concrete common failure remedies, including ports/startup|LiteLLM installer; APISIX troubleshooting|
|R9|Stop, reset and repeat|No explicit stop/cleanup instruction|Stop/uninstall supplied; persistence/reset unclear|Stop plus persistence semantics plus clean-reset guidance|Supabase stop; LiteLLM retained env/volume recovery|
|R10|Demo-to-own-use bridge|No usable continuation|Feature/deployment links or own-backend example only|Concrete own-backend/client change AND clearly scoped deployment continuation|LiteLLM Get Code+production; Supabase workflows|

Rubric frozen **2026-10-07 01:02 UTC (12:02 Melbourne)**, before first Flowplane read at01:02:36UTC. SHA-256 of the exact block above: `1a96b5dac1163cf675f8ab3a9f1f1a1c1727faf75aa9a094044010af8918d8f0`. No criteria/thresholds were revised during Phase2. External exemplar evidence: [A2] [K1] [E-docker] [O4] [L-q] [L-script] [L-compose] [AR-install] [KG-demo] [S4] [S5].

### External scoring inputs (frozen definitions, I judgments)

|Gateway|R1|R2|R3|R4|R5|R6|R7|R8|R9|R10|Total|Evidence|
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
|Envoy Gateway|2|0|1|2|2|2|1|1|1|1|13|[EG-q]|
|kgateway|1|0|0|2|2|1|2|1|1|1|11|[KG-q] [KG-demo] [KG-debug]|
|Contour|2|0|1|2|2|1|2|1|0|1|12|[C-q] [C-kind]|
|Emissary|2|0|1|2|2|1|1|0|0|1|10|[E-q]|
|Kong|0|1|0|2|2|0|1|0|0|1|7|[K1] [K2] [K3]|
|Tyk|1|1|1|1|2|1|1|1|0|1|10|[T2] [T3]|
|APISIX|2|1|1|2|2|1|2|2|1|2|16|[A2] [A3]|
|KrakenD|1|1|1|2|2|2|1|0|2|1|13|[R2] [R3] [R4]|
|Traefik|2|1|1|1|1|1|1|0|0|1|9|[F3]|
|Gravitee|0|1|0|2|2|1|1|1|1|1|10|[G1] [G2] [G3] [G4] [G5] [G6]|
|LiteLLM|2|1|2|1|2|0|2|2|2|2|16|[L-q] [L-script] [L-compose] [L-prod]|
|Portkey|1|1|0|1|2|0|1|0|0|1|7|[P-home] [P-install] [P-compose]|
|Agent Router|1|1|1|1|2|0|1|0|0|2|9|[AR-start] [AR-install] [AR-run]|

Scoring rationale: R6=1 where the sample is separately assembled,2where part of the bundled evaluation recipe; remote public targets=1, own credentialled providers=0. R7 needs both actionable system readiness and observable routed success; copied response text is not mandatory when a visible live result is specified. R9 counts an explicitly destructive stop/reset with stated volume deletion as2(KrakenD), but a stop command without state semantics as1. Kong's critical continuation could not be retrieved; its R1/R3/R6 zeros are provisional evidence scores, not proof the OSS tutorial is broken. Portkey's first local client endpoint and Gravitee's test target are unresolved/contradictory. Agent Router's native acquisition steps are incompletely enumerated; R3=1 is a conservative4–8invocation acquisition/run/request estimate, while the table's2command Docker alternative is separately identified. [K1] [P-home] [G4] [AR-install]


Sensitivity check (I): excluding the three gateways with unresolved essential request continuations (Kong, Portkey, Gravitee) raises the remaining10-gateway total median to11.5/20. Flowplane remains above it; the headline10/20 retains the full requested set. This reduces the temptation to treat fetch limitations as a product ranking.

## 3. Phase 2 — Flowplane against the frozen standard

### Persona walkthrough

D: The README's first route is **Quick Start (no clone, no Rust toolchain)**, after the short project introduction:0link transitions to the first command; exact viewport scrolling U. It asks for a container engine and a matching released bundle/image, explains native amd64/arm64 pulls, and separates contributor builds later. I: the architecture vocabulary before Quick Start is extra reading, but the prominent route is appropriate for this persona. [README.md:7–31][FP-readme]

|Stage|What the persona does/sees|Files, secrets, ports, concepts|Evidence|
|---|---|---|---|
|1 Select release|D `VER=3.1.4`; no release link supplied in this paragraph, only page name|Release tag must match image; tutorial says use after release appears|[README.md:20–31][FP-version] [evaluate-no-clone.md:13–21][FP-evalstart]|
|2 Acquire bundle|D `curl -fsSLO https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml`|One downloaded file, no authored file|[README.md:30–31][FP-version]|
|3 Start|D `FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:${VER}-eval docker compose -f compose.eval.yml up -d --no-build`|Images, Compose, evaluation versus production; no account or hand-generated secret|[README.md:33–35][FP-start] [README.md:54–57][FP-scope]|
|4 Wait and prove|D `curl http://127.0.0.1:10000/`; expected body `hello from the flowplane eval demo upstream`|Envoy gateway10000; tutorial says wait until healthy without specifying how; I an immediate request can arrive before readiness|[README.md:37–38][FP-start] [evaluate-no-clone.md:24–36][FP-proof]|
|5 Optional dashboard|D `docker compose -f compose.eval.yml exec flowplane-dashboard cat /shared/dashboard-url`, then open printed URL|Loopback8081, per-launch nonce, shared volume, read-only team view|[README.md:40–42][FP-dashboard] [evaluate-no-clone.md:40–52][FP-dashguide]|
|6 Optional auth|D `docker compose -f compose.eval.yml exec flowplane-eval sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) flowplane auth whoami'`|Token file, CLI inside container, seeded org/team; no copying token into host shell|[README.md:44–46][FP-auth] [evaluate-no-clone.md:54–76][FP-cliguide]|
|7 Continue the linked tutorial|D3 container CLI calls list cluster/listener/route;1heredoc writes/imports `/tmp/catalog-openapi.json`;3calls publish/status/MCP status|Additional7commands after whoami;1sample file; org/team, OpenAPI, version1, publish gate, generated tools|[evaluate-no-clone.md:63–144][FP-lifecycle]|
|8 Understand the limit|D catalog tool execution requires listener binding, omitted in sample; follow import guide for binding|I successful counts do not mean catalog traffic has been routed; sample has no `/items` backend|[evaluate-no-clone.md:146][FP-binding] [import-and-publish-openapi-spec.md:73–81][FP-import]|
|9 Exit|D `docker compose -f compose.eval.yml down -v`|I removes declared named-volume state; preserve/resume semantics are not taught here|[evaluate-no-clone.md:167–170][FP-teardown] [compose.eval.yml:399–405][FP-volumes]|

**I counts:** first demo response requires **3shell commands +1release assignment**, **0authored files/1download**, **0manual secret generations**. Dashboard adds1command+browser navigation. The full no-clone tutorial through tool status is **12commands +1assignment +browser**, with1authored JSON file:3start/proof,1dashboard,1whoami,3lists,1heredoc import,3publish/status. Teardown is separate. Estimated demo journey **3–8minutes**, full tutorial **10–20minutes**, highly download/read dependent; actual duration U. These counts derive from the recipes above. [README.md:28–49][FP-start] [evaluate-no-clone.md:58–144][FP-lifecycle]

D/I packaging audit: the compose file declares **10services:6long-running and4one-shot setup jobs**. Long-running: PostgreSQL, control plane, demo, dashboard, Envoy, agent. Setup: shared-init, PKI, CLI init, client-PKI verification. This is more machinery than the README's initial four-component description; I automatic orchestration hides appropriate detail, but unfamiliar successful “Exited” setup jobs deserve a sentence. [compose.eval.yml:22–74][FP-composehead] [compose.eval.yml:151–212][FP-composeinit] [compose.eval.yml:269–361][FP-composebottom]

D ports the laptop needs: **8080** API (override `FLOWPLANE_EVAL_API_PORT`), **10000** gateway (override `FLOWPLANE_EVAL_GATEWAY_PORT`), **8081** dashboard fixed. Internal connections include demo5678, xDS18000 and agent-local Envoy admin9901; none needs a host admin-port opening. The dashboard's bound/published port must match its Host/Origin check, so changing only its host mapping is not a documented fix. [compose.eval.yml:179–181][FP-apiport] [compose.eval.yml:201–204][FP-demo] [compose.eval.yml:312–317][FP-dashport] [compose.eval.yml:335–388][FP-agent]

D secrets/config are supplied: evaluation Postgres password, constant development encryption key, generated CA/server/client certificates, `/shared/dev-token`, and nonce URL. Product resources are created through the CLI; there is no manual Envoy YAML editing. I neither the number of containers nor the database is the primary user burden here. [compose.eval.yml:23–35][FP-composehead] [compose.eval.yml:112–178][FP-secrets] [compose.eval.yml:237–254][FP-initcli] [compose.eval.yml:293–323][FP-composebottom]

D version check: local `README.md`, `compose.eval.yml` and the no-clone tutorial are byte-identical to those at locally available tag `v3.1.4` (commit `ada18f1`). U: web retrieval of release page/tagged raw bundle returned cache misses; published asset availability/image digest/platform manifests were not verified. The README's multi-arch statement is credited as documented, not runtime proof. [README.md:20–25][FP-version]

### Rubric scorecard

I: scores apply the unchanged Phase1 definitions. Best and median are per criterion, calculated from the13gateway rows in the following matrix; they are not the score of a composite real product. Outside benchmarks are shown only as exemplars. U-critical-path zeros penalize available documentation evidence and are explicitly provisional.

|Criterion|Flowplane|Gateway best|Gateway median|Flowplane evidence / judgment|
|---|---:|---:|---:|---|
|R1 Coherent entry|2|2|1|One no-clone route, contributor path explicitly separate. [README.md:13–59][FP-readme]|
|R2 Laptop fit|1|1|1|Native amd64/arm64 documented; no quantified resource floor or Compose version in first route. [README.md:15–25][FP-version] [evaluate-no-clone.md:7][FP-prereq]|
|R3 Work to value|2|2|1|3invocations,0authored files. [README.md:28–38][FP-start]|
|R4 Config/credentials|2|2|2|No manual config/secret creation; generated local auth/PKI. [compose.eval.yml:158–178][FP-secrets] [compose.eval.yml:237–254][FP-initcli]|
|R5 Dependency assembly|2|2|2|One Compose startup; automated DB/setup dependencies. [compose.eval.yml:185–196][FP-cphealth] [compose.eval.yml:256–288][FP-composebottom]|
|R6 Demo workload|2|2|1|Local echo backend; CLI config generated in stack. [compose.eval.yml:201–204][FP-demo] [compose.eval.yml:253–254][FP-initcli]|
|R7 Readiness+proof|1|2|1|Expected body clear; internal CP/DB readiness exists, but no gateway healthcheck/user-facing bounded readiness command before curl. [evaluate-no-clone.md:24–36][FP-proof] [compose.eval.yml:335–349][FP-envoy]|
|R8 Recovery|1|2|1|Dashboard/agent log recipes; first traffic/port/pull failure decision tree absent in evaluated path. [evaluate-no-clone.md:46–52][FP-dashguide] [README.md:28–49][FP-start]|
|R9 Lifecycle|1|2|0|Destructive reset supplied; stop-preserving-state versus fresh reset unexplained. [evaluate-no-clone.md:167–170][FP-teardown] [compose.eval.yml:399–405][FP-volumes]|
|R10 Own-use bridge|1|2|1|Deployment continuation exists; no-clone next exercise imports an unbound sample rather than exposing user's backend. [evaluate-no-clone.md:146–165][FP-next] [README.md:99–114][FP-contributor]|
|Total|**15/20**|**16/20 best actual total**|**10/20 actual total median**|Best observed totals APISIX/LiteLLM; these are docs scores, not a performance ranking.|

### Flowplane assumption audit

D “only a container engine” in the README becomes Compose+curl in the tutorial. I the persona probably has curl but Docker installation alone does not explicitly establish Compose version or available memory/disk. Native ARM image support is a positive documented distinction. [README.md:15–25][FP-version] [evaluate-no-clone.md:7][FP-prereq]

D “Wait until the services are healthy” assumes the reader can infer what health means and inspect it. I the persona knows Compose, but not Envoy warming; this is avoidable trial uncertainty, unlike the peers' hard Kubernetes prerequisite. [evaluate-no-clone.md:24][FP-proof] [EG-q] [KG-q]

D “publish it, and verify that API tools became visible” assumes curiosity about API lifecycle/MCP; “Tool execution requires a listener route binding” introduces a second success boundary. I neither OpenAPI nor MCP knowledge nor interest is part of the persona. A read-only dashboard is useful feedback but cannot replace the missing own-backend task. [evaluate-no-clone.md:5][FP-prereq] [evaluate-no-clone.md:139–146][FP-binding] [README.md:152][FP-dashboardfeature]

D linked production guides assume an operator, OIDC, certificates, PostgreSQL and a separate dataplane host; the onboarding guide assumes a platform-team handoff. I these are appropriate **after** laptop evaluation, but do not help a lone developer reuse the eval gateway for an app. The contributor tutorial explicitly redirects evaluators, so do not count its Rust/Postgres/Envoy prerequisites against the leading route. [evaluate-platform.md:9–20][FP-platform] [onboard-api-team.md:5–18][FP-onboard] [getting-started.md:11–15][FP-getting]

### Gaps tied to patterns

|Gap|Frozen criterion|Closing patterns|Scope of evidence|
|---|---|---|---|
|G1 “Started” versus “routing now”|R7|P4 observable proof, P5 nearby failure help|Bare first curl and unspecified wait [README.md:28][FP-start] [evaluate-no-clone.md:24][FP-proof]|
|G2 Demo-to-my-API discontinuity|R10|P7 progressive bridge|Unbound catalog continuation; expose exists elsewhere [evaluate-no-clone.md:146][FP-next] [cli.md:393][FP-expose]|
|G3 Trial failure/re-run ambiguity|R8,R9|P5 repair, P6 stop/reset|Logs limited to dashboard/agent; only down-v shown [evaluate-no-clone.md:40][FP-dashguide] [evaluate-no-clone.md:167][FP-teardown]|
|G4 Unquantified laptop envelope|R2|P1 compatible bundle, explicit prerequisites|Architecture stated, resource/Compose floor not stated [README.md:20][FP-version] [evaluate-no-clone.md:5][FP-prereq]|

## 4. Phase 3 — ranked incremental recommendations

All recommendations, effort estimates and score/step projections are **I**, conditional on subsequent validation. They are proposals, not changes made by this research. Scores are not additive when two items close the same criterion. Preserve evaluation-only scope and existing product mutation/security boundaries. [README.md:54–57][FP-scope] [constitution.md:41–51][FP-constitution]

### Now — docs and defaults, days

**1. Make the very first request a bounded routing check, with failure instructions adjacent.** G1/R7, P4/P5 from APISIX and LiteLLM. Replace bare curl with retry logic that recognizes the exact demo body, times out and points to `compose ps -a` plus logs for PKI/init/CP/Envoy; keep the dashboard optional. [A2] [L-script] Current expected body/readiness split: [evaluate-no-clone.md:24][FP-proof] [compose.eval.yml:335][FP-envoy]. **Score R7:1→2, total15→16; commands3→3.** Effort half–1day docs; maintenance expected body/service names. No product runtime/default/topology change. Do not use direct Envoy admin as the normal check: constitution says curated CP diagnostics own that path. [constitution.md:51][FP-constitution]

**2. Put “Expose your API using the running evaluator” immediately after demo success.** G2/R10, P7 from LiteLLM's generated app call and Agent Router's laptop bridge. [L-q] [AR-start] Reuse existing CLI, seeded scope and auth; explain upstream address **as seen by Envoy's container**, then show expose, gateway request, and unexpose. Document distinct Mac/Linux host or container-network recipes, retaining10000 as the already-published listener; avoid silently adding10001 or assuming host127.0.0.1 is reachable from a container. Existing exposes create a listener; the bundle publishes only10000. [cli.md:393][FP-expose] [compose.eval.yml:237][FP-initcli] [compose.eval.yml:335][FP-envoy] **R10:1→2; demo commands unchanged; target own-API extension3–5commands plus backend availability**, rather than the current7-command lifecycle detour after whoami that still does not route the sample API. [evaluate-no-clone.md:63][FP-lifecycle] [evaluate-no-clone.md:139][FP-binding] Effort1–2days docs after recipe validation. Risks: demo resource-name/port collisions, Linux host reachability, destructive unexpose. This overlaps existing Bead `fpv2-drz`; use that work rather than a duplicate implementation task. Any added host-gateway mapping is a **packaging/topology change requiring constitution/design review**, not assumed acceptable by this report.

**3. Add a compact “if it fails / stop / resume / reset” box.** G3/R8,R9, P5/P6 from APISIX, Supabase and LiteLLM. [A2] [S4] [L-script] List8080/8081/10000 before startup; use existing API/gateway port overrides; explain dashboard remapping limitation. Show logs for image/startup/setup failure, fresh nonce retrieval, `down` for retained state, explicit same-image `up` for resume, and `down -v` for deleting evaluation data/certs. Existing overrides, fixed dashboard port and volumes: [compose.eval.yml:179][FP-apiport] [compose.eval.yml:312][FP-dashport] [compose.eval.yml:335][FP-envoy] [compose.eval.yml:399][FP-volumes]. **R8:1→2 andR9:1→2; zero happy-path commands added; combined with1–2 target19/20.** Effort1day. Maintenance: don't advertise stateful resume as proven until tested; dev token and cert expiry must be described. PKI already fails with reset instructions on expired/partial material. [compose.eval.yml:78–110][FP-pkifail]

**4. Tighten release/prerequisite prose, without changing the lead artifact.** G4/R2; P1 from compatible bundled stacks. Link the exact release, label Mac/Linux Docker context, give the validated Compose/resource floor and download expectations, and explain setup containers may successfully exit. Source facts available now: versions/arch [README.md:20][FP-version], service jobs [compose.eval.yml:22][FP-composehead] [compose.eval.yml:269][FP-composebottom]. **R2:1→2 only after evidence exists; commands3→3.** Effort halfday docs, plus later platform qualification. Risk invented limits; report provides no measured memory/time claim. Do not replace explicit release selection with floating latest. No runtime/topology change.

### Next — packaging and tooling, weeks

**5. Ship a small, release-pinned evaluation launcher and success receipt.** G1–G4; P1/P3/P4/P6 from LiteLLM's script and Supabase's printed URLs. [L-script] [S4] It should check Docker/Compose/ports, fetch matching artifacts, preserve credentials, start the existing bundle, prove the expected response, print gateway/dashboard URLs and exact stop/reset commands. **Three demo invocations→one; R3 is already2, so no extra R3 score; other gains only for gaps not already closed by1–4.** Effort1–2weeks including Mac/ARM/Linux failure/redo cases. Costs: shell/platform/release compatibility, safe resource ownership, terminal output hygiene. **New installer/tooling and packaging defaults require constitution check and a design decision before build.** It must call supported CLI/REST mutations, never seed PostgreSQL directly or bypass auth; local orchestration may not become a production contract. [constitution.md:43–46,69–72][FP-constitution] Avoid an installer until the three-command docs flow is coherent.

**6. Make evaluator reproducibility and first-run docs a release artifact contract.** P1/P4; borrow matching manifests from Envoy Gateway and explicit versions from Tyk. [EG-q] [T2] Record all evaluator image digests/architectures, checksums and actual command/output/stop-reset qualification for MacARM and Linux. The current bundle still floats PostgreSQL16 and Envoy1.37-latest while pinning echo1.0.0. [compose.eval.yml:22][FP-composehead] [compose.eval.yml:201][FP-demo] [compose.eval.yml:335][FP-envoy] **No immediate new rubric points**; protects1–5 and lowers failed attempts, not happy-path count. Effort1–2weeks initially, modest per release. **Changing image defaults is a packaging/defaults change: design/constitution review and upgrade verification needed.** Docs-only research did not run those qualification checks.

### Later — only after evidence supports more runtime change

**7. Consider native evaluation supervision only if Docker itself becomes the measured bottleneck.** P8 acquire/use from Ollama; packaging bundling from k3s is inspiration, not permission to copy its datastore. [O3] [KS-db] **Potential3commands→1; likely0extra rubric points once1–5 are delivered.** Effort several weeks+, substantial process/download/port/cert/lifecycle upkeep. **Architecture/runtime/deployment-topology decision required.** PostgreSQL authority, Envoy-only traffic, separate CP/dataplane units, no orchestrator dependence and supported mutations remain constraints. [constitution.md:41–46][FP-constitution] An embedded-SQLite replacement contradicts the current PostgreSQL invariant; merging the traffic path into the control plane contradicts Envoy-only/separate-unit invariants. Neither is an incremental recommendation. Reducing container count without reducing user work is not evidence of better DX.

### Minimum viable first-run to ship next

**Proposed docs-only flow, not executed.** Keep the existing release bundle and all security boundaries. State prerequisites: Docker with Compose, curl and a POSIX shell;8080/8081/10000 free. No clone, account, provider key, authored config or Envoy knowledge. Versions/ports/body come from [README.md:20][FP-version] [compose.eval.yml:179][FP-apiport] [compose.eval.yml:312][FP-dashport] [compose.eval.yml:335][FP-envoy] [compose.eval.yml:201][FP-demo]. The retry wrapper below is proposed prose/tooling, not existing Flowplane output. Fetch/start/prove remain3invocations; the shell loop checks **routed body**, not merely HTTP200, and fails after bounded retries.

```sh
curl -fsSLo compose.eval.yml \
  https://raw.githubusercontent.com/rajeevramani/flowplane/v3.1.4/compose.eval.yml

FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:3.1.4-eval \
  docker compose -f compose.eval.yml up -d --no-build

sh -c '
  echo "Waiting for Flowplane to route traffic..."
  expected="hello from the flowplane eval demo upstream"
  n=0
  while [ "$n" -lt 60 ]; do
    body=$(curl -fs --max-time 3 http://127.0.0.1:10000/) &&
      [ "$body" = "$expected" ] && { printf "%s\n" "$body"; exit 0; }
    n=$((n + 1))
    sleep 1
  done
  echo "Gateway check failed. Inspect: docker compose -f compose.eval.yml ps -a" >&2
  echo "Then: docker compose -f compose.eval.yml logs pki init pki-client-verify flowplane-eval envoy" >&2
  exit 1
'
```

Expected stable output from the final command:

```text
Waiting for Flowplane to route traffic...
hello from the flowplane eval demo upstream
```

Compose also prints environment-dependent pull/start progress; exact lines cannot be promised without execution. Worst-case loop can take roughly4minutes (60 ×3second request limit plus1second delay), independent of image-download time. The proposed text should call that bound out, not advertise instant startup.

Optional dashboard, using the already-documented discovery path [README.md:40][FP-dashboard]:

```sh
docker compose -f compose.eval.yml exec flowplane-dashboard cat /shared/dashboard-url
```

Expected shape, with a fresh generated nonce:

```text
http://127.0.0.1:8081/<per-launch-nonce>/
```

Show the next link as **“Route your own API with this running stack”**, then offer optional OpenAPI/AI/MCP tutorials. This is the new doc proposed by recommendation2, not an existing callable wizard. [evaluate-no-clone.md:146][FP-next] [cli.md:393][FP-expose]

Proposed lifecycle commands, to be validated before promising retained-state resume:

```sh
# Stop and keep evaluation state
docker compose -f compose.eval.yml down

# Resume with the same release image
FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:3.1.4-eval \
  docker compose -f compose.eval.yml up -d --no-build

# Reset: deletes evaluation database, shared state and certificates
docker compose -f compose.eval.yml down -v
```

Only the final reset command is currently in the quickstart. State ownership comes from named volumes; PKI/init guards can reject invalid preserved state. [evaluate-no-clone.md:167][FP-teardown] [compose.eval.yml:399][FP-volumes] [compose.eval.yml:78][FP-pkifail] No install or command in this proposed transcript was run for the report.

## 5. Appendix — full product captures

Every numbered field follows the requested template. D facts use cited sources; I counts/times/audits are estimates. D versions/access dates are in §6. Precise physical scroll counts are U throughout. Command sequences list invocation purposes; they do not claim runtime verification. UI stage totals are not literal click totals. Alternative paths do not silently replace the prominent path's scores.

### Envoy Gateway

1. **Entry:** D homepage→Get Started,1link; prerequisites before command. [EG-home]
2. **Artifacts:** D Helm leads; experimental standalone Docker also offered. [EG-q] [EG-standalone]
3. **Prerequisites:** D cluster; I Helm/kubectl/curl/privileges; arch unspecified. [EG-q]
4. **Parts/DB:** D controller, Envoy, example app; I no separately provisioned DB. [EG-q]
5. **Config/secrets:** D provided manifest; I no authored file/key generation. [EG-q]
6. **Value:** I6commands: install,wait,apply,service lookup,port-forward,curl;5–10minutes after cluster. [EG-q]
7. **Backend:** D packaged example app. [EG-q]
8. **Surface:** D CLI→HTTP,localhost8888 via forwarding. [EG-q]
9. **Verify/help:** D wait/request, LB hostname caveat; I port/pull/arch repairs absent. [EG-q]
10. **Exit/re-run:** D delete manifest+Helm uninstall; I background-forward cleanup unstated. [EG-q]
11. **Next:** D routing/security task links. [EG-q]
12. **Assumptions:** D “A Kubernetes cluster.” I fails persona; assumes operator reading. [EG-q]
13. **Moves/friction:** I demo reduces decisions; cluster interrupts evaluation. [EG-q]

D/I Docker alternative: experimental/non-production,8commands (mkdir,chmod,network,certgen,run,copy example,Python backend,curl),1authored YAML+repo example,2persistent containers including embedded Envoy process. Expected200;10–20minutes I. Permissions and certificate choreography remain; repository access implied by copy command. No teardown observed. [EG-standalone]

### kgateway (formerly Gloo)

1. **Entry:** D home→Envoy docs→Get started→sample app,3links. [KG-home] [KG-index]
2. **Artifacts:** D Gateway API apply,CRD/controller Helm2.4.3. [KG-q]
3. **Prerequisites:** D Kubernetes,kubectl,Helm; kind laptop option. [KG-q]
4. **Parts/DB:** I4workload containers plus cluster; no separate DB. [KG-q] [KG-demo]
5. **Config/secrets:** I2pasted manifests,0authored files/keys. [KG-demo]
6. **Value:** I13commands:4install/status+9sample actions; kind adds1;10–20minutes after tools. [KG-q] [KG-demo]
7. **Backend:** D httpbin pod with2containers. [KG-demo]
8. **Surface:** D kubectl/Helm→forward8080→curl. [KG-demo]
9. **Verify/help:** D status,HTTP200,Rosetta remedy,focused debug. [KG-demo] [KG-debug]
10. **Exit/re-run:** D3demo deletes/controller uninstall; I CRD cleanup incomplete. [KG-demo] [KG-q]
11. **Next:** D routing/security/resilience tasks. [KG-demo]
12. **Assumptions:** D “a Kubernetes cluster, `kubectl`, and `helm` already set up.” I persona fails. [KG-q]
13. **Moves/friction:** I clear flow; install/value split,domain vocabulary. No Docker-only path found in inspected navigation. [KG-index] [KG-demo]

### Contour

1. **Entry:** D1homepage Get Started link; environment section first. [C-home]
2. **Artifacts:** D YAML apply leads; Helm/provisioner alternatives. [C-q]
3. **Prerequisites:** D cluster/LB or kind; laptop Docker+kind. [C-kind]
4. **Parts/DB:** I7+workload containers; no separate DB. [C-q]
5. **Config/secrets:** I no keys; laptop1authored kind YAML. [C-kind]
6. **Value:** I5commands+browser (apply/status twice,forward); laptop adds2;10–25minutes excluding tools. [C-q] [C-kind]
7. **Backend:** D httpbin,3pods. [C-q]
8. **Surface:** D CLI→local.projectcontour.io:8888 browser. [C-q]
9. **Verify/help:** D ready counts/page,linked troubleshooting; I no inline port/arch repair. [C-q]
10. **Exit/re-run:** D kind delete cluster; I main quickstart teardown absent. [C-kind] [C-q]
11. **Next:** D HTTPProxy/GatewayAPI/deployment links. [C-q]
12. **Assumptions:** D “No additional configuration is required.” I presumes cluster/operator vocabulary. [C-q]
13. **Moves/friction:** I automatic config good; several install choices. No Docker-only path found. [C-q] [C-kind]

D kind2nodes exposes80/443; guide and linked manifest describe1.33. [C-kind] [C-yaml]

### Emissary-ingress

1. **Entry:** D1homepage Quickstart link; migration warning. [E-home]
2. **Artifacts:** D Helm4.1.0; Faces2.0.0. [E-q]
3. **Prerequisites:** I cluster/Helm/kubectl/free8080; arch unstated. [E-q]
4. **Parts/DB:** D gateway+Faces; I DB provision absent; total count U. [E-q]
5. **Config/secrets:** D2pasted resource heredocs; I0authored files/generated keys. [E-q]
6. **Value:** I6commands+browser:2gateway Helm,Faces,2applies,forward;10–20minutes after cluster. [E-q]
7. **Backend:** D Faces application. [E-q]
8. **Surface:** D CLI→localhost8080/faces/. [E-q]
9. **Verify/help:** D Helm waits; I expected page/failure remedies absent. [E-q]
10. **Exit/re-run:** I teardown absent in selected quickstart. [E-q]
11. **Next:** D Mapping/Host/production links. [E-q]
12. **Assumptions:** D “after you configure a Listener!” I cluster/provider literacy. [E-q]
13. **Moves/friction:** I visual target good; ordering/provider choices demand interpretation. [E-q]

D archived2.5Docker page uses demo/8080/admin:admin, but retains literal version/product placeholders and conflicting remote/bundled quote descriptions. I not evidence of a viable current4.1 Docker path. [E-docker]

### Kong

1. **Entry:** D README commands,0links to startup; further OSS tutorial U(fetch failed). [K1]
2. **Artifacts:** D clone Docker repo,cd compose,profile database up. [K1]
3. **Prerequisites:** D Docker20.10+; I Git/legacyCompose,8000/8001/8002 plus TLS ports. [K3] [K2]
4. **Parts/DB:** D2running+2migration services; Postgres9.5 bundled. [K2]
5. **Config/secrets:** D mounted password/config files; I no manual generation in route. [K2]
6. **Value:** I3startup commands; routed OSS count/time U. Separate developer guide requires credentials. [K1] [K4]
7. **Backend:** D httpbin only verified in separate Enterprise guide. [K4]
8. **Surface:** D terminal,AdminAPI8001,Manager8002. [K1]
9. **Verify/help:** D healthy status/issue link; I concrete port/arch repair absent. [K3]
10. **Exit/re-run:** I OSS guide exit absent; separate installer has destroy mode. [K3] [K4]
11. **Next:** D datastore/declarative configuration; Enterprise guide's production bridge is separate. [K3] [K4]
12. **Assumptions:** D “test drive Kong”, “under 5 minutes”. I terminal/Git/deployment-choice tolerance. [K1]
13. **Moves/friction:** I bundled DB useful; old DB/floating image and edition continuity uncertain. [K2] [K4]

U legacy tutorial retrieval failure is not a confirmed broken browser link. D separate guide describes3.4minimum/3.16.0.0example,decK1.66.1,Enterprise license or Konnect PAT. [K4]

### Tyk

1. **Entry:** D README→OSS quickstart,1link; README summarizes startup. [T1]
2. **Artifacts:** D Compose; author tyk.conf and compose YAML. [T2]
3. **Prerequisites:** D Docker24+/Compose2.20+; Linux ownership setup; I8080/6379,arch U. [T2]
4. **Parts/DB:** D gateway+Redis,2containers,versions5.15.0/8.2.0. [T2]
5. **Config/secrets:** D copy secret placeholder; I no key generator. [T2]
6. **Value:** I2commands/2files macOS to health; Linux adds2. Proxy requires generic-guide adaptation:~5commands,5–12minutes. [T2] [T3]
7. **Backend:** D public Petstore. [T3]
8. **Surface:** D GatewayAPI; next guide also includes unavailable GUI alternative. [T3]
9. **Verify/help:** D hello healthJSON,ownership remedy; I port/pull/arch help absent. [T2]
10. **Exit/re-run:** I no selected quickstart teardown. [T2]
11. **Next:** D OAS import,hot reload; I placeholders require interpretation. [T3]
12. **Assumptions:** D “under 2 minutes”. I author JSON/YAML; health treated as installation success. [T1] [T2]
13. **Moves/friction:** I explicit versions/platform ownership good; actual request recipe not turnkey. [T2] [T3]

I adapted request sequence: create OAS API,reload,call `/petstore/`; count is not a verified literal copy/paste chain. [T3]

### Apache APISIX

1. **Entry:** D home→Get Started,1link. [A1]
2. **Artifacts:** D shell installer leads; packaging alternatives follow. [A2]
3. **Prerequisites:** D Docker20.10+/curl,host network; I Mac qualification U. [A2]
4. **Parts/DB:** D2containers,etcd bundled. [A2]
5. **Config/secrets:** D eval Admin auth disabled,production warning;0files/keys I. [A2]
6. **Value:** I5commands:install,health,optional admincheck,routePUT,curl;4without optional check;3–8minutes. [A2]
7. **Backend:** D public httpbin. [A2]
8. **Surface:** D terminal/AdminAPI;Dashboard9180/ui optional. [A2]
9. **Verify/help:** D readiness,httpbin JSON,port/etcd/401 fixes. [A2]
10. **Exit/re-run:** D remove gateway+etcd,installer retry recovery. [A2]
11. **Next:** D routes/upstreams/plugins guide. [A3]
12. **Assumptions:** D “configure your first API route”. I JSON copying/network compatibility. [A2]
13. **Moves/friction:** I complete loop; external target and host-network fit unresolved. [A2]

U installer source retrieval failed; no claim of script/doc parity. D displayed server version3.16.0. [A2]

### KrakenD

1. **Entry:** D lower homepage start→install→Playground,2links. [R1] [R2]
2. **Artifacts:** D binary/Docker/Brew; Playground evaluator. [R2]
3. **Prerequisites:** I Docker/Compose/Git/make,many ports,arch U. [R3] [R4]
4. **Parts/DB:** D12services; file gateway plus observability/auth/data-store companions. [R4]
5. **Config/secrets:** D preset configs/users; I0edits/key generation. [R3]
6. **Value:** I clone(inferred),cd,make start,curl `/public`:4commands,5–15+minutes. [R3]
7. **Backend:** D local fakeAPI8000. [R3]
8. **Surface:** D browser/curl/Postman,optional SPA. [R3]
9. **Verify/help:** D make logs/support; I expected body/port fixes absent. [R3]
10. **Exit/re-run:** D make stop removes volumes. [R3]
11. **Next:** D commented config,Designer,advanced examples. [R3]
12. **Assumptions:** D “without configuring it”; “a mock API”. I showcase footprint tolerance. [R2] [R4]
13. **Moves/friction:** I fixture good;12services and install page's2019date reduce confidence. [R4] [R2]

D service list: gateway,fakeAPI,Grafana,InfluxDB,Jaeger,Elasticsearch,Kibana,Logstash,RabbitMQ,web,JWTrevoker,Keycloak. [R4]

### Traefik

1. **Entry:** D homepage→overview→Docker,2links. [F1] [F2]
2. **Artifacts:** D Compose leads; DockerCLI alternative. [F3]
3. **Prerequisites:** D Docker,80/8080,socket; I YAML,arch U. [F3]
4. **Parts/DB:** D proxy+whoami; I no DB. [F3]
5. **Config/secrets:** D2files,labels,insecure development dashboard. [F3]
6. **Value:** I3commands (up,up,curl),2files,dashboard=6actions;3–8minutes. [F3]
7. **Backend:** D localwhoami. [F3]
8. **Surface:** D terminal/YAML/dashboard. [F3]
9. **Verify/help:** D echoed headers/router view; I failure recipes absent. [F3]
10. **Exit/re-run:** I quickstart cleanup absent. [F3]
11. **Next:** D TLS/middleware/metrics/provider links. [F3]
12. **Assumptions:** D “Choose your preferred deployment method”. I YAML/socket literacy fits partly. [F2]
13. **Moves/friction:** I tangible echo; hostname/network/legacyCompose details demand interpretation. [F3]

D image examplev3.7. U separate-compose networking was not executed; no failure asserted. [F3]

### Gravitee

1. **Entry:** D README3commands; linked localDocker4.12docs. [G1] [G2]
2. **Artifacts:** D clone,cd docker,make mongodb; docs download Compose instead. [G1] [G2]
3. **Prerequisites:** I Docker/Git/make/freeports/resources; arch U; EElicense optional D. [G2] [G3]
4. **Parts/DB:** D7services,including Mongo/ES/Mailhog and2UIs. [G3]
5. **Config/secrets:** D admin/admin; optionallicense; I0CEsecret generation. [G1] [G2]
6. **Value:** I3commands+open/login=5dashboard actions;≥24attempted proxy actions,8–20minutes; proxy U. [G1] [G4]
7. **Backend:** D public JSONPlaceholder. [G4]
8. **Surface:** D UI wizard,Save&Deploy,keyless default. [G4]
9. **Verify/help:** D login,Rancher license-dir fix; proxy test contradicts target. [G1] [G4]
10. **Exit/re-run:** D make down/stop/start; README alternate starts down-v. [G5] [G1]
11. **Next:** D security/policy/publish/documentation. [G6]
12. **Assumptions:** D “only have a few minutes”. I full-suite/wizard tolerance. [G1]
13. **Moves/friction:** I DB/login easy; README4100/docs8085,nightly/latest mismatch. [G1] [G2] [G3] [G5]

D first-API test prose names gateway/context, but example uses backend origin `jsonplaceholder.typicode.com/myfirstapi` and expects empty response. I cannot verify a proxied request from that literal chain. [G4]

### LiteLLM proxy

1. **Entry:** D README→proxy index→Quickstart,2links I. [L-home] [L-index]
2. **Artifacts:** D shell installer; manual Compose/DB-less alternatives. [L-q]
3. **Prerequisites:** D Docker/Composev2/daemon/openssl; providerkey for request. [L-script] [L-q]
4. **Parts/DB:** D gateway+Postgres16,health-gated persistent volume. [L-compose]
5. **Config/secrets:** D generated retained master/salt/DB keys,mode600env;localhost bind. [L-script]
6. **Value:** I1command,0files;4UI stages startup/login/model/Playground;5–10minutes with key. [L-q]
7. **Backend:** I own provider,key required; no bundled mock here. [L-q]
8. **Surface:** D browser test/Get Code,then virtual keys. [L-q]
9. **Verify/help:** D readiness timeout/logs,free4000–4099,Docker errors. [L-script]
10. **Exit/re-run:** D down printed,env reused,orphan-volume restore/reset; data persists. [L-script] [L-compose]
11. **Next:** D client code and production deployment. [L-q] [L-prod]
12. **Assumptions:** D “paste your provider API key”. I provider access not guaranteed. [L-q]
13. **Moves/friction:** I excellent setup recovery; key lookup/provider/mutable tag still friction. [L-script] [L-compose]

D UI stages contain multiple actions: open/login,navigate Models/Add,select provider/model,pastekey,TestConnect/Add,open Playground/send. They are not4clicks. D quickstart mutable main-stable; stable release1.104.0shown does not prove tag equality. [L-q] [L-release]

### Portkey gateway

1. **Entry:** D in-README quickstart,0links to npx. [P-home]
2. **Artifacts:** D npx leads; Docker--rm8787 alternative; cloud recommended elsewhere. [P-install]
3. **Prerequisites:** D Node/npm,key; I Python/pip/free8787,arch U. [P-home]
4. **Parts/DB:** D1compose web service; I DB-less path. [P-compose]
5. **Config/secrets:** D provider Authorization; I1snippet substitution,0gateway-key generation. [P-home]
6. **Value:** I start/installSDK/run snippet=3commands,1edit,3–8minutes; endpoint U. [P-home]
7. **Backend:** I own provider; no mock found. [P-home]
8. **Surface:** D client code,console/public logs. [P-home]
9. **Verify/help:** I no readiness/expectedpayload/repair recipe in local deployment. [P-install]
10. **Exit/re-run:** D Docker--rm; I explicitstop/reset absent,compose no DBvolume. [P-install] [P-compose]
11. **Next:** D several deployment modes/enterprise routes. [P-install]
12. **Assumptions:** D “needs Node.js and npm”. I AI integrator/key access. [P-home]
13. **Moves/friction:** I simple packaging; first SDK snippet omits explicit localhost base URL. [P-home] [P-compose]

D README2.0pre-release banner,stable1.15.2shown;latestimage resolution U. [P-home] [P-release]

### Envoy AI Gateway → Agent Router

1. **Entry:** D original homepage redirects; GetStarted1link,install2nd. [AR-oldhome] [AR-home]
2. **Artifacts:** D native aigw CLI leads; release binary/Docker alternatives. [AR-install]
3. **Prerequisites:** D Linux/macOS/providerkey; I free1975/internet,ARM U. [AR-run]
4. **Parts/DB:** D1Dockercontainer; I Envoy+processor,process count U,no DB setup. [AR-install] [AR-run]
5. **Config/secrets:** D auto-config from OpenAI envs;0authored files I. [AR-run]
6. **Value:** I Docker start+curl=2commands,3–7minutes; native acquisition extra/unquantified. [AR-start] [AR-install]
7. **Backend:** D own OpenAI or Ollama. [AR-run]
8. **Surface:** D CLI/OpenAI-compatible HTTP1975/v1. [AR-start]
9. **Verify/help:** D admin1064health/logs; I wait/expectedbody/commonfailure recipes absent. [AR-run]
10. **Exit/re-run:** D Docker--rm,native state directories; I stop/reset narrative absent. [AR-install]
11. **Next:** D same configuration API→Kubernetes; customconfig example. [AR-start]
12. **Assumptions:** D “no Kubernetes, no Docker required”; “The CLI is experimental”. I provider/experimental tolerance. [AR-start] [AR-cli]
13. **Moves/friction:** I familiar envs; install1.1/run1.2,templatedURL/clone-directory drift. [AR-install] [AR-run]

D releases1.1.0stable/1.2.0rc1; old Kubernetes-first indexed basic-usage page is not the current homepage path. [AR-release] [AR-oldbasic]

### Supabase local development (pattern benchmark)

1. **Entry:** D homepage cloudsignup; README locallink→CLI. [S1] [S2]
2. **Artifacts:** D npm-first CLI,other package tabs,Docker; experimentalnative alternative. [S3]
3. **Prerequisites:** D DockerAPI runtime,Node20+ for npm; I ports/internet. [S4]
4. **Parts/DB:** D Postgres/Auth/Storage plus helpers; count U. [S4]
5. **Config/secrets:** D generated config/printed URLs+keys,no local login. [S4]
6. **Value:** I npm install,init,start,openStudio54323=3commands+browser,0edits,5–15minutes. [S3]
7. **Backend:** D ready services,own schema; optional bootstrap populated template. [S5]
8. **Surface:** D CLI→Studio. [S4]
9. **Verify/help:** D URL/success output,port/multiple-project guidance; I no inline pull repair. [S4]
10. **Exit/re-run:** D stop preservesDB; destructive mode separate; start reusesproject. [S4]
11. **Next:** D migrations/seeds/login/link/push. [S5]
12. **Assumptions:** D “In your repo, initialize the local Supabase project”. I project/Node assumed. [S3]
13. **Moves/friction:** I dependency orchestration good; cloudfirst/package branching obscures local trial. [S1] [S3]

D expanded CLI install guide adds version-pinning/help; counting that branch adds1command/1edit. I Studio value is not gateway traffic. [S4]

### Tailscale (pattern benchmark)

1. **Entry:** D homepage→SSOsignup,1link; native client. [T-home]
2. **Artifacts:** D OSapps; Macstandalone recommended; Linuxscript/binary. [TS-mac] [TS-linux]
3. **Prerequisites:** D SSO/two devices/Mac12+; I install/VPN privilege. [TS-q] [TS-mac]
4. **Parts/DB:** D Linuxdaemon/client; I hosted coordination,DBhidden. [TS-linux]
5. **Config/secrets:** D interactive SSO,no authoredfile/authkey in lead path. [TS-q]
6. **Value:** I6UI stages signup/SSO/use/firstdevice/seconddevice/console;5–10minutes;packet proof extra. [TS-q]
7. **Backend:** I own devices/services,no demo server. [TS-q]
8. **Surface:** D browser/native; Linuxinstall/up prints authURL. [TS-linux]
9. **Verify/help:** D live enrollment/support/firewall links,Mac troubleshooting. [TS-q] [TS-mac]
10. **Exit/re-run:** I quickstart lacks cleanup; attempted uninstall source U. [TS-q]
11. **Next:** D DNS,permissions,exitnodes,subnetrouters. [TS-q]
12. **Assumptions:** D “Next, add a second device”. I absent persona prerequisite. [TS-q]
13. **Moves/friction:** I live feedback/defaults help; account/two-device requirement. [TS-q]

D rolling docs validate2026-01-05; Linux archive example1.90.6 is not latest-release evidence. [TS-linux]

### Ollama (pattern benchmark)

1. **Entry:** D GetStartedCTA; destination retrieval U; download independently fetched. [O1] [O2]
2. **Artifacts:** D downloadcurl on Mac/Linux; Macdetail prefersDMG. [O2] [O4]
3. **Prerequisites:** D Mac14+,M-GPU/x86CPU; current model7.2GB/8GBmemory recommendation. [O4] [O3]
4. **Parts/DB:** D localserver11434; I runner count U,noexternalDB. [O5]
5. **Config/secrets:** D localmodel keyless; cloud credentials optional. [O3]
6. **Value:** I install,run gemma4:e2b,typeprompt=2commands+prompt,0files,5–20+minutes. [O2] [O3]
7. **Backend:** D model acquired by run. [O3]
8. **Surface:** D app/interactiveCLI/API. [O3]
9. **Verify/help:** D serve/version/status,OSlogs/GPU troubleshooting. [O5] [O6]
10. **Exit/re-run:** D /bye,modelstop/remove;Macuninstall separate. [O7] [O4]
11. **Next:** D API/streaming/tools,Modelfile/integrations. [O3] [O7]
12. **Assumptions:** D “Large models are slow on a computer without a strong GPU.” I hardware/download patience. [O2]
13. **Moves/friction:** I acquire+use elegant; largeassets/installer mismatch consume trial. [O3] [O4]

U installer source not retrieved,actualduration not measured; no pinned release in current quickstart. [O2] [O3]

### k3s (pattern benchmark)

1. **Entry:** D install command on homepage,0links. [KS-home]
2. **Artifacts:** D curlinstaller,binaryalternative. [KS-home]
3. **Prerequisites:** D Linux/systemd-or-openrc,server2cores/2GB,x86/ARM; I root. [KS-q] [KS-req]
4. **Parts/DB:** D bundledservice pluspods,SQLitedefault single-server. [KS-q] [KS-db]
5. **Config/secrets:** D generated kubeconfig,firstserver needs nojoin token. [KS-q]
6. **Value:** I install,getnode=2commands,0files,2–5minutes compatibleLinux;macOSnative blocked. [KS-home]
7. **Backend:** I no workload in two-command route. [KS-home]
8. **Surface:** D shell/k3s kubectl. [KS-home]
9. **Verify/help:** D Readycheck,requirements/firewall/cgroup guidance. [KS-home] [KS-req]
10. **Exit/re-run:** D generated uninstall deleteslocalDB/config/storage; serviceautostarts. [KS-stop] [KS-q]
11. **Next:** D nodejoin/HA; related-project tools,not laptopDocker recipe. [KS-q] [KS-related]
12. **Assumptions:** D “all cluster administrators should be familiar with” Kubernetes basics. I Linuxoperator intent. [KS-q]
13. **Moves/friction:** I bundledCLI/config/store; host effects/OS prerequisite substantial. [KS-q] [KS-stop]

D homepage advertises readiness~30seconds; this report's estimate is not verification. Rolling docs updated2026-10-06,no productpin. [KS-home] [KS-q]

## 6. Sources and verification audit

All sources were accessed **2026-10-07**. Dates are access dates, not page publication dates. Rolling/main/latest pages describe their displayed snapshot, not a guaranteed released binary. Source labels are clickable links wherever used.

|Ref|Official source|Access date|Doc/product version described|
|---|---|---|---|
|EG-home|[EG-home]|2026-10-07|Unversioned homepage|
|EG-q|[EG-q]|2026-10-07|latest, commands v1.9.2|
|EG-standalone|[EG-standalone]|2026-10-07|latest, v1.9.2 experimental|
|KG-home|[KG-home]|2026-10-07|Unversioned homepage|
|KG-index|[KG-index]|2026-10-07|2.4 latest|
|KG-q|[KG-q]|2026-10-07|2.4 latest; commands v2.4.3; GatewayAPI1.6.1|
|KG-demo|[KG-demo]|2026-10-07|2.4 latest; sample branch v2.4.x|
|KG-debug|[KG-debug]|2026-10-07|2.4 latest|
|C-home|[C-home]|2026-10-07|Unversioned homepage|
|C-q|[C-q]|2026-10-07|Unversioned; linked manifest1.33|
|C-kind|[C-kind]|2026-10-07|1.33|
|C-yaml|[C-yaml]|2026-10-07|release-1.33|
|E-home|[E-home]|2026-10-07|Unversioned homepage|
|E-q|[E-q]|2026-10-07|4.1.0; modified2026-05-19|
|E-docker|[E-docker]|2026-10-07|Archived2.5; modified2024-08-08|
|K1|[K1]|2026-10-07|masterREADME;unversioned|
|K2|[K2]|2026-10-07|master;Konglatest/Postgres9.5|
|K3|[K3]|2026-10-07|master;Docker20.10+|
|K4|[K4]|2026-10-07|min3.4;example3.16.0.0;decK1.66.1|
|T1|[T1]|2026-10-07|masterREADME|
|T2|[T2]|2026-10-07|Gateway5.15.0/Redis8.2.0|
|T3|[T3]|2026-10-07|currentunversionedOAS|
|A1|[A1]|2026-10-07|currenthomepage|
|A2|[A2]|2026-10-07|currentexample3.16.0|
|A3|[A3]|2026-10-07|currentunversioned|
|R1|[R1]|2026-10-07|currenthomepage;CEunpinned|
|R2|[R2]|2026-10-07|updated2019-03-11|
|R3|[R3]|2026-10-07|masterREADME|
|R4|[R4]|2026-10-07|master;gatewayunpinned;Keycloak26.0/Grafana9.1.2/ES8.4.1|
|F1|[F1]|2026-10-07|homepageadvertises3.7|
|F2|[F2]|2026-10-07|currentoverview|
|F3|[F3]|2026-10-07|v3.7|
|G1|[G1]|2026-10-07|masterREADME|
|G2|[G2]|2026-10-07|APIM4.12|
|G3|[G3]|2026-10-07|master;APIMlatest/Mongo6.0/ES8.17.2|
|G4|[G4]|2026-10-07|APIM4.12|
|G5|[G5]|2026-10-07|master;describesnightly/3.9.0override|
|G6|[G6]|2026-10-07|APIM4.12|
|S1|[S1]|2026-10-07|current homepage, unversioned; CTA target https://supabase.com/dashboard/sign-up retrieved.|
|S2|[S2]|2026-10-07|master README, unpinned rolling revision.|
|S3|[S3]|2026-10-07|current unversioned CLI local overview; npm/Homebrew route.|
|S4|[S4]|2026-10-07|current unversioned CLI docs; release not pinned, Node20+ requirement.|
|S5|[S5]|2026-10-07|current unversioned workflow.|
|T-home|[T-home]|2026-10-07|current homepage, unversioned; primary CTA login.tailscale.com.|
|TS-q|[TS-q]|2026-10-07|rolling docs, last validated2026-01-05, client release not pinned.|
|TS-linux|[TS-linux]|2026-10-07|rolling docs, last validated2026-01-05; static archive example1.90.6, not a latest-version claim.|
|TS-mac|[TS-mac]|2026-10-07|rolling docs, last validated2026-01-05; current client macOS12+.|
|O1|[O1]|2026-10-07|current homepage, unversioned; Get started click retrieval failed.|
|O2|[O2]|2026-10-07|current download page, unversioned installer; installer text at https://ollama.com/install.sh unavailable through tool.|
|O3|[O3]|2026-10-07|rolling docs, no release pin; current example gemma4:e2b.|
|O4|[O4]|2026-10-07|rolling docs, no release pin; Mac14+.|
|O5|[O5]|2026-10-07|rolling docs, no release pin.|
|O6|[O6]|2026-10-07|rolling docs; older-version example0.5.7 is not a current-release claim.|
|O7|[O7]|2026-10-07|rolling docs, no release pin.|
|KS-home|[KS-home]|2026-10-07|current homepage, latest installer not pinned.|
|KS-q|[KS-q]|2026-10-07|rolling docs, updated2026-10-06, no product-release pin.|
|KS-req|[KS-req]|2026-10-07|rolling docs, no product-release pin.|
|KS-db|[KS-db]|2026-10-07|rolling docs, updated2026-10-06; SQLite default, external DB certification versions explicitly listed there.|
|KS-stop|[KS-stop]|2026-10-07|rolling docs, updated2026-10-06.|
|KS-related|[KS-related]|2026-10-07|rolling docs, updated2026-10-01, no release pin; alternate provisioning tools.|
|L-home|[L-home]|2026-10-07|main README; unversioned|
|L-index|[L-index]|2026-10-07|rolling, unversioned|
|L-q|[L-q]|2026-10-07|rolling, unversioned; main-stable example|
|L-script|[L-script]|2026-10-07|main; read only|
|L-compose|[L-compose]|2026-10-07|main-stable gateway/Postgres16|
|L-prod|[L-prod]|2026-10-07|rolling, unversioned|
|L-release|[L-release]|2026-10-07|stable1.104.0 shown; mutable tag equality U|
|P-home|[P-home]|2026-10-07|main;2.0 pre-release banner|
|P-install|[P-install]|2026-10-07|main;latest image example|
|P-compose|[P-compose]|2026-10-07|main;latest image|
|P-release|[P-release]|2026-10-07|stable1.15.2 shown|
|AR-oldhome|[AR-oldhome]|2026-10-07|redirects to Agent Router|
|AR-home|[AR-home]|2026-10-07|current unversioned homepage|
|AR-start|[AR-start]|2026-10-07|1.2|
|AR-cli|[AR-cli]|2026-10-07|1.2;experimental|
|AR-install|[AR-install]|2026-10-07|1.1|
|AR-run|[AR-run]|2026-10-07|1.2|
|AR-release|[AR-release]|2026-10-07|1.1.0 stable/1.2.0-rc1 shown|
|AR-oldbasic|[AR-oldbasic]|2026-10-07|older indexed Kubernetes-first excerpt; version U|
|K-legacy|[K-legacy]|2026-10-07|linked OSS latest; fetch cache miss, version U|
|FP-release|[FP-release]|2026-10-07|requested v3.1.4; web fetch cache miss|
|FP-taggedbundle|[FP-taggedbundle]|2026-10-07|web fetch cache miss; local tag artifact inspected|
|curl-manual|[curl-manual]|2026-10-07|rolling current manual; consulted retry options, proposed flow uses POSIX loop|

### Flowplane source register

Read-only local docs at commit `67bd6766ebe8d17dfecae6cca1322128bfa88985`; accessed2026-10-07. Local tag `v3.1.4` is available and matches the three leading files byte-for-byte. Line citations point to the local inspected files. Immutable public URLs below retain the snapshot if the working tree changes.

|File|Snapshot URL|Version/access|
|---|---|---|
|README.md|[README.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/README.md)|3.1.4-oriented local docs /2026-10-07|
|compose.eval.yml|[compose.eval.yml snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/compose.eval.yml)|3.1.4-oriented local docs /2026-10-07|
|docs/README.md|[docs/README.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/README.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/cli-auth-and-contexts.md|[docs/how-to/cli-auth-and-contexts.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/cli-auth-and-contexts.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/evaluate-platform.md|[docs/how-to/evaluate-platform.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/evaluate-platform.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/import-and-publish-openapi-spec.md|[docs/how-to/import-and-publish-openapi-spec.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/import-and-publish-openapi-spec.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/learn-and-publish-api-spec.md|[docs/how-to/learn-and-publish-api-spec.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/learn-and-publish-api-spec.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/onboard-api-team.md|[docs/how-to/onboard-api-team.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/onboard-api-team.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/production-readiness.md|[docs/how-to/production-readiness.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/production-readiness.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/register-dataplane-mtls.md|[docs/how-to/register-dataplane-mtls.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/register-dataplane-mtls.md)|3.1.4-oriented local docs /2026-10-07|
|docs/how-to/view-team-dashboard.md|[docs/how-to/view-team-dashboard.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/how-to/view-team-dashboard.md)|3.1.4-oriented local docs /2026-10-07|
|docs/reference/adoption-evaluation-issue-map.md|[docs/reference/adoption-evaluation-issue-map.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/reference/adoption-evaluation-issue-map.md)|3.1.4-oriented local docs /2026-10-07|
|docs/reference/cli.md|[docs/reference/cli.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/reference/cli.md)|3.1.4-oriented local docs /2026-10-07|
|docs/tutorials/evaluate-no-clone.md|[docs/tutorials/evaluate-no-clone.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/tutorials/evaluate-no-clone.md)|3.1.4-oriented local docs /2026-10-07|
|docs/tutorials/getting-started.md|[docs/tutorials/getting-started.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/docs/tutorials/getting-started.md)|3.1.4-oriented local docs /2026-10-07|
|internal/README.md|[internal/README.md snapshot](https://github.com/rajeevramani/flowplane/blob/67bd6766ebe8d17dfecae6cca1322128bfa88985/internal/README.md)|3.1.4-oriented local docs /2026-10-07|
|Architecture constitution|[constitution.md](/Users/rajeevramani/workspace/projects/flowplane-private-vault/constitution.md:41)|Local canonical vault document /2026-10-07; no public URL|

### Self-check and limits

- **Comparison cells:** all17product rows and every cell carry an official-source reference, including bounded absence/U claims. Counts, times and assumptions are labelled I rather than presented as measurements.
- **Rubric:** ten definitions and thresholds are byte-preserved from the pre-Flowplane freeze; hash verified on assembly. Gateway medians derive only from the13listed gateway rows, not benchmarks. Unknown critical continuations are scored conservatively and labelled provisional.
- **Flowplane claims:** factual descriptions, gap judgments and architecture constraints cite local file:line evidence. Recommendations/transcript are explicitly proposed, not claimed existing behavior. The report changes no Flowplane runtime, defaults, topology or user docs.
- **Navigation:** content-link transitions verified where retrievable. Physical scroll counts, dynamic UI click totals and sidebar prominence in rendered viewport were not verified; stages are not clicks. Ollama CTA destination fetch failed. Kong OSS continuation fetch failed; neither fetch failure proves a broken browser link.
- **Artifacts:** published Flowplane release/registry availability, all image architecture manifests/digests, image pull success, current floating tags and external demo availability were not tested. Local Flowplane tag equivalence is document evidence only. APISIX/Ollama installer sources were not readable through the research tool, so script parity remains U.
- **Runtime:** no installation, product execution, live curl, Compose startup, release qualification or measured timings. Proposed Flowplane retry/resume and own-backend recipes need validation before publication. Native Agent Router acquisition/process count and Portkey SDK implicit endpoint are unresolved. Gravitee's gateway/backend test contradiction remains unresolved. Exact Emissary/Supabase/native Ollama process counts are undisclosed.
- **Scope:** followed prominent entry paths and directly relevant next-step/platform/troubleshooting links; did not recursively audit every feature/reference link. Historical alternatives are marked historical. This is a bounded first-run study, not a docs correctness certification or security audit.
- **Source budgets:** capture text is compressed; brief assumption quotations stay below25words per source. Repetition is limited across table, patterns and appendix. Effort estimates and pattern-fit judgments are research inference.

Implementation follow-ups are tracked in Beads: `fpv2-7xu` covers validation/readiness/recovery/lifecycle; existing `fpv2-drz` covers the own-API expose journey. Packaging/native proposals remain design candidates rather than approved build tasks. The report's ranked proposals are the requested research deliverable, not an authorization to implement architectural changes.

[EG-home]: https://gateway.envoyproxy.io/ "Accessed 2026-10-07; Unversioned homepage"
[EG-q]: https://gateway.envoyproxy.io/docs/tasks/quickstart/ "Accessed 2026-10-07; latest, commands v1.9.2"
[EG-standalone]: https://gateway.envoyproxy.io/docs/tasks/operations/standalone-deployment-mode/ "Accessed 2026-10-07; latest, v1.9.2 experimental"
[KG-home]: https://kgateway.dev/ "Accessed 2026-10-07; Unversioned homepage"
[KG-index]: https://kgateway.dev/docs/envoy/latest/ "Accessed 2026-10-07; 2.4 latest"
[KG-q]: https://kgateway.dev/docs/envoy/latest/quickstart/ "Accessed 2026-10-07; 2.4 latest; commands v2.4.3; GatewayAPI1.6.1"
[KG-demo]: https://kgateway.dev/docs/envoy/latest/install/sample-app/ "Accessed 2026-10-07; 2.4 latest; sample branch v2.4.x"
[KG-debug]: https://kgateway.dev/docs/envoy/latest/operations/debug/ "Accessed 2026-10-07; 2.4 latest"
[C-home]: https://projectcontour.io/ "Accessed 2026-10-07; Unversioned homepage"
[C-q]: https://projectcontour.io/getting-started/ "Accessed 2026-10-07; Unversioned; linked manifest1.33"
[C-kind]: https://projectcontour.io/docs/1.33/guides/kind/ "Accessed 2026-10-07; 1.33"
[C-yaml]: https://raw.githubusercontent.com/projectcontour/contour/release-1.33/examples/render/contour.yaml "Accessed 2026-10-07; release-1.33"
[E-home]: https://emissary-ingress.dev/ "Accessed 2026-10-07; Unversioned homepage"
[E-q]: https://emissary-ingress.dev/docs/4.1/quick-start/ "Accessed 2026-10-07; 4.1.0; modified2026-05-19"
[E-docker]: https://emissary-ingress.dev/docs/2.5/topics/install/docker/ "Accessed 2026-10-07; Archived2.5; modified2024-08-08"
[K1]: https://github.com/Kong/kong "Accessed 2026-10-07; masterREADME;unversioned"
[K2]: https://raw.githubusercontent.com/Kong/docker-kong/master/compose/docker-compose.yml "Accessed 2026-10-07; master;Konglatest/Postgres9.5"
[K3]: https://github.com/Kong/docker-kong/blob/master/compose/README.md "Accessed 2026-10-07; master;Docker20.10+"
[K4]: https://developer.konghq.com/gateway/get-started/ "Accessed 2026-10-07; min3.4;example3.16.0.0;decK1.66.1"
[T1]: https://github.com/TykTechnologies/tyk "Accessed 2026-10-07; masterREADME"
[T2]: https://tyk.io/docs/deployment-and-operations/tyk-open-source-api-gateway/quick-start "Accessed 2026-10-07; Gateway5.15.0/Redis8.2.0"
[T3]: https://tyk.io/docs/api-management/gateway-config-managing-oas "Accessed 2026-10-07; currentunversionedOAS"
[A1]: https://apisix.apache.org/ "Accessed 2026-10-07; currenthomepage"
[A2]: https://apisix.apache.org/docs/apisix/getting-started/README/ "Accessed 2026-10-07; currentexample3.16.0"
[A3]: https://apisix.apache.org/docs/apisix/getting-started/configure-routes/ "Accessed 2026-10-07; currentunversioned"
[R1]: https://www.krakend.io/ "Accessed 2026-10-07; currenthomepage;CEunpinned"
[R2]: https://www.krakend.io/docs/overview/installing/ "Accessed 2026-10-07; updated2019-03-11"
[R3]: https://github.com/krakend/playground-community "Accessed 2026-10-07; masterREADME"
[R4]: https://github.com/krakend/playground-community/blob/master/docker-compose.yml "Accessed 2026-10-07; master;gatewayunpinned;Keycloak26.0/Grafana9.1.2/ES8.4.1"
[F1]: https://traefik.io/traefik "Accessed 2026-10-07; homepageadvertises3.7"
[F2]: https://doc.traefik.io/traefik/getting-started/ "Accessed 2026-10-07; currentoverview"
[F3]: https://doc.traefik.io/traefik/getting-started/docker/ "Accessed 2026-10-07; v3.7"
[G1]: https://github.com/gravitee-io/gravitee-api-management "Accessed 2026-10-07; masterREADME"
[G2]: https://documentation.gravitee.io/apim/getting-started/local-install-with-docker "Accessed 2026-10-07; APIM4.12"
[G3]: https://raw.githubusercontent.com/gravitee-io/gravitee-api-management/master/docker/quick-setup/mongodb/docker-compose.yml "Accessed 2026-10-07; master;APIMlatest/Mongo6.0/ES8.17.2"
[G4]: https://documentation.gravitee.io/apim/getting-started/create-and-publish-your-first-api/create-an-api "Accessed 2026-10-07; APIM4.12"
[G5]: https://github.com/gravitee-io/gravitee-api-management/blob/master/docker/README.md "Accessed 2026-10-07; master;describesnightly/3.9.0override"
[G6]: https://documentation.gravitee.io/apim/getting-started/create-and-publish-your-first-api/publish-your-api "Accessed 2026-10-07; APIM4.12"
[S1]: https://supabase.com/ "Accessed 2026-10-07; current homepage, unversioned; CTA target https://supabase.com/dashboard/sign-up retrieved."
[S2]: https://github.com/supabase/supabase "Accessed 2026-10-07; master README, unpinned rolling revision."
[S3]: https://supabase.com/docs/guides/local-development "Accessed 2026-10-07; current unversioned CLI local overview; npm/Homebrew route."
[S4]: https://supabase.com/docs/guides/local-development/cli/getting-started "Accessed 2026-10-07; current unversioned CLI docs; release not pinned, Node20+ requirement."
[S5]: https://supabase.com/docs/guides/local-development/cli-workflows "Accessed 2026-10-07; current unversioned workflow."
[T-home]: https://tailscale.com/ "Accessed 2026-10-07; current homepage, unversioned; primary CTA login.tailscale.com."
[TS-q]: https://tailscale.com/docs/how-to/quickstart "Accessed 2026-10-07; rolling docs, last validated2026-01-05, client release not pinned."
[TS-linux]: https://tailscale.com/docs/install/linux "Accessed 2026-10-07; rolling docs, last validated2026-01-05; static archive example1.90.6, not a latest-version claim."
[TS-mac]: https://tailscale.com/docs/install/mac "Accessed 2026-10-07; rolling docs, last validated2026-01-05; current client macOS12+."
[O1]: https://ollama.com/ "Accessed 2026-10-07; current homepage, unversioned; Get started click retrieval failed."
[O2]: https://ollama.com/download "Accessed 2026-10-07; current download page, unversioned installer; installer text at https://ollama.com/install.sh unavailable through tool."
[O3]: https://docs.ollama.com/quickstart "Accessed 2026-10-07; rolling docs, no release pin; current example gemma4:e2b."
[O4]: https://docs.ollama.com/macos "Accessed 2026-10-07; rolling docs, no release pin; Mac14+."
[O5]: https://docs.ollama.com/linux "Accessed 2026-10-07; rolling docs, no release pin."
[O6]: https://docs.ollama.com/troubleshooting "Accessed 2026-10-07; rolling docs; older-version example0.5.7 is not a current-release claim."
[O7]: https://docs.ollama.com/cli "Accessed 2026-10-07; rolling docs, no release pin."
[KS-home]: https://k3s.io/ "Accessed 2026-10-07; current homepage, latest installer not pinned."
[KS-q]: https://docs.k3s.io/quick-start "Accessed 2026-10-07; rolling docs, updated2026-10-06, no product-release pin."
[KS-req]: https://docs.k3s.io/installation/requirements "Accessed 2026-10-07; rolling docs, no product-release pin."
[KS-db]: https://docs.k3s.io/datastore "Accessed 2026-10-07; rolling docs, updated2026-10-06; SQLite default, external DB certification versions explicitly listed there."
[KS-stop]: https://docs.k3s.io/installation/uninstall "Accessed 2026-10-07; rolling docs, updated2026-10-06."
[KS-related]: https://docs.k3s.io/related-projects "Accessed 2026-10-07; rolling docs, updated2026-10-01, no release pin; alternate provisioning tools."
[L-home]: https://github.com/BerriAI/litellm "Accessed 2026-10-07; main README; unversioned"
[L-index]: https://docs.litellm.ai/docs/simple_proxy "Accessed 2026-10-07; rolling, unversioned"
[L-q]: https://docs.litellm.ai/docs/proxy/docker_quick_start "Accessed 2026-10-07; rolling, unversioned; main-stable example"
[L-script]: https://raw.githubusercontent.com/BerriAI/litellm/main/scripts/quickstart.sh "Accessed 2026-10-07; main; read only"
[L-compose]: https://raw.githubusercontent.com/BerriAI/litellm/main/docker/docker-compose.quickstart.yml "Accessed 2026-10-07; main-stable gateway/Postgres16"
[L-prod]: https://docs.litellm.ai/docs/proxy/deploy "Accessed 2026-10-07; rolling, unversioned"
[L-release]: https://github.com/BerriAI/litellm/releases "Accessed 2026-10-07; stable1.104.0 shown; mutable tag equality U"
[P-home]: https://github.com/Portkey-AI/gateway "Accessed 2026-10-07; main;2.0 pre-release banner"
[P-install]: https://github.com/Portkey-AI/gateway/blob/main/docs/installation-deployments.md "Accessed 2026-10-07; main;latest image example"
[P-compose]: https://raw.githubusercontent.com/Portkey-AI/gateway/main/docker-compose.yaml "Accessed 2026-10-07; main;latest image"
[P-release]: https://github.com/Portkey-AI/gateway/releases "Accessed 2026-10-07; stable1.15.2 shown"
[AR-oldhome]: https://aigateway.envoyproxy.io/ "Accessed 2026-10-07; redirects to Agent Router"
[AR-home]: https://theagentrouter.ai/ "Accessed 2026-10-07; current unversioned homepage"
[AR-start]: https://theagentrouter.ai/docs/getting-started/ "Accessed 2026-10-07; 1.2"
[AR-cli]: https://theagentrouter.ai/docs/cli/ "Accessed 2026-10-07; 1.2;experimental"
[AR-install]: https://theagentrouter.ai/docs/cli/aigwinstall/ "Accessed 2026-10-07; 1.1"
[AR-run]: https://theagentrouter.ai/docs/cli/aigwrun/ "Accessed 2026-10-07; 1.2"
[AR-release]: https://github.com/theagentrouter/agent-router/releases "Accessed 2026-10-07; 1.1.0 stable/1.2.0-rc1 shown"
[AR-oldbasic]: https://aigateway.envoyproxy.io/docs/latest/getting-started/basic-usage/ "Accessed 2026-10-07; older indexed Kubernetes-first excerpt; version U"
[K-legacy]: https://docs.konghq.com/gateway-oss/latest/getting-started/configuring-a-service/ "Accessed 2026-10-07; linked OSS latest; fetch cache miss, version U"
[FP-release]: https://github.com/rajeevramani/flowplane/releases/tag/v3.1.4 "Accessed 2026-10-07; requested v3.1.4; web fetch cache miss"
[FP-taggedbundle]: https://raw.githubusercontent.com/rajeevramani/flowplane/v3.1.4/compose.eval.yml "Accessed 2026-10-07; web fetch cache miss; local tag artifact inspected"
[curl-manual]: https://curl.se/docs/manpage.html "Accessed 2026-10-07; rolling current manual; consulted retry options, proposed flow uses POSIX loop"
[FP-readme]: /Users/rajeevramani/workspace/projects/flowplane/README.md:13 "Accessed 2026-10-07; inspected local snapshot"
[FP-version]: /Users/rajeevramani/workspace/projects/flowplane/README.md:20 "Accessed 2026-10-07; inspected local snapshot"
[FP-start]: /Users/rajeevramani/workspace/projects/flowplane/README.md:28 "Accessed 2026-10-07; inspected local snapshot"
[FP-scope]: /Users/rajeevramani/workspace/projects/flowplane/README.md:54 "Accessed 2026-10-07; inspected local snapshot"
[FP-dashboard]: /Users/rajeevramani/workspace/projects/flowplane/README.md:40 "Accessed 2026-10-07; inspected local snapshot"
[FP-auth]: /Users/rajeevramani/workspace/projects/flowplane/README.md:44 "Accessed 2026-10-07; inspected local snapshot"
[FP-contributor]: /Users/rajeevramani/workspace/projects/flowplane/README.md:99 "Accessed 2026-10-07; inspected local snapshot"
[FP-dashboardfeature]: /Users/rajeevramani/workspace/projects/flowplane/README.md:152 "Accessed 2026-10-07; inspected local snapshot"
[FP-prereq]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:5 "Accessed 2026-10-07; inspected local snapshot"
[FP-evalstart]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:13 "Accessed 2026-10-07; inspected local snapshot"
[FP-proof]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:24 "Accessed 2026-10-07; inspected local snapshot"
[FP-dashguide]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:40 "Accessed 2026-10-07; inspected local snapshot"
[FP-cliguide]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:54 "Accessed 2026-10-07; inspected local snapshot"
[FP-lifecycle]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:63 "Accessed 2026-10-07; inspected local snapshot"
[FP-binding]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:139 "Accessed 2026-10-07; inspected local snapshot"
[FP-next]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:146 "Accessed 2026-10-07; inspected local snapshot"
[FP-teardown]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/evaluate-no-clone.md:167 "Accessed 2026-10-07; inspected local snapshot"
[FP-composehead]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:22 "Accessed 2026-10-07; inspected local snapshot"
[FP-composeinit]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:151 "Accessed 2026-10-07; inspected local snapshot"
[FP-composebottom]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:269 "Accessed 2026-10-07; inspected local snapshot"
[FP-volumes]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:399 "Accessed 2026-10-07; inspected local snapshot"
[FP-apiport]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:179 "Accessed 2026-10-07; inspected local snapshot"
[FP-demo]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:201 "Accessed 2026-10-07; inspected local snapshot"
[FP-dashport]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:312 "Accessed 2026-10-07; inspected local snapshot"
[FP-agent]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:335 "Accessed 2026-10-07; inspected local snapshot"
[FP-envoy]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:335 "Accessed 2026-10-07; inspected local snapshot"
[FP-secrets]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:112 "Accessed 2026-10-07; inspected local snapshot"
[FP-initcli]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:237 "Accessed 2026-10-07; inspected local snapshot"
[FP-cphealth]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:185 "Accessed 2026-10-07; inspected local snapshot"
[FP-pkifail]: /Users/rajeevramani/workspace/projects/flowplane/compose.eval.yml:78 "Accessed 2026-10-07; inspected local snapshot"
[FP-expose]: /Users/rajeevramani/workspace/projects/flowplane/docs/reference/cli.md:393 "Accessed 2026-10-07; inspected local snapshot"
[FP-import]: /Users/rajeevramani/workspace/projects/flowplane/docs/how-to/import-and-publish-openapi-spec.md:73 "Accessed 2026-10-07; inspected local snapshot"
[FP-platform]: /Users/rajeevramani/workspace/projects/flowplane/docs/how-to/evaluate-platform.md:9 "Accessed 2026-10-07; inspected local snapshot"
[FP-onboard]: /Users/rajeevramani/workspace/projects/flowplane/docs/how-to/onboard-api-team.md:5 "Accessed 2026-10-07; inspected local snapshot"
[FP-getting]: /Users/rajeevramani/workspace/projects/flowplane/docs/tutorials/getting-started.md:11 "Accessed 2026-10-07; inspected local snapshot"
[FP-constitution]: /Users/rajeevramani/workspace/projects/flowplane-private-vault/constitution.md:41 "Accessed 2026-10-07; inspected local snapshot"
