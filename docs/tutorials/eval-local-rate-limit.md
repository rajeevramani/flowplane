# Add a local rate limit

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

**Outcome:** Manually install a listener-wide policy, observe HTTP 429, and verify recovery.

**Prerequisites and starting state:** Complete [Expose your own backend](eval-expose-own-backend.md). Start with both `/` and `/hello` working on listener `demo`; keep the host backend running. Use the same evaluation terminal with `fp` available. Python 3 extracts and validates the editable JSON; policy editing is manual.

Run commands individually and inspect each result. **Stop on failures or unexpected results; do not continue blindly.** These are manual steps, not an onboarding script.

**Release boundary:** matching v3.2.0 container images are not yet published. This draft is not packaged-candidate or cross-platform qualification. See the [evaluation learning path](evaluate-no-clone.md) for the shared release and platform limitations.

This manual experiment adds a **listener-wide**, per-Envoy token bucket. It affects both `/` and `/hello`, not a fleet-wide/global quota. The listener filter chain must contain `local_rate_limit`; a per-route override alone cannot add a missing filter.

### Read the current listener and prepare an editable file

```sh
fp -o json listener get demo > eval-listener-before.json
```

```sh
cat eval-listener-before.json
```

Inspect `data.revision` and `data.spec`. If a local rate limit already exists, stop and inspect it instead of adding another. Keep all existing spec fields and filters.

The following command only extracts the current `spec` into an editable PATCH file. It does not add a policy or update Flowplane:

```sh
python3 -c 'import json; from pathlib import Path; resource=json.loads(Path("eval-listener-before.json").read_text())["data"]; print(json.dumps({"spec": resource["spec"]}, indent=2))' > eval-local-limit.json
```

Open `eval-local-limit.json` in your editor. Under `spec.http_filters`, append this entry to the existing array (create the array if absent). Preserve every other field and existing filter:

```json
{
  "filter": {
    "type": "local_rate_limit",
    "stat_prefix": "eval_local",
    "token_bucket": {
      "max_tokens": 2,
      "tokens_per_fill": 2,
      "fill_interval_ms": 5000
    }
  },
  "disabled": false
}
```

Save the edited file, then check its JSON syntax and contents:

```sh
python3 -m json.tool eval-local-limit.json
```

Expected: valid JSON containing the full preserved `spec`, with exactly one new local-rate-limit entry. Stop if parsing fails or any unrelated field/filter was lost.

### Apply the revision-checked update

Transfer your edited file into the CLI container. Run this command by itself so its standard input is only the specified file:

```sh
docker compose -f compose.eval.yml exec -T flowplane-eval \
  sh -ec 'cat > /tmp/eval-local-limit.json' < eval-local-limit.json
```

Set `revision` to the **exact `data.revision` you inspected above**. `1` below is only an example; replace it with that observed number before running:

```sh
revision=1
```

```sh
fp --revision "$revision" listener update demo --file /tmp/eval-local-limit.json
```

Expected: a successful update. A revision conflict means the listener changed since your read: stop, read it again, and reconcile your edit. Do not blindly replace the revision and retry an old file.

```sh
fp listener get demo
```

Inspect the returned spec for the new filter and confirm unrelated fields remain. Wait a few seconds for configuration delivery before testing.

### Observe the limit manually

Keep other callers idle. Run this command once at a time, quickly, several times:

```sh
curl --max-time 2 -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:10000/
```

Expected: successful requests return `200`; after the two-token bucket is exhausted, a request returns **`429`**. Repeat the command rapidly enough to exhaust the bucket within its five-second refill interval. Do not treat a connection error, `404` or `5xx` as rate-limit evidence. If you cannot observe `429`, stop and inspect the policy rather than claiming the limit works.

Wait at least six seconds without other traffic, then run:

```sh
curl --max-time 5 -i http://127.0.0.1:10000/
```

Expected: **HTTP 200** and the original body `hello from the flowplane eval demo upstream`. Confirm both. If either is wrong, stop and diagnose.

Envoy's data-plane 429 does not promise a `Retry-After` header here. It differs from the control-plane write throttle in the [error reference](../reference/errors.md). See the [filter reference](../reference/filters.md) for route overrides and [global rate limiting](../how-to/global-rate-limit.md) for quotas shared across Envoys.

## Continue

[Next: Remove APIs safely](eval-remove-apis.md). For failures, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md); do not retry mutations as readiness checks.
