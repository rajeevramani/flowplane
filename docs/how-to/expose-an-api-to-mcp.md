# Expose an existing API to MCP

> Audience: api-teams · Status: draft — 3.2.0 release preparation; runtime qualification pending

This is an **optional** continuation after the [evaluation tutorial](../tutorials/evaluate-no-clone.md). First HTTP success does not require MCP. Use the existing exposed listener and exact route from that journey; this example assumes listener `demo`, route config `demo-routes`, route `demo`, virtual-host name `default`, and reachable gateway URL `http://127.0.0.1:10000/`. If you already removed that exposure, follow [Expose an API](expose-an-api.md) to recreate it and verify the sample body before continuing. Do not repeat exposure mutation in a readiness loop.

No new MCP tool, new provider, or new execution path is introduced here.

An imported API tool is not served before explicit publish. `tools/list` establishes visibility; `tools/call` invokes the generated tool but returns a `gateway_invocation` descriptor, not a backend response. The caller must separately execute bounded HTTP through Envoy to observe the backend.

Keep the management credential separate from backend traffic auth. MCP authenticates to the control plane with a management principal; the sample gateway remains unauthenticated unless you separately configured traffic protection. Never forward the management bearer token to the gateway. Use an explicit local allowlisted endpoint for this disposable example. Never enable automatic redirects forwarding credentials, or execute arbitrary returned hosts, headers or shell commands.

**Prerequisites:** Python 3 on the host, the tutorial's `fp()` helper in this terminal, and an existing MCP Streamable HTTP client that can privately attach your management bearer credential. The client targets your control-plane base URL plus `/api/v1/mcp` (default local evaluation: `http://127.0.0.1:8080/api/v1/mcp`). Outside evaluation, use your existing supported login/client configuration. In the evaluator the generated management credential lives at `/shared/dev-token` inside the control-plane container. Attaching it privately to a chosen MCP client is client-specific and is not qualified here; start only with an already securely configured client, otherwise stop rather than print/export the token. Do not copy credentials into the JSON requests below or saved transcripts. This page does not configure a particular agent client or claim clipboard/client-UI qualification.

For this team the principal needs API definitions Create/Read/Update (and Delete only for the explicit optional cleanup), route configs/listeners Read, MCP tools Execute for dynamic `tools/list`/`tools/call`, and MCP tools Read for the REST catalog/status inspection. The evaluator's dev-org administrator supplies the management rights, not backend traffic authentication. Use your existing approved team grants outside evaluation; do not grant platform-wide authority just for this example.

## 1. Import a spec bound to the existing route

Keep each complete parenthesized block intact when pasting into interactive zsh. The evaluator helper intentionally forwards stdin through Compose `exec -T` for imports; an ungrouped first command can consume the remaining input before the shell parses the later commands. A shell exit of zero is not proof that later deletion or verification ran.

Run in a fresh working directory where these metadata files do not already exist. This block saves inspected IDs and a minimal OpenAPI document, then pipes that document to the CLI inside the evaluator. `/dev/stdin` here is the container's input stream, not a host pathname. The `fp()` helper's `-T` preserves that stream. Stop if any block fails; a name collision is not permission to reuse or delete somebody else's API.

The shortcut's route config is `demo-routes`, listener `demo`, virtual-host name `default` and exact route name `demo`—not a universal `all` route. First inspect the returned specs and confirm they still represent your sample exposure.

