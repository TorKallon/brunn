# Brunn Observability

## Contract

Production metrics are opt-in, fail open, and do not change any API request,
response, authorization, persistence, retrieval, or dreaming semantic.
Brunn uses the Rust `metrics` facade with the DogStatsD exporter. The
exporter aggregates locally and submits every histogram as a Datadog
distribution.

Every custom metric has the `brunn.` prefix and these global tags:

| Tag | Meaning |
| --- | --- |
| `env` | Deployment environment, from `DD_ENV` |
| `service` | Stable service name, from `DD_SERVICE` |
| `version` | Immutable release version, from `DD_VERSION` |
| `component` | `api`, `worker`, `migrate`, or `healthcheck` |

Domain tags are fixed enumerations such as route template, operation, result,
status, retrieval lane, compute operator, queue, and storage operation.
Unknown values are folded into `other`.

The DogStatsD exporter also emits its standard
`datadog.dogstatsd.client.*` self-telemetry without the Brunn prefix. It
inherits the same global tags and exposes submitted points, packets, bytes,
aggregation, serialization drops, and transport drops.

The following are prohibited as metric tags: user or tenant ID, credential,
scope, session, record, source, object key, filesystem path, query text, title,
task text, request ID, model output, error message, or any other arbitrary
input. Individual investigations use structured logs and audited records.

Structured production logs are delivered to Datadog through the selected
platform's supported log stream. Logs may include service, component, release,
severity, operation, stable error code, and request ID for correlation. They
must not include API credentials, secret material, memory or source content,
query or task text, model prompts or outputs, uploaded payloads, or raw
third-party response bodies. Log delivery is observational and fail-open; a
Datadog outage cannot change a memory operation.

The hosted MCP gateway emits one content-free outcome record for each POST to
`/mcp`, including a correlation ID, JSON-RPC method, bounded tool name, response
status, elapsed time, request size, and only the SHA-256 of a valid
`idempotency_key`. It never logs tool arguments, paths, content, raw keys, OAuth
tokens, or response bodies. A missing record during a client-reported tool
failure therefore means the call did not reach Brunn's `/mcp` ingress.

## Metric Families

| Prefix | Operational question |
| --- | --- |
| `brunn.runtime.*` | Are API and worker processes alive and restarting? |
| `brunn.http.*` | How much traffic arrives, where, how large, and how slow? |
| `brunn.api.errors` | Which stable API failures are rising? |
| `brunn.auth.*` | Are credentials valid and capabilities correctly denied? |
| `brunn.operation.*` | Which memory operations complete, degrade, or conflict? |
| `brunn.retrieval.*` | Which lanes run, fail, return candidates, or leave gaps? |
| `brunn.read.*` | Which exact views are slow, partial, or truncated? |
| `brunn.compute.*` | Which deterministic operators are slow or partial? |
| `brunn.verify.*` | What classifications and structural checks are produced? |
| `brunn.write.*` | What is committed, replayed, deduplicated, or reviewed? |
| `brunn.capture.*` | Does automatic capture commit, draft, or degrade? |
| `brunn.projection.*` | How much policy projection includes, withholds, or transforms? |
| `brunn.usage_tracking.*` | Is post-policy record-use telemetry being persisted? |
| `brunn.model.*` | What model latency, outcomes, and token mix are observed? |
| `brunn.embedding.*` | Are embedding calls healthy, sized, and keeping up? |
| `brunn.dream.*` | Are scheduler, model, candidate, and review flows healthy? |
| `brunn.worker.*` | Are durable jobs progressing and queues aging? |
| `brunn.deletion.*` | Which deletion surfaces remove, retain, or fail? |
| `brunn.db.*` | Are transactions or connection pools under pressure? |
| `brunn.object_store.*` | Is object storage healthy, slow, or moving unusual volume? |
| `brunn.stage.*` | Are staged imports large, warned, or quarantined? |
| `brunn.asset.access.*` | Are native files being found and downloaded successfully, and at what volume? |
| `brunn.asset.description.*` | Are searchable derivative descriptions completing, falling back, or failing? |
| `brunn.asset.upload.*` | Are resumable uploads progressing, replaying safely, or failing integrity checks? |
| `brunn.asset.storage.*` | How much logical data exists versus physically deduplicated object data? |
| `brunn.vault.export.*` | Are revision-pinned portable manifests being produced successfully? |
| `brunn.dependency.*` | Is a required external dependency ready or degraded? |
| `brunn.telemetry.*` | Did periodic observability snapshots fail? |
| `datadog.dogstatsd.client.*` | Is the exporter sending or dropping packets and bytes? |

Counters describe events, gauges describe current state, and distributions
describe latency, size, counts per operation, tokens, and queue age. Use rates
for counters, current values for gauges, and p50/p95/p99 for distributions.

## Dashboard

`infra/datadog/brunn-production-dashboard.json` is the source-controlled
dashboard definition. Its template variables are `env`, `service`, `version`,
and `component`. The groups are ordered for incident use:

1. service overview
2. usage and cost drivers
3. API and authorization
4. reasoning and retrieval quality
5. persistence and learning
6. models and embeddings
7. background work and deletion
8. dependencies and telemetry health
9. native assets and portability

The model-and-embedding group keeps exact model token totals, cached-input
tokens, output tokens, embedding request volume, and embedding input size
prominent for the selected dashboard window. With one initial user, those
aggregate totals represent the owner's workload. Datadog deliberately does not
receive user IDs. Per-source most-used, least-used, never-used, and
least-recently-used views remain in the authenticated Control UI.

The dashboard can exist before metrics arrive. A new deployment is considered
observable only after the runtime, HTTP, and worker series appear with the
expected unified service tags.

The Railway Datadog Agent also performs outside-in HTTP checks against the
public `/healthz` and `/api/ready` URLs every 15 seconds. Unlike application
heartbeats and private-network readiness checks, these canaries exercise public
DNS, TLS, the Railway edge-to-Web connection, and the proxied API/durable-
dependency boundary. Readiness accepts an explicitly degraded optional
embedding provider but still requires the database and object store. Their
`http.can_connect` service checks are the availability signals;
`network.http.response_time` is the successful-response latency signal. The
instance and URL tags distinguish ingress from API or durable-dependency
failure. The checked-in Agent configuration uses a three-second timeout, so it
does not inherit Railway's repeated five-second edge dial delay.

## Alert Starting Points

Alert thresholds require production baselines. Begin with monitors for:

- API or worker `runtime.alive` absent for two collection windows
- sustained 5xx rate or a sharp increase in stable API error codes
- p95 API latency above the agreed service objective
- semantic dependency degraded
- database pool utilization above 0.8
- queued job age above its expected completion window
- any deletion surface failure
- capture or dream model degradation above the accepted ratio
- telemetry snapshot errors or DogStatsD client packet drops

Tune thresholds from actual production distributions; do not encode test-only
latencies or traffic assumptions as permanent policy.
