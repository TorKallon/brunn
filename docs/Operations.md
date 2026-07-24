# Straylight Local Operations

## Prerequisites

- Docker Desktop or Docker Engine with Compose and BuildKit
- an OpenAI API key for semantic indexing, automatic capture, and deep dreaming
- free ports recorded in the Nyx project port map

Create `.env` from `.env.example`, replace every placeholder secret, and keep
the file mode at `0600`. Do not place secrets in Compose files, logs, test
fixtures, or MCP configuration.

## Local Endpoints

| Service | Bind |
| --- | --- |
| SPA | `0.0.0.0:13110` |
| Postgres | `127.0.0.1:15110` |
| API | `127.0.0.1:18110` |
| MinIO S3 | `127.0.0.1:19110` |
| MinIO console | `127.0.0.1:19111` |

The SPA proxies `/api` to the API. On the local tailnet, use
`http://nyx:13110/` for human inspection. Databases and object storage remain
localhost-only.

## Lifecycle

```bash
make config
make build
make up
make ps
```

Useful commands:

```bash
make logs
make migrate
make mcp
docker compose down
```

`docker compose down` preserves named Postgres and MinIO volumes. Do not use
`down -v` unless intentionally destroying all local Straylight data.

The repository builds its own pinned PostgreSQL 17 plus pgvector 0.8.5 image.
New clusters use PostgreSQL's built-in `C.UTF-8` collation and page checksums;
the database healthcheck rejects an OS-dependent or checksum-disabled cluster.
An older libc-backed cluster must be moved with the coordinated logical
backup/restore path, not mounted into the Alpine runtime.

The native agent adapter emits minified JSON by default. Use
`./memory --pretty <operation>` only for human inspection. MCP emits textual
JSON without a second `structuredContent` copy by default; set
`STRAYLIGHT_MCP_INCLUDE_STRUCTURED_CONTENT=1` only for a client that explicitly
requires it.

## Health

```bash
curl -fsS http://127.0.0.1:18110/health
curl -fsS http://127.0.0.1:18110/ready
docker compose ps
```

Readiness reports database, object-store, and embedding state separately.
Embedding degradation must name its provider and model.

## Verification

```bash
python3 -m unittest discover -s tests -v
python3 tests/live_api_smoke.py \
  --base-url http://127.0.0.1:18110 \
  --env-file .env
python3 tests/live_alpha_safety.py --env-file .env
python3 tests/live_runtime_safety.py --env-file .env
make object-store-check

cd apps/web
npm run build
npm test -- --run

cd ../mcp
npm run build
npm test
```

Rust checks run in the pinned builder image or through Compose:

```bash
docker build -f apps/api/Dockerfile --target builder .
docker compose run --rm migrate
```

The live smoke is destructive only within uniquely provisioned evaluation
users. Temporary credentials are revoked and dream jobs are reviewed or
rejected during cleanup. The separate alpha-safety test exercises complete
export and retention-gated account deletion. The live smoke also performs a
real automatic capture, proves replay and source-integrity failure semantics,
retrieves the committed fact, and opens one fresh hard-gated learned view
before review.

Capture and scheduler tuning is explicit in `.env`:

```bash
STRAYLIGHT_CAPTURE_MODEL=gpt-5.6
STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS=8192
STRAYLIGHT_DREAM_SCHEDULER_POLL_SECONDS=15
STRAYLIGHT_DREAM_INACTIVITY_SECONDS=60
STRAYLIGHT_DREAM_COOLDOWN_SECONDS=900
```

The scheduler remains shadow-only. These intervals control refresh latency,
not authority or active-corpus promotion.

## Production Observability

Straylight emits production metrics through DogStatsD when
`STRAYLIGHT_METRICS_ENABLED=true`. The exporter aggregates counters and
histograms in process, sends histograms as Datadog distributions, and fails
open: an unavailable Agent must never make the API or worker unavailable.

Set the deployment identity and Agent credentials in `.env`:

