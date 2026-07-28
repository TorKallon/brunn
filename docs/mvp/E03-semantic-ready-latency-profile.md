# E03 — Semantic-Ready Latency Profile

Status: Harness ready — not run
Date: 2026-07-27
Gates: `--gate-profile e03-semantic-ready` with explicit `--e03-arm`; measurement baseline and the primary decision input to E09
Phase: 0 (measurement; no product code — D09(a) instrumentation is a measurement enabler, not a behavior change)

## Question

Every latency number we cite — the entire v8 640K soak (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) and the clean 3,340-record fixture (results/2026-07-27-3340-clean-30-sample.json) — is exact+lexical with embeddings pending. NO semantic-ready profile exists. Once any semantic coverage exists, every search pays a synchronous, uncached OpenAI embedding call (apps/api/src/simple_core.rs:3005) plus an HNSW probe (`iterative_scan=relaxed_order`). What do open and search look like then, phase by phase, and do the SLO gates hold?

## Preconditions and build items

1. **D09(a) `timings_ms` decomposition** (Small; apps/api/src/simple_core.rs) — per-phase attribution (`embed`, `exact`, `lexical`, `semantic`, `merge`, `budget`; open phases likewise). Build first; without it this experiment can only report totals.
2. **`--wait-semantic` provisioning flag** (Small; performance_eval.py) — sets `wait_for_semantic=True` for the simple protocol: provisioning blocks until embedding backfill completes before sampling begins. Backfill must go through the existing rate limiter, not around it; provisioning wall-clock is recorded.
3. **Inverted pending-retrieval gate** (Small; performance_eval.py) — today the harness tolerates the pending state (`semantic_unavailable` notice, deferred lanes). Under `--wait-semantic` the assertion inverts: zero `retrieval_lane_deferred` events and zero `semantic_unavailable` notices across all samples, otherwise the run is invalid (we would be re-measuring the baseline by accident).
4. **Semantic-lane gate thresholds** (Small; performance_eval.py gate config) — the semantic phase gets its own reported percentile row so it can be gated separately from lexical.
5. **Mock embedder** — exists: tests/mock_openai_embeddings.py, a deterministic OpenAI-compatible server, already usable as the semantic-failure probe hooks.
6. **Eval-corpus backfill for E09** (Small; scope addition) — embed the quality-suite eval corpora (the fixtures behind eval/work_cases.json, eval/rupture_ops_cases.json, eval/recent_work_cases.json) through the same rate-limited backfill path, verified by zero `semantic_unavailable` notices on a warm probe query. This is the deliverable E09's precondition 1 depends on; without it no E03 mode touches the quality-suite corpus (modes 1-3 cover only synthetic/owner performance fixtures). Spend is inside the embeddings ceiling below.

Implementation note (2026-07-27): all six build items now have deterministic
harness support. All three arms run the same current product/harness build with
`verbatim_spans=false`; exact source, API image, worker image (where present),
DB image, runtime build revision, container IDs/start times, and one isolated
Compose project are bound before and after each run. The ambient API URL must
equal the named API container's single loopback-published port, and that API's
read/write and read-only database URLs must resolve to the named DB on their
sole Compose network. `eval/e03_mode1.py` proves a
hashing-provider, semantic-disabled, no-worker baseline whose chunks stay
zero-embedded and pending. Its 64K plan gate keeps both authoritative SQL
fingerprints and the lexical GIN plan blocking; the HNSW plan is explicitly
not applicable until a semantic-ready arm has nonzero vector cardinality, and
remains blocking in Modes 2 and 3. `eval/e03_mode2.py` owns a run-unique mock lifecycle
and proves API and worker point to that exact mock with dummy credentials
before import. `eval/e03_mode3.py` owns the real-provider proxy and captures
cold then warm queries after one 64K import; a second provisioning run is
forbidden because it would change HNSW/table cardinality. `eval/e03_quality_backfill.py` estimates before mutation,
imports through the ordinary rate-limited worker path, verifies that the
cross-process foreground guard is configured, and requires a semantic-only
warm probe with candidates and no semantic gap. These harnesses are unit
tested; no definitive experiment has been run.

