# Brunn Local Operations

## Prerequisites

- Docker Desktop or Docker Engine with Compose and BuildKit
- an OpenAI API key for semantic indexing and automatic capture
- a connected ChatGPT subscription for the optional production Codex Dreamer
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

These `BRUNN_DREAM_*` intervals belong to the older shadow scheduler, which
remains disabled in the production Railway configuration. The separate Codex
Dreamer uses the schedule and authority described below.

## Production Codex Dreamer

The private Railway `dreamer` service runs `brunn dreamer serve`. Its internal
scheduler runs at 03:00 America/Los_Angeles; Railway cron stays unset. Startup
attempts at most the newest due slot, and server admission suppresses duplicate
successful nightly work. Manual attempts can retry the same date. Each accepted
attempt receives a new version of `dreams/runs/YYYY-MM-DD.md`, preserving earlier
successes and failures.

`dreams/CONTROL.md` must explicitly enable the service. Missing, malformed, or
disabled CONTROL produces zero workspace writes. Calendar dates never promote
report-only mode to full. In report-only mode, approvals remain held; enabling
publication explicitly permits validated application of those approvals.
Silence and an elapsed review window never approve a proposal. The Dreams page
supports approve, reject, defer, and correction against the displayed candidate
and state version. Source or target changes require renewed validation; failed
validation leaves the proposal available for review.

Keep these four credentials separate:

| Variable | Authority and custody |
| --- | --- |
| `DREAMER_WORKSPACE_TOKEN` | Wrapper reads CONTROL and existing workspace context. It never reaches Codex. |
| `DREAMER_MODEL_TOKEN` | Dedicated `read_only` Brunn credential. Before model execution, the wrapper checks `/v1/me` for read access and an allowlist of read capabilities; vault, mutation, and credential-management access are rejected. |
| `DREAMER_RUNNER_TOKEN` | Exactly `dreamer:run`, `secret:read`, `secret:write`, and `notification:publish`; only the wrapper holds it. |
| `DREAMER_INTERNAL_TOKEN` | Shared API-to-Dreamer private HTTP authentication; only API and Dreamer receive it. |

The Docker image pins Codex CLI 0.153.4 and defaults to `gpt-5.6-sol`.
`DREAMER_CODEX_MODEL` controls the model; `DREAMER_CODEX_VERSION` checks the
qualified CLI version. ChatGPT authentication lives in the owner-scoped
`dreamer-codex-auth` vault secret. `dreamer-runtime` holds bounded operational
state and any terminal publication awaiting retry. Connect and every execution
exit preserve refreshed authentication using secret identity and version
compare-and-set, followed by readback. A failed custody or receipt write is a
failure, even when Codex exited successfully. Keep the existing encryption key
and authenticated vault contents across deployment; no reconnect is needed.

The wrapper uses `/v1/workspace/dreamer/admit`, `/checkpoint`, `/candidates`, and
`/finish` with the runner credential. Admission freezes exact input references,
versions, hashes, and an upper generation under an owner-scoped lease and fence.
One versioned `dreams/state.md` retains progress and unfinished work. Scanned
generation and processed generation are distinct: partial attempts cannot erase
unprocessed inputs, and concurrent writes remain eligible for the next pass.
Codex receives the frozen input list and returns a bounded `dream.candidates.v1`
JSON file. The server validates sources, citations, permissions, and target
versions before publishing candidate records or approved summaries.

The server atomically publishes the accepted terminal run and the deterministic
`dreams/latest-receipt.md` projection using `dream.latest-receipt.v2`. The latter
contains the exact immutable run reference and version consumed by briefings.
Failed terminal publication remains in runtime custody for retry before any new
model execution. A review-ready notification uses the accepted run version as
its entry target and source. Inbox publication and push delivery have separate
outcomes; failure retains the same event key for retry. No real push belongs in
a fixture run.