```bash
STRAYLIGHT_METRICS_ENABLED=true
STRAYLIGHT_DOGSTATSD_ADDR=datadog-agent:8125
STRAYLIGHT_METRICS_FLUSH_SECONDS=3
DD_API_KEY=<Datadog API key>
DD_SITE=datadoghq.com
DD_ENV=production
DD_SERVICE=straylight
DD_VERSION=<immutable release version>
```

Start or inspect the opt-in Agent profile:

```bash
make observability-up
make observability-status
make observability-logs
```

An externally managed Datadog Agent is also supported. Point
`STRAYLIGHT_DOGSTATSD_ADDR` at its reachable UDP address and omit the Compose
profile. API and worker metrics share `env`, `service`, and `version`; the
bounded `component` tag distinguishes them.

The checked-in dashboard is
`infra/datadog/straylight-production-dashboard.json`. It covers HTTP demand and
errors, retrieval quality and lane behavior, reads and deterministic compute,
writes and capture, model and embedding usage, dreaming, background queues,
deletion propagation, database pools, and object storage. Dashboard queries
use the exact emitted metric names.

Metric tags are deliberately content-free and bounded. Never add user, tenant,
credential, session, record, scope, source, path, query, title, request ID,
model output, error message, or arbitrary input as a metric tag. Use logs and
audited database records for individual-event investigation. High-cardinality
identifiers would both leak context and make custom metrics unbounded.

Quick validation:

```bash
docker compose --env-file .env --profile observability config --quiet
docker compose --env-file .env exec datadog-agent agent status
python3 tests/dogstatsd_wire_smoke.py --env-file .env
```

After first production traffic, confirm `straylight.http.requests`,
`straylight.runtime.alive`, and `straylight.worker.cycles` are reporting for
the expected `env`, `service`, `version`, and `component` tags.

Datadog stores distributions immediately but does not enable p50/p95/p99
aggregation automatically. Once representative traffic has created the metric
names, apply the bounded queryable-tag allowlists and enable percentiles:

```bash
make datadog-configure
python3 infra/datadog/configure_percentiles.py --strict
```

The non-strict command skips distributions that have not reported yet and is
safe to rerun. The strict form is the release gate for every percentile widget
in the production dashboard.

## Evaluation

Set the native adapter environment without printing the token:

```bash
export STRAYLIGHT_API_URL=http://127.0.0.1:18110
export STRAYLIGHT_EVAL_TOKEN='<owner read/write token>'
```

Run the unchanged filesystem baseline and native service only:

```bash
python3 agent_work_eval.py --manifest eval/work_cases.json run \
  --filesystem-native --concurrency 3 --timeout 420 \
  --out results/native-main.json

python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json run \
  --filesystem-native --concurrency 3 --timeout 420 \
  --out results/native-rupture-ops.json

python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json run \
  --filesystem-native --concurrency 3 --timeout 420 \
  --out results/native-personal-coordination.json

python3 transition_eval.py --manifest eval/transition_cases.json run \
  --filesystem-native --concurrency 2 --timeout 420 \
  --out results/native-transitions.json
```

Native provisioning writes one-time case credentials to
`runs/<run-id>/.native-provisioning.json` with mode `0600` before starting the
next case. Resume an interrupted run with `--resume-run-id <run-id>`; the
public JSON result contains only redacted provisioning metadata. Transition
runs use the same private state and resume option.

Provision full suites serially when they use OpenAI embeddings. Each case is an
isolated user and intentionally reimports the frozen corpus; launching several
fresh suites together can exhaust the account embedding-token-per-minute limit
even though ordinary query traffic is healthy. Once a suite's private
provisioning state is complete, agent runs may execute concurrently. An
interrupted provision resumes from the already committed case imports.

Every `/v1/admin/eval/import` scope is created with automatic dreaming disabled
and paused. This prevents synthetic benchmark corpora from consuming dream
work or changing retrieval during a comparison; ordinary user scopes retain
the configured automatic scheduler behavior.

Always pass `--filesystem-native` for the product gate. The legacy `workspace`
condition intentionally exercises the frozen Python/SQLite reference harness,
not the Rust service.

