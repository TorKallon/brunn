# Brunn Local Operations

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
`down -v` unless intentionally destroying all local Brunn data.

The repository builds its own pinned PostgreSQL 17 plus pgvector 0.8.5 image.
New clusters use PostgreSQL's built-in `C.UTF-8` collation and page checksums;
the database healthcheck rejects an OS-dependent or checksum-disabled cluster.
An older libc-backed cluster must be moved with the coordinated logical
backup/restore path, not mounted into the Alpine runtime.

The native agent adapter emits minified JSON by default. Use
`./memory --pretty <operation>` only for human inspection. MCP emits textual
JSON without a second `structuredContent` copy by default; set
`BRUNN_MCP_INCLUDE_STRUCTURED_CONTENT=1` only for a client that explicitly
requires it.

The production remote connector endpoint and OAuth runbook are documented in
[`Remote MCP.md`](Remote%20MCP.md). It is separate from the local stdio
registration and must never be given an owner or credential-management token.

Semantic retrieval remains an experimental, default-off accelerator:

```bash
BRUNN_SEMANTIC_LANE=false
BRUNN_EMBED_CACHE=true
BRUNN_SEMANTIC_DEADLINE_MS=2500
BRUNN_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS=5000
BRUNN_SEMANTIC_QUERY_CONCURRENCY=8
BRUNN_EMBEDDING_BACKFILL_GUARD=true
```

`BRUNN_SEMANTIC_DEADLINE_MS=0` removes the semantic-specific deadline but
does not remove the outer 2.5-second retrieval-lane timeout. The semantic lane
runs concurrently with exact+lexical. For a hybrid request, exact+lexical form
the response barrier: semantic candidates are included only when ready by that
point, and a pending or failed optional semantic lane neither delays nor marks
the response partial. Semantic-only requests wait for the lane under its
2.5-second bound, which gives normal cold provider requests time to complete.
Online query embeddings are deduplicated per normalized query (single flight), capped globally at
`BRUNN_SEMANTIC_QUERY_CONCURRENCY` concurrent provider calls (1–64), and
hard-bounded by `BRUNN_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS` (1–60000)
independent of the 60-second provider client used by backfill; a timed-out or
failed call is negative-cached for 60 seconds. A provider task already launched
by a hybrid request remains bounded and may warm the cache after that response
returns. After semantic index readiness succeeds, one batched search coalesces
its distinct uncached semantic queries into provider batches of at most 16
items and 100,000 UTF-8 bytes while retaining per-query tickets, ordering,
single-flight, and deadlines (an individual query over that byte target remains
a singleton, preserving the existing query contract). Turning
`BRUNN_EMBEDDING_BACKFILL_GUARD=false` stops the worker from claiming
embedding jobs. With the guard on, each publication is capped at 64 chunks and
full batches are separated by at least 250ms. In Compose the worker also reads
the API process's rolling foreground-latency snapshot from
`http://api:8080/health/foreground-latency`. Hosted environments must set
`BRUNN_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL` to a private/internal API
URL and may tune its request timeout with
`BRUNN_EMBEDDING_BACKFILL_FOREGROUND_STATUS_TIMEOUT_MS`.

The snapshot schema is `brunn-foreground-latency@v1`, covers 60 seconds,
and contains only sample counts, p95 values, newest-sample ages, and generation
time. It contains no query text, paths, user IDs, or credentials. A configured
worker pauses embedding claims when either p95 exceeds its open/search limit
or when the endpoint is unavailable, non-successful, malformed, stale by more
than 10 seconds, or more than 5 seconds in the future. An absent endpoint URL
preserves standalone local-development behavior but does not satisfy the D12
backfill-guard gate. Keep this route on the private service network in hosted
deployments.

`/ready` reports the active flags and whether a foreground status URL is
configured; embedding degradation remains visible there but cannot remove an
otherwise healthy database/object-store-backed API replica. Authenticated
`/v1/status` additionally reports semantic cache, explicit-lane deferral, and
hybrid opportunistic-miss counters, plus actual query-provider batch and item
counts. The E03/E09 harnesses validate these values and the immutable build
revision before their guarded backfill or reasoning work.

## Agent Secret Vault

Trusted agents store, retrieve, list, and delete named secrets through the
`secret.put`, `secret.get`, `secret.list`, and `secret.delete` MCP tools
(`/v1/workspace/secrets*`). Values are AES-256-GCM ciphertext in the dedicated
`brunn.secrets` table, outside the memory corpus: never embedded,
dreamed, searched, exported, logged, or written to object storage. Account
exports include secret metadata but strip `value_ciphertext` and
`value_nonce`. A content-free `brunn.secret_access_log` records which
credential performed each put, get, and delete.