For Railway rollout, first pass the local database, runner, MCP, and UI gates
and record the existing deployment IDs and digests. Stage a dedicated model
token with `railway variable set DREAMER_MODEL_TOKEN --stdin --skip-deploys
--service dreamer --environment production`; `.railway/railway.ts` preserves
the value. Never place a token in command arguments or shared output. Apply the
additive migrations through the worker's existing migrate pre-deploy step, then
verify the API and runner credential authority before deploying Dreamer. The
existing runner needs an explicit, audited `dreamer:run` grant, or replacement
with the updated bounded runner template. Migrations do not upgrade existing
credentials automatically. Deploy matching API, MCP, and web contracts before
the new runner can publish review notifications.

`BRUNN_DREAMER_SUMMARY_READS_ENABLED` independently gates summary-first reader
responses and defaults to false. Railway configuration preserves its deployed
value. Enable it only after the accepted-summary canary proves exact source
validation and source links; set it false to withdraw summaries from readers
without discarding review decisions or changing Dreamer publication mode.

Imported prose and questions from untyped historical runs appear in Review's
History through `legacy_items`, with `legacy: true` and `counts.legacy`.
They retain their original IDs, hashes, statuses, and exact source versions,
but have no candidate preview or new decision actions. Existing identical
decision requests still replay. Current questions remain in `items`.
State loading separates old imports in memory; the next normal write persists
the split without advancing source work or inventing decisions. History is
bounded at 96 notes independently of the 96 active-item slots and does not enter
runner admission, pending counts, new run bodies, or receipts. Original run
versions retain full text; protected reads still validate their visibility.

Historical location candidates have a primary timeline of at most 250 words:
chronological places and approximate times, preserving meaningful brief stops
and material uncertainty. The runner independently audits the complete frozen
packet and retains all canonical selectors in evidence metadata. Exact clocks,
source versions, raw selections, fingerprints, and publication locks remain
enforced; concise wording does not establish continuous presence or venues.
Review and publication omit citation-marker noise and audit appendices from the
primary body. The immutable candidate and protected summary manifest retain
claim-to-source mappings. Default location-summary reads return compact evidence
pointers instead of injecting that manifest into the response. Detailed evidence
is an explicit follow-up read; expired or changed evidence still causes fallback.
When revising a consumed historical pilot, explicitly requeue its closed day
before triggering a manual run; preserve the pending item identity and mode.

Each admitted run also queues the previous closed Pacific day when it has raw
observations and no retained or already dispositioned work for that day.
Location reasoning separates autonomous discovery, drafting, and an independent
audit (with one bounded correction when needed). Discovery has an initial pass
and one fenced follow-up when it finds leads. The follow-up can investigate
aliases from pre-day Brunn context and replace web pages that failed independent
verification. Both discovery requests have stable replay identities; a third
request cannot broaden the evidence. The final admission still has at most
eight web sources, eight context queries, four historical excerpts and 12 KiB
of excerpt text. Neither discovery pass receives a previous daily answer or
owner correction. Historical excerpts stay private; web searches use public
place identities and away observations only.

Nightly and manual runs have a shared one-hour ceiling. Discovery receives at
most twenty minutes across its two passes; the independent audit and its one
correction share the remaining location reasoning deadline. Time reserved before
drafting is a minimum reservation, not a cap that discards time an early draft
leaves unused. Stage setup and retries cannot reset the overall deadline. The
narrative pass and finalization retain their separate reserves.
The primary timeline normally uses one first-to-last approximate observation
window per place, with material gaps stated once rather than a list of every
sampling segment. This does not assert continuous presence.
Discovery sees raw observations with known Home coordinates/addresses removed;
it can search the public web, but has no shell or Brunn tools. The wrapper
independently fetches at most eight public HTTPS pages and verifies short exact
quotations. The fenced `/workspace/dreamer/location-discover` operation retains
those excerpts under `Evidence/Location/<date>/` and searches up to eight
discovered aliases against at most four historical Brunn source excerpts.
Historical version selection and generated/evaluation-output exclusions happen
before matching. The source cutoff is the beginning of the day being summarized,
so later answers and corrections cannot become a replay's answer key.
Public PDF lookups use `poppler-utils` in the Dreamer image to verify quotations
against extracted text from at most the first twenty pages. Downloads remain
bounded to 2 MiB; conversion is serialized, has an eight-second wall deadline,
one-MiB text limit and Linux CPU/address-space limits, and inherits no account
credentials. The stored receipt records the PDF response hash and extraction
method. Image-only, malformed, inaccessible or nonmatching sources are withheld.
Nyx tests can set `BRUNN_PDFTOTEXT` to the bundled converter's absolute path.