```sh
(
set -eu
umask 077
for mcp_metadata_file in mcp-demo-route.json mcp-demo-listener.json mcp-demo-openapi.json mcp-demo-created.json; do
  if [ -e "$mcp_metadata_file" ] || [ -L "$mcp_metadata_file" ]; then
    printf '%s\n' "Refusing to overwrite $mcp_metadata_file" >&2
    exit 1
  fi
done
fp -o json route get demo-routes > mcp-demo-route.json
fp -o json listener get demo > mcp-demo-listener.json
python3 - <<'PY'
import json
from pathlib import Path
route = json.loads(Path('mcp-demo-route.json').read_text())['data']
listener = json.loads(Path('mcp-demo-listener.json').read_text())['data']
if route['name'] != 'demo-routes' or listener['name'] != 'demo':
    raise SystemExit('Unexpected sample exposure identity')
if not isinstance(route['id'], str) or not isinstance(listener['id'], str):
    raise SystemExit('Invalid sample exposure IDs')
spec = {'openapi': '3.0.3', 'info': {'title': 'Local demo', 'version': '1'},
        'paths': {'/': {'get': {'operationId': 'getDemo', 'responses': {
            '200': {'description': 'Demo text', 'content': {
                'text/plain': {'schema': {'type': 'string'}}}}}}}}}
with Path('mcp-demo-openapi.json').open('x') as output:
    json.dump(spec, output)
PY
mcp_route_id=$(python3 -c 'import json; print(json.load(open("mcp-demo-route.json"))["data"]["id"])')
mcp_listener_id=$(python3 -c 'import json; print(json.load(open("mcp-demo-listener.json"))["data"]["id"])')
cat mcp-demo-openapi.json | fp -o json api create mcp-demo --from-openapi /dev/stdin \
  --route-config-id "$mcp_route_id" --listener-id "$mcp_listener_id" \
  --virtual-host default --route demo > mcp-demo-created.json
fp -o json api status mcp-demo
)
```

The imported version is 1. Save the actual create reply and inspect its API identity; do not fabricate a response or infer that import already made the tool servable. The API must retain a listener binding and the listener's reachable `public_base_url` for a descriptor to be constructed.

## 2. Publish deliberately, then inspect the catalog

Import is not publication. Follow [Import and publish OpenAPI](import-and-publish-openapi-spec.md) with your inspected IDs and exact route. For imported version 1, publication uses the existing CLI:

```sh
(
set -eu
mcp_observed_revision=$(fp -o json api status mcp-demo | python3 -c '
import json, sys
from pathlib import Path
created = json.loads(Path("mcp-demo-created.json").read_text())["data"]["api"]
current = json.load(sys.stdin)["data"]["api"]
if current["id"] != created["id"]:
    raise SystemExit("API ownership mismatch: stopped before mutation")
if not current["name"] == created["name"] == "mcp-demo":
    raise SystemExit("API name mismatch: stopped before mutation")
if type(current["revision"]) is not int or current["revision"] <= 0:
    raise SystemExit("Invalid API revision")
print(current["revision"])
')
fp api spec publish mcp-demo 1 --reason 'Reviewed local demo operation'
fp -o json api status mcp-demo
fp -o json mcp status
fp -o json mcp tools
)
```

Publication does **not** enforce an expected API revision on the server. The ID/name check above is a preflight only; it does not protect against concurrent changes or replacement between checking and publishing. Use this disposable example without concurrent editors. The observed revision is diagnostic, not a publication lock. Deletion in section 5 separately enforces its expected revision.

Check the actual API status and team catalog; do not infer backend invocation from a positive tool count. An enabled, published catalog declaration is not itself a backend response.

## 3. Request a descriptor using your MCP client

Initialize the authenticated client with protocol version `2025-11-25`. Keep the returned `Mcp-Session-Id` associated with the same management principal and send it on subsequent requests. Send `notifications/initialized`; the empty HTTP 202 is an acknowledgement, not an API response. Supply `team: "default"` for the evaluator rather than assuming a session-wide team default.

Send this JSON-RPC request through that client's authenticated MCP transport:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"team":"default"}}
```

Choose the exact generated `api_` tool name for your published demo operation from the returned list. Do not guess a tool name or select an unrelated operation. A name visible in another team's catalog is not authority to call it here.

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"<exact-generated-tool-name-from-tools-list>","arguments":{"team":"default"}}}
```

Reject a JSON-RPC error or a result with `isError: true`. Decode the JSON text entry in `result.content` and save only that descriptor as `mcp-demo-invocation.json`. The descriptor identifies a method, URL, body, headers, IDs, `auth.mode: "caller_gateway_credentials"`, expiration and correlation ID. Neither its existence nor its correlation ID proves the backend was contacted. Calling an unpublished, disabled or unbound tool does not establish HTTP success.

