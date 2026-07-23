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

The live smoke is destructive within uniquely provisioned evaluation users. It
leaves those isolated users and corpora because the alpha does not expose a
user-delete endpoint. Temporary credentials are revoked and dream jobs are
reviewed or rejected during cleanup. It also performs a real automatic capture,
proves replay and source-integrity failure semantics, retrieves the committed
fact, and opens one fresh hard-gated learned view before review.

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
chunk reference and `--range START:END` for exact source lines.

The active agent-work manifests use `concept_tokens_v1`: exact phrases still
match, while conservative token-set matching accepts reordered paraphrases,
equivalent negation forms, and rate notation while preserving negation and
exact identifier/date components. This prevents harmless wording differences
from being reported as retrieval losses; source-path checks, forbidden
conclusions, checkpoint structure, and every claim's concept requirements
remain deterministic.

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

Before public deployment, define and test:

- Postgres physical or logical backup schedule
- MinIO versioned-bucket replication
- coordinated restore procedure
- recovery point and recovery time objectives
- encryption and key rotation
- deletion propagation across backups and replicas

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
