# Remove APIs safely

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

**Outcome:** Remove shared exposures in both orders, verify the survivor, and inspect final cleanup.

**Prerequisites and starting state:** Complete [Expose your own backend](eval-expose-own-backend.md), and optionally [Add a local rate limit](eval-local-rate-limit.md). Start with `demo` and `own` sharing listener `demo`, and both response bodies verified. Keep the host backend running until survivor checks finish. Use your evaluation terminal with `fp` available.

Run commands individually and inspect each result. **Stop on failures or unexpected results; do not continue blindly.** These are manual steps, not an onboarding script.

**Release boundary:** matching v3.2.0 container images are not yet published. This draft is not packaged-candidate or cross-platform qualification. See the [evaluation learning path](evaluate-no-clone.md) for the shared release and platform limitations.

`fp` disables terminal allocation, so pass `--yes` deliberately for destructive commands. These commands remove gateway configuration, not your database or infrastructure.

First remove the original exposure:

```sh
fp --yes unexpose demo
```

Expected: the original upstream is deleted, but the shared listener/config are retained for `own`. If you completed the rate-limit tutorial, the listener-wide policy remains active. Removing `demo` does not authorize deletion of all similarly named resources.

Wait for configuration delivery and at least six seconds without traffic for the bucket refill, then check the survivor:

```sh
curl --max-time 5 -fsS http://127.0.0.1:10000/hello
```

Expected body: `hello from my API`.

```sh
fp listener get demo
```

```sh
fp route get demo-routes
```

Expected: the shared resources still exist, the surviving route is retained, and any installed policy is retained. Stop on an unexpected result before proceeding to final cleanup.

Remove the final exposure:

```sh
fp --yes unexpose own
```

Then inspect each inventory separately:

```sh
fp cluster list
```

```sh
fp route list
```

```sh
fp listener list
```

Expected: all three inventories are empty for this otherwise untouched example. Final managed cleanup deletes the shortcut-created listener/config and their later policy edits. Dataplane registration, backend process and infrastructure remain. A borrowed manual/legacy scaffold is never shortcut-deleted; references or invalid last-route removal may reject cleanup atomically. Resolve dependencies through supported lifecycles, not force-deletion.

## Repeat in the opposite removal order

Start only after the inventories above are empty. Recreated resources have new identities, and the previous rate-limit policy was deleted with the managed listener. Keep your own backend running. Use the same engine-specific host name you verified in the [own-backend tutorial](eval-expose-own-backend.md); the command below uses the locally exercised Podman variant.

```sh
fp expose http://demo-upstream:5678 --name demo --path / --port 10000 \
  --public-base-url http://127.0.0.1:10000
```

```sh
fp expose http://host.containers.internal:3001 --name own --path /hello --listener demo
```

Verify both bodies before removal; allow a few seconds for xDS delivery and repeat only the reads if necessary:

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/
```

Expected: `hello from the flowplane eval demo upstream`.

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/hello
```

Expected: `hello from my API`.

Now remove the attached exposure first:

```sh
fp --yes unexpose own
```

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/
```

Expected after configuration delivery: the sample body still arrives. Inspect the retained resources:

```sh
fp listener get demo
```

```sh
fp route get demo-routes
```

Expected: the listener/config remain with the original sample route, but no `own` route. Stop if the survivor or resource state is unexpected.

```sh
fp --yes unexpose demo
```

```sh
fp cluster list
```

```sh
fp route list
```

```sh
fp listener list
```

Expected: all three inventories are empty. Pre-3.2 legacy trios have no shortcut association: matching names do not authorize adoption or deletion. See [Expose an API](../how-to/expose-an-api.md).

Stop the host backend with Ctrl-C in its terminal. API removal does not stop the evaluation infrastructure. For stopping while preserving volumes, resuming, or deliberately erasing this evaluation, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md). To test persistence of exposures or policies, follow that guide **before** removing them; `down` cannot restore configuration removed by `unexpose`. Never reset somebody else's stack or use a destructive reset as the first readiness fix.


## Continue

[Return to the evaluation learning path](evaluate-no-clone.md). For failures, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md); do not retry mutations as readiness checks.