The API process needs `BRUNN_SECRET_ENCRYPTION_KEY` (or its `_FILE`
sibling): base64 that decodes to exactly 32 bytes, generated by
`make production-secrets` in self-hosted production and stored as an API
service variable on Railway. The key is deliberately separate from the
database, so a database backup alone cannot reveal values; without the key the
secret endpoints fail closed while every other product keeps working. The
additional authenticated data binds each ciphertext to the deployment
environment, user, secret, and version, so rotating the key (unlike the
notification token key) has no self-service reset: re-store each secret after
a rotation.

Access is gated by the `secret:read` and `secret:write` capabilities, both in
the API and in row-level security. Ordinary memory and read-only credentials
receive no secret access; owner (`credential:manage`) credentials carry both
capabilities. Future per-agent scoping can restrict individual secrets to
individual credentials without schema changes because every access already
records the acting credential.

## Notifications And APNs

The durable notification inbox is independent of push. Publishing creates the
authenticated inbox record first; APNs is a best-effort wake-up transport for
enabled iOS installations. Denied permission, a missing device, or provider
failure therefore cannot erase the alert.

Notification configuration is deliberately split by process:

| Process | Configuration |
| --- | --- |
| API | `BRUNN_APNS_APP_ID`, `BRUNN_APNS_DELIVERY_ENABLED`, plus `BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY` or its `_FILE` sibling, so authenticated installation upserts can validate the app topic, encrypt device tokens, and suppress transport work while the release gate is off. |
| Worker | The API values plus `BRUNN_APNS_TEAM_ID`, `BRUNN_APNS_KEY_ID`, and `BRUNN_APNS_PRIVATE_KEY` or its `_FILE` sibling. The worker sends only when the release gate is on. |
| Migrate, Web, MCP | No notification token or APNs provider secrets. |

`BRUNN_APNS_DELIVERY_ENABLED` defaults to `false` and must have the same
value on API and worker. While false, new notification deliveries are recorded
as suppressed, previously queued/running transport work is also suppressed,
and the worker never constructs an APNs provider or sends to Apple. When true,
worker startup fails closed unless the complete provider
configuration is usable. Turn it on only for an approved signed-device canary
or owner rollout; turning it off again stops new transport attempts without
affecting the durable inbox.

On Railway, changing a service variable does not alter the environment snapshot
of an existing deployment. A plain service restart therefore retains the old
value; create a new deployment for both worker and API after changing the gate.
Deploy the worker first and require its `APNs notification delivery enabled`
startup line before deploying the API. A delivery suppressed under the old
gate is terminal, so every post-rollout canary needs a fresh `event_key`.

`BRUNN_APNS_APP_ID` is the non-secret iOS bundle ID
(`com.rourkem.brunn`). Apple team and key identifiers are non-secret but
remain worker-only because no other process uses them. The notification token
key must be base64 that decodes to exactly 32 bytes. Every supported secret
accepts a sibling `_FILE` variable; never set the direct value and file form
together. In self-hosted production, Compose mounts
`notification_token_encryption_key` into API and worker and mounts
`apns_private_key` only into worker. Railway stores the same values as service
variables, with APNs team ID, key ID, and private key only on worker.

`make production-secrets` generates the token-encryption key and imports an
operator-supplied Apple `.p8` key without printing either value:

```bash
make production-secrets \
  SECRETS_DIR=/approved/private/path \
  OPENAI_KEY_FILE=/approved/openai-key \
  RESEND_KEY_FILE=/approved/resend-key \
  DATADOG_KEY_FILE=/approved/datadog-key \
  APNS_PRIVATE_KEY_FILE=/approved/AuthKey_KEYID.p8
```

For another secret manager, generate 32 random bytes and base64-encode them
without logging the result, then supply them through
`BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY_FILE`. Rotating this key is a
coordinated installation reset: existing encrypted device tokens cannot be
decrypted with a new key and must be registered again.

The signed app reports `development` or `production` from its
`BRUNN_APNS_ENVIRONMENT` build setting. That value is stored with the
installation and selects Apple's sandbox or production endpoint. It must match
the provisioning profile and `aps-environment` entitlement that produced the
device token; a token from one environment is invalid in the other. The
checked-in Debug configuration uses `development` and Release uses
`production`; verify the resolved entitlement against the provisioning profile
for every signed archive.