Before submission, the correction gate attaches the exact frozen source excerpts
in memory and runs the shared candidate contract, including hydrated byte limits,
inline citations and allowed raw fields. The API repeats validation under current
evidence locks. Rejected submissions retain the bounded public API error in the
attempt outcome so source conflicts and contract failures remain diagnosable.
The wrapper attaches the frozen context manifest after auditing; the model does
not recopy it. A supplied manifest that differs from admission is rejected.

Client-rendered public pages may carry a quoted description in an inert
`application/json` or `application/ld+json` script. The verifier can match an exact
quote within one decoded JSON string and records `inert_json_string` extraction.
It never executes scripts or joins unrelated JSON values into a quotation.
Replay saved autonomous discovery outputs against the real fetcher on Nyx with
`cargo run --manifest-path apps/api/Cargo.toml --example location-source-replay -- <discovery.json> ...`.
This performs public reads only and never admits sources or changes a candidate.
Use the `location-candidate-replay` example with a matching admission packet and
candidate output for an offline publication preflight. The fixed legend
“Times are approximate observation windows.” is presentation text, like table
headings; specific factual and uncertainty statements still require citations.
Place aliases match complete words/phrases, never substrings inside opaque
strings. Credential documents are excluded before matching and from narrative
admission; excerpts containing token-like strings are withheld. Narrative reads
receive a server-issued session correlation reference with their frozen input.

Drafting and auditing have no tools and receive the unchanged frozen location
packet plus the admitted source versions. They receive no earlier candidate
body, owner decision prose, or narrative backlog. Context identity evidence
cannot substitute for observed position/time evidence. Lookup receipts include
URL, fetch time, exact quotation and response hash; source changes, revocation
or the 30-day lookup freshness deadline invalidate dependent summaries. Raw
retention remains unchanged. Manual queueing accepts a date/timezone, never a
hand-selected context list or itinerary.

The same overall run budget reserves time for ordinary memory summaries in a
separate subsequent pass. Location results are accepted first and survive a
failed narrative pass. Both submissions share one durable review notification;
unprocessed work remains pending. `DREAMER_REASONING_EFFORT` can explicitly set
the supported effort for the configured ChatGPT-backed Codex model. It does not
enable API-key billing. To switch the Dreamer account, wait for the terminal run
receipt, disconnect in Brunn Settings, then use Connect with the new account's
device-code login. Host Codex authentication and personal scheduled tasks are
independent of this stored Dreamer credential.

Deploy only the tested committed `main` tree with explicit project, service,
and environment arguments. A detached upload is not a health result: record its
deployment ID, wait for success, check private `/healthz`, the CLI version,
credential capabilities, and a coherent attempt/receipt. Startup catch-up may
start a run immediately when CONTROL is enabled, so credential provisioning and
publication gates must be ready first. An explicitly authorized report-only
canary must prove exact source-to-candidate-to-receipt lineage and retained work;
full-mode publication remains an independent owner choice. If rollback is
needed, disable Dreamer admission before reverting its image and dependent
services to the recorded deployments. Preserve vault secrets, run versions,
state, and additive schema; an older image must not regain calendar-based full
mode while rollback is being inspected.

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
- The API sends submitted source plus bounded current context to OpenAI for
  `memory.capture`, using `store: false` and a privacy-preserving user-scoped
  safety identifier. The production Codex Dreamer uses the connected ChatGPT
  subscription and a separate read-only Brunn identity; its authentication and
  storage behavior are governed by that Codex runtime.

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
