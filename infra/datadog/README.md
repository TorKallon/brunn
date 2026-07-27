# Datadog Dashboard

The source-controlled production dashboard is
`straylight-production-dashboard.json`.

The first dashboard group is the primary owner view for the simplified
workspace. It uses only metrics emitted by `simple_core`, `simple_worker`, and
`usage`: open/write latency, evidence count, retrieval-lane timeouts, job
outcomes/duration/batch size, and usage-buffer health. The existing HTTP route
latency graph supplies per-route visibility for broad workspace searches.

The simplified `straylight.jobs` table is not included in the current telemetry
snapshot, so no dashboard or monitor claims to show its queue depth, oldest job,
or terminal failed-row count. Those remain explicit instrumentation gaps. The
legacy `straylight.worker.queue.*` widgets and monitor cover legacy durable
queues only. The simplified API does expose per-lane duration/candidate counts
and separate retry versus terminal-failure job transitions. It does not scan
the workspace merely to publish entry counts, chunk counts, or embedding lag;
those remain explicit gaps until they can be measured without adding recurring
full-corpus work. HTTP route latency remains the broad-search alert signal.

Published dashboard:

- ID: `iqu-ei7-654`
- URL: <https://app.datadoghq.com/dashboard/iqu-ei7-654>

The definition is valid for Datadog's v1 ordered-dashboard API with automatic
reflow. Update the existing dashboard instead of creating duplicates:

```bash
curl -fsS -X PUT \
  "https://api.${DD_SITE:-datadoghq.com}/api/v1/dashboard/iqu-ei7-654" \
  -H "Content-Type: application/json" \
  -H "DD-API-KEY: ${DD_API_KEY}" \
  -H "DD-APPLICATION-KEY: ${DD_APP_KEY}" \
  --data-binary @infra/datadog/straylight-production-dashboard.json
```

Keep API and application keys in the environment or secret manager. Never add
them to this repository.

Datadog distributions require percentile aggregation to be enabled after a
metric first reports. Apply the source-controlled tag allowlists and enable
percentiles with:

```bash
make datadog-configure
```

The command skips metrics that have not reported yet so it is safe during a
rolling deployment. Run it again after each new distribution first appears.
Use `python3 infra/datadog/configure_percentiles.py --strict` as a release gate
after representative production traffic.