Push payloads are intentionally generic and opaque: a schema version, opaque
notification and delivery references, and a strict
`brunn://notification` route. They never contain briefing prose, topics,
paths, source URLs, or other private content. Tapping a push opens the
authenticated durable notification detail first; any briefing or entry target
is a secondary explicit action from that screen. `accepted_by_apns` means
Apple accepted the provider request. It does not prove the phone received,
displayed, or sounded the alert. Opens and explicit acknowledgements are
separate receipts.

Installation lifecycle follows the account, not an individual Web session.
Registrations survive normal 30-day session rotation and password reset.
Explicit “Disconnect this iPhone” revokes the installation before clearing the
local account session. Account deletion removes installations, deliveries, and
receipts through the user-owned database cascade. No production APNs rollout
is complete until signed-device development and production canaries cover a
briefing, intraday alert, correction, cold launch, retry, token rotation,
invalid token, denied permission, and authenticated detail opening.

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
BRUNN_CAPTURE_MODEL=gpt-5.6
BRUNN_CAPTURE_MAX_OUTPUT_TOKENS=8192
BRUNN_DREAM_SCHEDULER_POLL_SECONDS=15
BRUNN_DREAM_INACTIVITY_SECONDS=60
BRUNN_DREAM_COOLDOWN_SECONDS=900
```

The scheduler remains shadow-only. These intervals control refresh latency,
not authority or active-corpus promotion.

## Production Observability

Brunn emits production metrics through DogStatsD when
`BRUNN_METRICS_ENABLED=true`. The exporter aggregates counters and
histograms in process, sends histograms as Datadog distributions, and fails
open: an unavailable Agent must never make the API or worker unavailable.

Set the deployment identity and Agent credentials in `.env`:

```bash
BRUNN_METRICS_ENABLED=true
BRUNN_DOGSTATSD_ADDR=datadog-agent:8125
BRUNN_METRICS_FLUSH_SECONDS=3
DD_API_KEY=<Datadog API key>
DD_SITE=datadoghq.com
DD_ENV=production
DD_SERVICE=brunn
DD_VERSION=<immutable release version>
```

Start or inspect the opt-in Agent profile:

```bash
make observability-up
make observability-status
make observability-logs
```

An externally managed Datadog Agent is also supported. Point
`BRUNN_DOGSTATSD_ADDR` at its reachable UDP address and omit the Compose
profile. API and worker metrics share `env`, `service`, and `version`; the
bounded `component` tag distinguishes them.

The checked-in dashboard is
`infra/datadog/brunn-production-dashboard.json`. It covers HTTP demand and
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

After first production traffic, confirm `brunn.http.requests`,
`brunn.runtime.alive`, and `brunn.worker.cycles` are reporting for
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

## Vault And Asset CLI

The same `brunn-state` binary is present in the API image. A local dry run needs
no token and performs no write:

```bash
docker compose run --rm --no-deps \
  --entrypoint /usr/local/bin/brunn-state \
  --volume /path/to/vault:/vault:ro api vault import \
  --root /vault --scope scope:root --vault-id owner-vault --dry-run
```

For a live import, mount the vault read-only, set `BRUNN_STATE_API_TOKEN` through
the environment or a secret file, and omit `--dry-run`. Missing files are
retained. To make local absence authoritative, first run with `--mirror`; the
CLI prints the paths it would remove and an exact `--confirm-mirror` value.
Run again with that value only after reviewing the preview.

Portable export never overwrites an existing destination:

```bash
brunn-state vault export \
  --vault-id owner-vault --scope scope:root --output /backups/owner-vault
```

Add `--history` to include superseded source versions. Verify
`CHECKSUMS.sha256` before treating an export as usable. This portable directory
is for source-native files and human-readable workspace state; the separate
account export remains the complete service disaster-recovery artifact.

To restore a portable export through the vault importer, pass its `sources/`
directory as `--root` and keep `manifest.json` plus `CHECKSUMS.sha256` beside
it. Brunn State verifies the manifest checksum and each source hash before
reusing the exported MIME type, nanosecond modification time, and mode.
Generated companions in any reserved
`.brunn-state/generated/descriptions/` subtree are ignored and recreated. An
identical repeat reports `status: unchanged`; it does not create a new
authoritative revision.

An agent fetches one pinned native version without loading it into the protocol
response:

```bash
brunn-state asset fetch \
  --session-id session:... --asset-ref asset:... --version 1 \
  --output /private/work/receipt.png