## 4. Separately call the gateway, not the control plane

For this exact root-GET sample, manually validate the saved descriptor against the local allowlist before issuing a separate gateway request. Never run returned data as shell code. Do not follow redirects or automatically forward returned authorization headers. This is a local allowlist example, not a general-purpose descriptor executor.

```sh
(
set -eu
python3 - <<'PY'
import json
from pathlib import Path
value = json.loads(Path('mcp-demo-invocation.json').read_text())
if not (value['type'] == 'gateway_invocation'):
    raise SystemExit('Descriptor is outside the sample allowlist')
if not (value['method'] == 'GET'):
    raise SystemExit('Descriptor is outside the sample allowlist')
if not (value['url'] == 'http://127.0.0.1:10000/'):
    raise SystemExit('Descriptor is outside the sample allowlist')
if not (value['body'] is None):
    raise SystemExit('Descriptor is outside the sample allowlist')
if not (value['headers'] == {'host': 'default'}):
    raise SystemExit('Descriptor is outside the sample allowlist')
if not (value['auth']['mode'] == 'caller_gateway_credentials'):
    raise SystemExit('Descriptor is outside the sample allowlist')
PY
body=$(curl --max-time 3 -fsS -H 'Host: default' http://127.0.0.1:10000/)
[ "$body" = 'hello from the flowplane eval demo upstream' ]
printf '%s\n' "$body"
)
```

The descriptor currently emits the virtual-host **name** as `Host: default`. This sample works because the exposed virtual host has wildcard domains `*`; that header is not a promise that arbitrary named domains will match.

The hard-coded request is the explicit local allowlist, not automatic execution of a descriptor URL. A different listener, path, method or traffic credential requires deliberate client configuration and separate verification. This example does not qualify arbitrary upstreams, credential forwarding or an agent's automatic tool executor.

## 5. Preserve data and remove only your resources

Existing bindings and captures can block shortcut unexpose. Stop active learning through the supported lifecycle when applicable; terminal capture history can still block final removal. Stopping learning does not remove binding rows or historical foreign keys. This import-only example starts no learning session.

Inspect the demo API definition and its ownership first. When you explicitly choose to delete the disposable API you just created, use its supported API lifecycle to remove that definition/binding before attempting shortcut removal. Do not delete somebody else's API or blindly retry a conflict. Preserve existing volumes; no teardown/reset is required by this continuation. The following block compares the fresh API ID/name with the successful create reply in `mcp-demo-created.json` before deletion, stops on missing/malformed/mismatched metadata, and pins the observed revision instead of auto-fetching a later one. Only delete the API you just created; imported spec/tool rows and its binding belong to that API lifecycle. Inspect the actual lifecycle response and the team catalog afterward; a shell exit alone is not a deletion readback.

```sh
(
set -eu
mcp_api_revision=$(fp -o json api status mcp-demo | python3 -c '
import json, sys
from pathlib import Path
created = json.loads(Path("mcp-demo-created.json").read_text())["data"]["api"]
current = json.load(sys.stdin)["data"]["api"]
if current["id"] != created["id"]:
    raise SystemExit("API ownership mismatch: stopped before mutation")
if not current["name"] == created["name"] == "mcp-demo":
    raise SystemExit("API name mismatch: stopped before mutation")
if type(current["revision"]) is not int or current["revision"] <= 0:
    raise SystemExit("Invalid API revision")
print(current["revision"])
')
fp --yes --revision "$mcp_api_revision" api delete mcp-demo
fp -o json mcp tools
)
```

The sample gateway exposure is separate. After confirming no bindings/captures or other references remain, you may separately choose `fp --yes unexpose demo` using the primary guide and inspect its dispositions. Do not run that removal just to finish this optional walkthrough; retaining your working HTTP exposure is valid. No destructive volume reset is part of this recipe.

For general learning, see [Learn and publish](learn-and-publish-api-spec.md). For shared-listener/reference conflicts and safe gateway removal, use [Expose an API](expose-an-api.md#remove-the-exposure-without-deleting-somebody-elses-infrastructure).