Cases marked `active: false` are retained for historical reproducibility but
are excluded from normal runs because their product premise has been retired.
Pass `--include-retired` or select the case explicitly with `--case` to replay
one. Native agents should treat `memory.open` as their first evidence packet,
query only unresolved gaps, and batch repeated `--path` or `--ref` reads in one
call when several exact sources are required. Agent adapters default a focused
query to eight source leads. Use `--neighbors N` for symmetric context around a
chunk reference and `--range START:END` for exact source lines. A
`complete_source` already contains the full source and must not be read again.
Treat `likely_sufficient` as primary-source and anchor coverage, then inspect
each requested task facet before deciding whether a focused query is needed.

Evaluation operation records split `result_chars` into `source_text_chars` and
`metadata_chars`, and also record complete sources, pointer sources,
sufficiency status, and replay-weighted characters. Compare uncached input
separately from cumulative cached-history replay.

The active agent-work manifests use `concept_tokens_v1`: exact phrases still
match, while conservative token-set matching accepts reordered paraphrases,
equivalent negation forms, and rate notation while preserving negation and
exact identifier/date components. This prevents harmless wording differences
from being reported as retrieval losses; source-path checks, forbidden
conclusions, checkpoint structure, and every claim's concept requirements
remain deterministic. The matcher treats `remove` and `exclude` as equivalent
boundary operations and does not count an explicitly negated forbidden phrase
as an asserted conclusion.

## Migration Discipline

- Migrations are ordered, transactional, embedded in the Rust binary, and
  applied before API or worker startup.
- Never edit a migration that has been applied to a durable environment. Add a
  new migration.
- Test both a fresh database and a repeated no-op migration.
- A migration checksum mismatch is a schema-drift failure, not something to
  bypass with broad `IF NOT EXISTS` clauses.

## Storage And Backup

Postgres is canonical for records, revisions, manifests, credentials, jobs,
and audit. MinIO is canonical for source and artifact blobs. A usable backup
must capture both stores and preserve their shared revision point.

`scripts/backup.sh` implements a quiesced, coordinated, checksummed backup of a
serializable PostgreSQL dump and every object-store version. The v2 manifest
records retention, deletion expiry, exact runtime images, Compose hashes, and
the database collation/checksum invariants; `scripts/prune-backups.sh` refuses
unknown or unverifiable bundles. `scripts/restore-drill.sh` streams the dump
into a resource-bounded isolated Compose project and verifies database/object
inventories, storage invariants, migrations, API health, and operator
onboarding/recovery.

Use `make production-backup` with the production overlay and an approved
backup root. The schedule, off-host encrypted destination, key custody, and
final RPO/RTO are launch-owner decisions. See `Alpha Launch Runbook.md`.

## Security Notes

- Do not expose Postgres, MinIO, or the API directly to the LAN or internet.
- Treat the SPA origin as the local human control boundary.
- Use read-only tokens for less agentic or untrusted clients.
- Ordinary read/write tokens cannot manage credentials.
- Evaluation import is an administrative local harness surface and should not
  be exposed unchanged in a public deployment.
- The API sends the submitted source plus bounded current context to OpenAI for
  `memory.capture`; the dream worker sends bounded selected evidence for deep
  consolidation. Both request paths use `store: false` and a privacy-preserving
  user-scoped safety identifier.

## Troubleshooting

Start with:

```bash
docker compose ps
docker compose logs --tail=200 migrate api worker web
curl -fsS http://127.0.0.1:18110/ready
```

For retrieval failures, inspect per-lane coverage and freshness before changing
ranking. For write failures, keep idempotency, base revision, expected version,
and source evidence intact. For capture drafts, inspect `issues`, `unresolved`,
the compiled `draft.save_request`, and the model receipt before changing a
guardrail. For missing learned context, compare the session revision with the
candidate base revision and inspect hard gates and retrieval evaluation. For
migrations, fix the schema or migration rather than manually marking it
applied.