```

The CLI creates the output privately, streams to a temporary file, verifies
size and SHA-256, and refuses overwrite or symlink traversal.

## Evaluation

Set the native adapter environment without printing the token:

```bash
export BRUNN_API_URL=http://127.0.0.1:18110
export BRUNN_EVAL_TOKEN='<owner read/write token>'
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

### Real-provider semantic fault proxy

Real-provider E03 mode 3 and E09 semantic-arm stacks use the checked-in
`eval/openai_embedding_fault_proxy.py`. Give every isolated stack unique
state/config/log paths, ports, and an instance ID. Set the API's
`OPENAI_BASE_URL` to the proxy's `/v1` URL; the API keeps the provider key and
the proxy forwards its authorization header without logging it.

For definitive E03 Mode 3, use `eval/e03_mode3.py` rather than issuing two
performance commands manually. The wrapper verifies API and worker point to
the exact numeric Docker-gateway proxy URL, proves the proxy identity from both
container network namespaces with a locally present digest-pinned helper image
and no pull, binds exact API/worker/DB image IDs, owns the proxy lifecycle, and
runs the built-in single-import cold/warm pair. It requires the official OpenAI
upstream and records provider token-usage actuals. Separate cold and warm
imports are invalid because the second import changes table and HNSW
cardinality.

Start with the upstream base URL free of credentials, query, and fragment:

```bash
python3 eval/openai_embedding_fault_proxy.py start \
  --state runs/<run>/proxy-state.json \
  --config runs/<run>/proxy-config.json \
  --log runs/<run>/proxy.log \
  --instance-id <run-unique-id> \
  --port <run-unique-port> \
  --upstream-base-url https://api.openai.com/v1
```

If control is reachable from anything other than loopback, also supply
`--control-token-file <owner-only-file>` on start and every control command.
The start receipt contains the implementation and upstream URL SHA-256 values.
Bind both values and the exact instance ID into every hook:

```bash
python3 eval/openai_embedding_fault_proxy.py configure \
  --state runs/<run>/proxy-state.json \
  --instance-id <run-unique-id> \
  --expect-implementation-sha256 <start-receipt-value> \
  --expect-upstream-base-url-sha256 <start-receipt-value> \
  --mode error --error-status 503

python3 eval/openai_embedding_fault_proxy.py configure \
  --state runs/<run>/proxy-state.json \
  --instance-id <run-unique-id> \
  --expect-implementation-sha256 <start-receipt-value> \
  --expect-upstream-base-url-sha256 <start-receipt-value> \
  --mode forward
```

The historical E09 deadline arms use `--mode slow --delay-ms 800` together with
their explicitly pinned 300/600ms configurations; keep those values when
reproducing the experiment. They are not the current hybrid-response contract.
Current fault qualification must verify both behaviors: a hybrid request
returns complete exact+lexical evidence without a semantic failure gap, and a
semantic-only request reports its bounded outcome at 2.5 seconds. Configure
commands emit only a fixed attestation schema. Definitive performance runs use
`--require-semantic-failure-hook-attestation`; the HTTP deadline probe requires
attestation unless explicitly run in non-definitive local mode with
`--allow-unattested-hooks`. Both harnesses require the injection and restore
attestations to identify the same proxy, implementation, and upstream, with a
newer restore revision. Restore forward mode before stopping the run-owned
proxy. Stop is fingerprint-bound too:

```bash
python3 eval/openai_embedding_fault_proxy.py stop \
  --state runs/<run>/proxy-state.json \
  --instance-id <run-unique-id> \
  --expect-implementation-sha256 <start-receipt-value> \
  --expect-upstream-base-url-sha256 <start-receipt-value>
```

The semantic HTTP probe provisions one unique read/write evaluation identity.
`DELETE /v1/workspace/admin/eval/imports/{import_id}` is available only on an
evaluation-enabled stack and only to that identity's own delete-capable scoped
credential. It atomically removes searchable fixture state and jobs, soft
deletes its entries, and revokes all credentials for that evaluation user.

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

`interface_eval.py` is the complete agent-interface gate. With no agent,
interface, suite, or case filters it runs all frozen cases through Codex and
OpenClaw using Brunn State CLI, MCP, raw HTTP, and the matched local-file
control. The current 52-case suite therefore schedules 416 fresh-agent runs.
Use `--interface` only for a deliberate focused diagnostic; a release
comparison must retain the filesystem cells.

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
and audit. The configured S3-compatible object store is canonical for exact
source and artifact bytes. MinIO supplies that store in development and in the
self-hosted production option; a managed cloud bucket is the preferred hosted
option. A usable backup must capture both stores and preserve their shared
revision point.

