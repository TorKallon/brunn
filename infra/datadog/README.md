# Datadog Dashboard

The source-controlled production dashboard is
`straylight-production-dashboard.json`.

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