Current-build execution hardening adds
`eval/openai_embedding_fault_proxy.py` for real-provider modes. The proxy is
run-unique and forwards only `/v1/embeddings`; it never logs or persists bearer
headers, request bodies, or response bodies. Loopback control is allowed
locally, while non-loopback control requires a bearer token loaded from an
owner-only file. Every configure command binds the proxy instance ID, checked-in
implementation SHA-256, and upstream-base-URL SHA-256. Definitive real-provider
performance runs add `--require-semantic-failure-hook-attestation`, so both the
injected-error hook and restored-forward hook must return matching,
secret-free attestations. Mode 3 additionally requires the exact official
`https://api.openai.com/v1` upstream, proves the owned proxy identity from both
the API and worker network namespaces using a locally present digest-pinned
helper image with pull disabled, and records OpenAI `usage.prompt_tokens`
actuals without retaining response bodies. Bound teardown runs even if proxy
start times out.

## Arms

Three modes, per the Codex review note:

- **Mode 1 — exact+lexical availability.** The API uses the explicit hashing
  provider only so it can start without an external credential; semantic
  retrieval is disabled and no worker runs, so the hashing provider is never
  invoked. Every imported chunk remains pending with zero embeddings.
- **Mode 2 — semantic-ready DB path (mock embedder; deterministic, free).** `--wait-semantic` with the mock as the embedding endpoint. Isolates the DB-side cost (HNSW probe, merge, budget) from provider latency: the embed phase is near-zero and deterministic, so mode 2 minus mode 1 ≈ pure semantic-lane DB cost.
- **Mode 3 — production semantic path (real OpenAI embed).** One 64K import is
  followed by 30 unique cold queries and the identical 30 warm queries in the
  same session, corpus, API process, worker process, and proxy instance.
  Cardinality is checked before, between, and after the phases. Because
  `embed_cache=false`, each phase must record exactly 30 provider requests,
  successes, and cache bypasses, with zero cache hits/misses, failures, or
  deferrals. “Warm” therefore measures connection/provider behavior only.

## Corpus and fixtures

Modes 1 and 2 use 64K and may use the explicitly separate 640K
`--future-soak`. Mode 3 is exactly one 64K import and rejects default
1K/10K scales, 640K, `--unique-queries`, or a second warm invocation.
Thirty samples are definitive. The explicit E03 profile accepts exactly 30
samples and rejects quick mode so
the fixed cost bound and cold/warm pairing cannot be silently expanded.

## Procedure

MM-DD is the run date.