`scripts/backup.sh` implements a quiesced, coordinated, checksummed backup of a
serializable PostgreSQL dump and every object-store version. The v2 manifest
records retention, deletion expiry, exact runtime images, Compose hashes, and
the database collation/checksum invariants; `scripts/prune-backups.sh` refuses
unknown or unverifiable bundles. `scripts/restore-drill.sh` streams the dump
into a resource-bounded isolated Compose project and verifies database/object
inventories, storage invariants, migrations, API health, and operator
onboarding/recovery.

Before either backup mode snapshots data, it stops the API and worker, applies
the current migrations, pins any legacy durable locator to an immutable object
version, and streams every database-referenced object to verify its exact
version, size, and SHA-256. A restore drill performs the same reference check
against the restored stores. The drill writes a passing receipt only after that
check, restored API health, worker startup, and operator recovery all pass.

Use `make production-backup` with the production overlay and an approved
backup root. The schedule, off-host encrypted destination, key custody, and
final RPO/RTO are launch-owner decisions. See `Alpha Launch Runbook.md`.

### Deletion and backup erasure

Deleting an account export is a resumable two-phase operation. PostgreSQL first
commits `deleting` while retaining the exact object locator, the object store is
purged second, and PostgreSQL removes the locator last. A failed or interrupted
request can therefore be retried without reporting a missing object as a
completed deletion.

Account deletion similarly does not treat elapsed time as proof that retained
backups are gone. After canonical data and object versions are purged, the
request remains `awaiting_backup_expiry`. Backup pruning must write a
checksummed prune receipt and prove that the oldest retained verified backup
was created after the canonical purge. Record that watermark only as part of a
successful applying prune:

```bash
BRUNN_RECORD_BACKUP_WATERMARK=true \
  ENV_FILE=/path/to/production.env \
  make backup-prune BACKUP_ROOT=/durable/backups
```

Only then can the worker complete eligible account deletions. The status API
exposes the verification time, retained-backup watermark, receipt SHA-256, and
receipt source. Datadog queue metrics retain `account_export:deleting` and
`account_deletion:awaiting_backup_expiry` as visible states.

### Managed S3 production

Start from `production.managed-s3.env.example`. Set
`BRUNN_OBJECT_STORE_MODE=managed-s3`, an existing private bucket, its
region, and an absolute durable `BRUNN_MANAGED_BACKUP_ROOT`. Keep
`BRUNN_S3_CREATE_BUCKET=false`. Prefer a workload identity with access
limited to that bucket. Static credentials must be mounted as secret files and
selected with both `BRUNN_S3_ACCESS_KEY_FILE` and
`BRUNN_S3_SECRET_KEY_FILE`; direct key values are rejected by the
production validator and never expanded into rendered Compose configuration.
The bucket must have versioning enabled and must permit
listing, reading, writing, and deleting exact object versions. Do not attach a
lifecycle rule that can delete live Brunn State versions.

Validate the hosted shape before deployment:

```bash
make managed-production-config ENV_FILE=/path/to/production.env
make production-images ENV_FILE=/path/to/production.env
make production-deploy ENV_FILE=/path/to/production.env
```

The production deploy and rollback scripts select the managed overlay from
`BRUNN_OBJECT_STORE_MODE`; they do not start or depend on MinIO in this
mode. A pre-deploy backup pauses the API and worker, takes one PostgreSQL
snapshot, and exports every cloud object version and delete marker into the
durable backup root.

Run and verify a coordinated cloud backup with:

```bash
make managed-production-backup ENV_FILE=/path/to/production.env
```

For a restore drill, prepare a separate environment file and empty bucket,
using different database secrets and `BRUNN_RESTORE_DRILL=true`. The drill
restores the database and every object version into an isolated Compose
project, applies the current migrations, pins restored legacy references,
remaps provider-specific version IDs, verifies every remapped database
reference against the restored bytes, checks row counts and storage invariants,
starts the restored API, then removes only the objects it created:

```bash
make managed-production-restore-drill \
  ENV_FILE=/path/to/production.env \
  DRILL_ENV_FILE=/path/to/restore-drill.env \
  BACKUP_DIR=/durable/backups/BACKUP_ID
```

Do not point the drill at the production bucket. A backup is not considered
usable until `verify-managed-backup.sh` passes and a restore drill has passed
against the same backup format.

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
