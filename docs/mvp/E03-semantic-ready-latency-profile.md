# E03 — Semantic-Ready Latency Profile

Status: Specified — not run
Date: 2026-07-27
Gates: none directly — measurement baseline and the primary decision input to E09 (E09-semantic-existence-experiment.md, the semantic existence experiment; the lane policy itself is D11-semantic-lane-policy.md); requires D09(a) (D09-latency-contract-and-gates.md) as a measurement enabler
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

## Arms

Three modes, per the Codex review note:

- **Mode 1 — exact+lexical availability (baseline, exists).** Embeddings pending; reproduces the cited evidence profile on the current build so cross-mode deltas are same-build.
- **Mode 2 — semantic-ready DB path (mock embedder; deterministic, free).** `--wait-semantic` with the mock as the embedding endpoint. Isolates the DB-side cost (HNSW probe, merge, budget) from provider latency: the embed phase is near-zero and deterministic, so mode 2 minus mode 1 ≈ pure semantic-lane DB cost.
- **Mode 3 — production semantic path (real OpenAI embed).** Cache-miss (fresh query strings) and warm (repeated query strings) sub-runs. Note: simple_core.rs:3005 has no application-side cache, so "warm" measures connection reuse and provider-side behavior only; an application query-embedding cache would be a Dxx design change and is out of scope here. Corpus: owner-shaped corpus when available; 64K synthetic until then. OWNER DECISION: whether to wait for the Tier A owner-corpus import on Nyx or run 64K synthetic now and re-run mode 3 after import.

## Corpus and fixtures

Scales 1k/10k/64k (default), 640k via `--future-soak` for modes 1 and 2 (mode 2 embeddings are free, so soak-scale backfill costs nothing but time; mode 3 is not run at 640k). 30 samples definitive; 3 via `--quick` for iteration only. Clean git tree per the implementation fingerprint gate.

## Procedure

MM-DD is the run date.

1. Land build items 1-4; verify `timings_ms` phase-sum sanity (D09 acceptance gate 1) on a `--quick` run.
2. Mode 1: `python performance_eval.py run --label e03-mode1-64k --scales 64000 --samples 30 --out results/2026-MM-DD-e03-mode1-64k.json` (the harness lives at repo root), then `python performance_eval.py run --label e03-mode1-640k --future-soak --out results/2026-MM-DD-e03-mode1-640k.json` for the 640k point.
3. Mode 2: start tests/mock_openai_embeddings.py; point the API's embedding endpoint at it; run with `--wait-semantic` at 64k and `--future-soak`: results/2026-MM-DD-e03-mode2-64k.json, results/2026-MM-DD-e03-mode2-640k.json. Record backfill wall-clock and rate-limit behavior.
4. Semantic-failure probe (within mode 2 config): run with `--semantic-failure-start-command` / `--semantic-failure-stop-command` wired to the mock's hooks; assert graceful degradation — semantic lane times out under RETRIEVAL_LANE_TIMEOUT (~2.5s), response arrives with `semantic_unavailable`, exact+lexical results intact, and open/search totals still inside the hard SLO gates during the outage window.
5. Mode 3: real OpenAI embeddings, 64k, `--wait-semantic`; two sub-runs — cache-miss (unique queries) then warm (repeat queries): results/2026-MM-DD-e03-mode3-cold-64k.json, results/2026-MM-DD-e03-mode3-warm-64k.json.
6. Eval-corpus backfill (build item 6): run the rate-limited backfill over the three quality-suite corpora; record wall-clock, spend, and the zero-`semantic_unavailable` warm-probe verification in results/2026-MM-DD-e03-eval-corpus-backfill.json. This unblocks E09 precondition 1.
7. Report per-phase p50/p95/p99 tables per mode from `timings_ms`; diff against the v8 baselines.

## Metrics

- p50/p95/p99 per phase (embed, exact, lexical, semantic, merge, budget; open phases) per mode and scale.
- Embed attribution: embed-phase share of search total at p50 and p95, mode 3 cold vs warm.
- Semantic DB cost: mode 2 minus mode 1 per phase at 64k and 640k.
- Drift check: mode 1/2 deltas between 64k and 640k (the v8 finding of no latency drift with change-log growth must hold with semantic on).
- Backfill provisioning wall-clock under the rate limit, per scale.
- Probe outcome: degradation behavior and latency during simulated provider failure.

## Acceptance criteria

1. Existing hard SLO gates (open ≤5,000ms, search ≤3,000ms, read ≤1,000ms, checkpoint ≤2,000ms) hold in ALL modes, including mode 3 cold and the failure-probe window.
2. D09 regression-tier gates hold in modes 1 and 2 at 64k and 640k. Mode 3 search is reported against the ≤500ms regression gate but a breach there is a finding for E09 (the embed call is the suspect), not an automatic build failure — that is precisely the decision this experiment feeds.
3. Embed phase fully attributed: mode 3 embed p50/p95/p99 stated as absolute ms and as share of search total.
4. Zero `retrieval_lane_deferred` / `semantic_unavailable` in all `--wait-semantic` sample sets; failure probe passes.
5. Output explicitly labeled as the first semantic-ready profile, superseding the "no semantic-ready profile exists" caveat on all cited baselines.

## Cost preflight and ceiling

Reasoning: **$0.** performance_eval.py drives the simple protocol deterministically; no agent reasoning runs. The subscription rule (all reasoning via ChatGPT-authenticated Codex subscription, fail-closed via `require_codex_subscription`) is unexercised here.

Embeddings (usage-billed OpenAI, explicitly exempt per Decisions.md, listed separately): mode 2 $0 (mock). Mode 3: 64K synthetic corpus backfill — at the observed ~$0.19 per 9.6M-token corpus, a 64K-record synthetic corpus lands in the $0.19-$2 range depending on record length; query embeddings for 2×30 samples are negligible (<$0.01). Eval-corpus backfill for E09 (build item 6): the quality-suite fixture corpora are a fraction of the 9.6M-token reference, ≤$0.50 combined. Preflight estimate: ≤$2.50. Ceiling: **$5 embeddings, $0 reasoning**, hard. Re-running mode 3 on the owner corpus after import adds one more ~$0.19-2 backfill under the same ceiling.

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
- D09-latency-contract-and-gates.md ((a) is prerequisite; (b) gates consumed); E09-semantic-existence-experiment.md (consumer; D11-semantic-lane-policy.md is the policy design); E10-combined-preflight.md (inherits whichever semantic posture E09 picks); Decisions.md (cost rules, embeddings exemption)