1. Use separate project-scoped stacks and exact container IDs from
   [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
   Build one clean revision/image pair and use the same full source SHA, exact
   API image ID, and exact DB image ID for all arms. Modes 2 and 3 require the
   worker image ID to equal the API image ID.
2. Run Mode 1 with no worker and
   `STRAYLIGHT_EMBEDDING_PROVIDER=hashing`:
   `python3 eval/e03_mode1.py --label e03-mode1-64k --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-api-image-id "$API_IMAGE_ID" --expect-db-image-id "$DB_IMAGE_ID" --scales 64000 --samples 30 --out results/2026-MM-DD-e03-mode1-64k.json`.
3. Configure Mode 2 API and worker with the exact run-unique local mock `/v1`
   URL and a `mock-`, `dummy-`, or `test-` inline key, then run:
   `python3 eval/e03_mode2.py --label e03-mode2-64k --mock-port "$MOCK_PORT" --mock-state "$MOCK_STATE" --mock-log "$MOCK_LOG" --mock-config "$MOCK_CONFIG" --expected-openai-base-url "$MOCK_BASE_URL" --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --worker-container "$WORKER_CONTAINER" --expect-build-revision "$REV" --expect-api-image-id "$API_IMAGE_ID" --expect-db-image-id "$DB_IMAGE_ID" --scales 64000 --samples 30 --out results/2026-MM-DD-e03-mode2-64k.json`.
4. Semantic-failure probe (within mode 2 config): the mode-2 wrapper wires the
   mock's injected-503 configure command and distinct fast-state restore
   command into `--semantic-failure-start-command` /
   `--semantic-failure-stop-command`. In addition, run
   `eval/semantic_http_probe.py --run-id <unique-run>` with slow/restore proxy
   configure hooks. The probe now provisions its own unique one-document,
   read/write evaluation user and nonce-bearing marker, waits for semantic
   readiness, and always calls the atomic evaluation cleanup endpoint in a
   `finally` path. Passing the probe requires both source cleanup and proof
   that the scoped credential was revoked; neither token nor fixture source
   body is written to the result artifact.
   Require the cold full HTTP response to retain exact+lexical evidence and
   defer semantic before provider delay, the identical query to succeed from
   the asynchronously warmed cache, and a new semantic query to succeed after
   restore.
5. Configure Mode 3 API and worker to the exact run-unique proxy `/v1` URL
   using the numeric gateway of their sole Docker network,
   then let the wrapper own start, attested error/restore controls, paired
   sampling, and bound teardown:
   `python3 eval/e03_mode3.py --label e03-mode3-paired-64k --proxy-port "$PROXY_PORT" --proxy-state "$PROXY_STATE" --proxy-config "$PROXY_CONFIG" --proxy-log "$PROXY_LOG" --proxy-instance-id "$PROXY_INSTANCE" --expected-proxy-base-url "$PROXY_BASE_URL" --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --worker-container "$WORKER_CONTAINER" --expect-build-revision "$REV" --expect-api-image-id "$API_IMAGE_ID" --expect-db-image-id "$DB_IMAGE_ID" --samples 30 --out results/2026-MM-DD-e03-mode3-paired-64k.json`.
   Do not run separate cold and warm imports.
6. Eval-corpus backfill (build item 6): first run
   `python3 eval/e03_quality_backfill.py --provider-mode openai estimate
   --out results/2026-MM-DD-e03-eval-corpus-backfill-estimate.json`. Only after
   the estimate passes the $5 ceiling, run the same harness's `run` subcommand
   with a unique run ID. It imports all three quality corpora through the
   ordinary guarded worker path and records wall-clock, machine-readable
   preflight/actual spend accounting, and a zero-gap semantic-only warm probe
   in `results/2026-MM-DD-e03-eval-corpus-backfill.json`. This rehearses and
   prices the shared backfill path; it is not persistent coverage proof for
   E09, whose quality harness provisions isolated per-case users and must
   independently wait for and verify semantic coverage.
   After retrieving the provider receipt, hash the immutable run artifact and
   invoke:

   `python3 eval/e03_quality_backfill.py reconcile-receipt --input <run.json> --input-sha256 <sha256> --run-id <exact-run-id> --provider-receipt <receipt-file> --billed-input-tokens <tokens> --billed-usd <usd> --out <new-reconciliation.json>`

   The command refuses overwrite, run-ID or hash mismatch, already-reconciled
   input, inconsistent token/USD pricing, and spend above the original
   ceiling. It records only the receipt hash and size, never the receipt
   contents, and does not mutate the original run.
7. Report per-phase p50/p95/p99 tables per mode from `timings_ms`; diff against the v8 baselines.

## Metrics

- p50/p95/p99 per phase (embed, exact, lexical, semantic, merge, budget; open phases) per mode and scale.
- Embed attribution: embed-phase share of search total at p50/p95/p99, mode 3 cold vs warm.
- Semantic DB cost: mode 2 minus mode 1 per phase at 64k and 640k.
- Drift check: mode 1/2 deltas between 64k and 640k (the v8 finding of no latency drift with change-log growth must hold with semantic on).
- Backfill provisioning wall-clock under the rate limit, per scale.
- Probe outcome: degradation behavior and latency during simulated provider failure.

## Acceptance criteria

1. Existing hard SLO gates (open ≤5,000ms, search ≤3,000ms, read ≤1,000ms, checkpoint ≤2,000ms) hold in ALL modes, including mode 3 cold and the failure-probe window.
2. D09 regression-tier gates hold in modes 1 and 2 at 64k and 640k. Mode 3 search is reported against the ≤500ms regression gate but a breach there is a finding for E09 (the embed call is the suspect), not an automatic build failure — that is precisely the decision this experiment feeds.
3. Embed phase fully attributed: mode 3 embed p50/p95/p99 stated as absolute ms and as share of search total.
4. Zero `retrieval_lane_deferred` / `semantic_unavailable` in all `--wait-semantic` sample sets; failure probe passes.
   The failure probe must return the planted target from a semantic-only query
   both before injection and after restore; exact/lexical mixed-mode evidence
   is tracked separately and cannot mask an empty semantic lane.
5. Output explicitly labeled as the first semantic-ready profile, superseding the "no semantic-ready profile exists" caveat on all cited baselines.
6. The D02 30-probe measurement is complete and internally exact in every arm:
   planted manifest, returned rows, identifiers, paths, byte offsets,
   exact-only modes, typed booleans, counted outcome, and reported result all
   agree. Because `verbatim_spans=false` is frozen across E03, the raw feature
   outcome (including the known 0/30 result) is a nonblocking finding named
   `verbatim_identifier_feature_acceptance`; measurement incompleteness or
   corruption remains blocking. The default non-E03
   `verbatim_identifier` feature-acceptance gate remains blocking.

## Cost preflight and ceiling

Reasoning API billing: **$0.** All planning, review, and interpretation use the
owner's ChatGPT-authenticated Codex subscription. The deterministic harness
does not invoke a reasoning model or inherit API credentials for reasoning.

Embeddings (usage-billed OpenAI, explicitly exempt per Decisions.md, listed
separately): Modes 1 and 2 cost $0 (disabled hashing path and owned mock).
Mode 3 includes one 64K backfill plus the paired 60 query embeddings, ordinary
samples, and failure/restore probes. The artifact uses a conservative query
allowance and records per-scale and aggregate estimates, then reconciles those
estimates against the proxy's aggregate provider token-usage receipt. Every
successful provider response must report usage; missing usage invalidates the
run. Preflight maximum:
**$2.50**. Ceiling: **$5 embeddings, $0 reasoning API billing**, hard. This
stricter ceiling is below the user's $20 notification threshold, so no >$20
warning is expected; crossing $5 aborts before further mutation.

## Abort criteria

- Backfill stalls >12h wall-clock at 64k under the rate limit: abort, file the rate-limit finding, do not lift the limit to make the eval pass.
- Any `--wait-semantic` run emits a deferred/unavailable notice: invalidate the sample set, fix provisioning, rerun.
- Mock server nondeterminism detected (differing vectors for identical input): abort mode 2, fix tests/mock_openai_embeddings.py.
- Any hard SLO gate breach >2× in any mode: stop sampling, file a defect with the phase table — that is a product bug, not an eval result to average away.
- Embeddings spend crosses $5.

## Reporting

The run record must contain: git SHA (clean tree); all six-plus artifact paths named above; per-mode per-scale phase tables (p50/p95/p99); embed-attribution table (cold/warm); mode 2 − mode 1 semantic DB cost; 64k→640k drift statement; backfill wall-clock and rate-limit observations; failure-probe transcript; embeddings spend actuals vs preflight; and a one-paragraph E09 decision input stating whether the synchronous uncached embed call (simple_core.rs:3005) is compatible with the D09 regression tier or needs redesign.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; results/2026-07-27-3340-clean-30-sample.json (both exact+lexical only — the gap this closes)
- apps/api/src/simple_core.rs:3005 (synchronous uncached query embed); RETRIEVAL_LANE_TIMEOUT ~2.5s
- tests/mock_openai_embeddings.py (deterministic mock + failure hooks)
- eval/openai_embedding_fault_proxy.py (real-provider forwarding and
  fingerprint-bound slow/error/restore controls)
- D09-latency-contract-and-gates.md ((a) is prerequisite; (b) gates consumed); E09-semantic-existence-experiment.md (consumer; D11-semantic-lane-policy.md is the policy design); E10-combined-preflight.md (inherits whichever semantic posture E09 picks); Decisions.md (cost rules, embeddings exemption)
