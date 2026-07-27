# Straylight

Straylight is an agent-first workspace and memory layer. It gives agents a
source-preserving place to learn, resume complex work, inspect artifacts,
reason across changing state, and leave useful durable checkpoints. The core
product is a multi-user online workspace with exact entry versions and a cheap
change cursor, not a globally snapshot-isolated database.

The product stack is Rust, PostgreSQL with pgvector, versioned S3-compatible
object storage, OpenAI text embeddings, a TypeScript SPA, and a typed
TypeScript MCP server. Every component runs in Docker. Python and SQLite remain
only in the frozen evaluation harness and do not define product behavior.

## Start the local alpha

```bash
cd /Users/Shared/projects/straylight
cp .env.example .env
# Replace every placeholder in .env, then keep it private.
chmod 600 .env
make config
make up
```

Health and UI:

```bash
curl -fsS http://127.0.0.1:18110/health
curl -fsS http://127.0.0.1:18110/ready
open http://nyx:13110/
```

The API, Postgres, and MinIO stay localhost-only. The SPA is the deliberately
tailnet-accessible human surface.

## Repository map

- `apps/api`: Rust workspace API and worker, binary transfer, import/export,
  usage telemetry, checkpoints, background indexing and dreaming, plus retained
  owner-alpha compatibility code
- `apps/web`: TypeScript/React audit and control SPA
- `apps/mcp`: typed stdio MCP adapter
- `infra`: Postgres role initialization and pinned MinIO build/policy
- `docs/Architecture.md`: system design and trust boundaries
- `docs/Specification.md`: alpha behavior and acceptance contract
- `docs/Alpha Launch Runbook.md`: production candidate, deployment, recovery,
  backup, rollback, and incident procedure
- `docs/Alpha Readiness.md`: release-gate evidence and owner decision register
- `docs/Object Store Evaluation.md`: required S3 contract and provider evidence
- `docs/Operations.md`: local operation, verification, and evaluation
- `eval`: frozen main, Rupture Ops, personal coordination, and transition suites
- `tests`: deterministic harness tests and destructive live API smoke

## Verify

```bash
python3 -m unittest discover -s tests -v
python3 tests/live_api_smoke.py --base-url http://127.0.0.1:18110 --env-file .env

(cd apps/web && npm run build && npm test -- --run)
(cd apps/mcp && npm run build && npm test)
```

See `docs/Operations.md` for migrations, MCP, comparative evaluation, storage
notes, and the opt-in Datadog Agent profile and production dashboard. The
launch runbook is intentionally stricter than the local-development path.

## Evaluations

The harnesses test whether a fresh agent can capture ordinary source material,
receive safe fresh learned context, recover canonical knowledge, resume complex
work, inspect artifacts, reason across superseded state, revise plans, verify
claims, and leave a useful durable checkpoint.

### 2026-07-27 simplified workspace

The latest strict 57-case draw recovered 160/228 claims through the simplified
service, 170/228 through the legacy service, and 171/228 through direct
Markdown. A matched repeat of all 16 ordinary cases where legacy initially won
narrowed the legacy-to-simplified gap to 46/64 versus 45/64 claims. The
simplified response contained an accepted source for 21 of the 22 disputed
claims, and all five changed-evidence continuations preserved exact parent,
revision, prior-source, delta-source, and checkpoint lineage. This does not
prove perfect parity, but it found no material retrieval-driven degradation.
RuptureOps context overfetch remains the leading quality risk.

Mean uncached model input was 24,260 tokens for simplified Straylight, 24,092
for legacy Straylight, and 25,168 for direct Markdown. The simplified path is
effectively tied with legacy and 3.6% below files.

At the accumulated 3,340-entry production reproduction, the legacy service
failed with HTTP 408 after 26.088 seconds before useful retrieval. The
simplified service completed open in 1.047 seconds, targeted search in 0.674
seconds, and broad search in 1.867 seconds. Direct-file discovery took
0.119-0.124 seconds, so the simplified service is practical but still has
lookup overhead to reduce.

See `results/2026-07-27-simplification-final-evaluation.md`.

### 2026-07-23 alpha hardening candidate

The complete post-hardening run recovered 179/180 deterministic claims through
Straylight versus 175/180 through direct files. Across 45 paired cases,
Straylight used 0.7% more cumulative input, 11.8% less uncached input, and
15.1% fewer agent tool calls. The single strict miss contained every required
fact and source but placed two facts in adjacent claim slots.

Deterministic, destructive live API, read-only, account lifecycle, dependency
failure, database, object-store behavior, backup/restore, current/N-1 rollback,
browser, and real GPT-5.6 shadow-dream gates pass. Public alpha remains blocked
on owner decisions and exact production-provider qualification; Community
MinIO is not an acceptable production store because its image retains critical
and high vulnerabilities.

See `results/2026-07-23-alpha-candidate-comparison.md` and
`docs/Alpha Readiness.md` for the evidence and remaining decision boundary.

### 2026-07-22 token-efficiency and source-hydration evaluation

The final implementation keeps the canonical write and dreaming contracts
unchanged. It gives `memory.open` a bounded, source-complete reasoning packet,
uses a shared source-text budget instead of a per-file cutoff, keeps overflow
sources as exact pointers, reports retrieval sufficiency, compacts agent-only
transport, and records source-text, metadata, and replay-weighted character
mix. The full HTTP representation remains the audit view.

The pre-change state is commit `10787b1`. Its API and web images, exact API
executable, deployed web bundle, MCP distribution, Compose file, checksums, and
image archive are preserved under
`/Users/Shared/projects/straylight-baselines/20260722T222352-0700-10787b1`.

| Suite | Filesystem | Native Straylight |
| --- | ---: | ---: |
| Main active work | 13/13 cases, 52/52 claims | **13/13, 52/52** |
| Personal coordination | 14/15, 59/60 | **15/15, 60/60** |
| Rupture Ops | 12/12, 48/48 | **12/12, 48/48** |
| Changed-evidence transitions | 5/5, 20/20 | **5/5, 20/20** |

Across the 40 ordinary cards, final Straylight passes 40/40 cases and 160/160
claims. Regraded with the same matcher, the saved pre-change service run passed
38/40 and 158/160. Average API calls fell from 3.53 to 2.72 (-23%),
model-visible service output from 47,177
to 32,623 characters (-31%), cumulative input from 138,414 to 115,319 tokens
(-17%), and uncached input from 31,137 to 24,491 (-21%). These are separate
fresh-agent runs, so they establish the aggregate outcome rather than assigning
every individual movement to one retrieval change.

Against the fresh filesystem run, final Straylight uses 10.7% more cumulative
input because tool-result history is replayed into later cached turns, but
12.3% less uncached input. Its model-visible service output is 79.5% exact
source text and 20.5% metadata. The filesystem personal miss is a mechanically
strict shorthand miss in an otherwise correct answer; it is retained rather
than hand-scored away.

The changed-evidence transition suite is perfect in both conditions.
Checkpoint resume uses 22.7% less cumulative input, 10.4% less uncached input,
and 26.3% fewer calls than filesystem reconstruction while committing all five
parent-, revision-, and source-linked child checkpoints.

The deterministic regrade accepts `remove`/`exclude` as the same boundary
operation and no longer treats an explicitly negated forbidden conclusion as
asserted. Answers were not regenerated, citation requirements and thresholds
were not relaxed, and both conditions use the same matcher.

Current result files:

- `results/2026-07-22-efficiency-final-main.json`
- `results/2026-07-22-efficiency-final-personal.json`
- `results/2026-07-22-efficiency-final-rupture.json`
- `results/2026-07-22-efficiency-final-transitions.json`

## Checkpoint transition evaluation

The v0.3 transition suite is the current decisive gate. It starts from a persisted revision-N checkpoint, adds one revision-N+1 evidence or constraint delta, and asks a genuinely fresh agent to advance the work and commit a source-preserving child checkpoint.

Five cards cover Warmind, Charlemagne, Star Rupture, Switzerland, and Straylight. The comparison is:

1. **Filesystem rebuild:** prior checkpoint, full frozen corpus, and delta through ordinary files.
2. **Straylight checkpoint resume:** snapshot-pinned `open`, optional batched hybrid retrieval/read, and an immutable child checkpoint.

```bash
python3 transition_eval.py validate
python3 transition_eval.py run \
  --filesystem-native \
  --concurrency 2 \
  --timeout 420 \
  --out results/2026-07-22-efficiency-final-transitions.json \
  --report results/2026-07-22-efficiency-final-transitions.md
```

Current result:

- Both conditions pass 5/5 cards and 20/20 claims.
- Native Straylight commits 5/5 correctly linked, source-preserving child
  checkpoints.
- Native resume averages 75,013 cumulative input tokens versus 97,105 for
  filesystem reconstruction, 20,126 uncached tokens versus 22,456, and 2.8
  service calls versus 3.8 filesystem calls.
- Complete bounded N-to-N+1 source deltas remain on the continuation-critical
  path without requiring semantic re-indexing before the fresh agent can resume.

Primary v0.3 files:

- `transition_eval.py`
- `native_memory.py`
- `eval/transition_cases.json`
- `eval/transition-deltas`
- `apps/api/src/read_service.rs`
- `results/2026-07-22-efficiency-final-transitions.json`

## Agent-work evaluation

The primary v0.2 suite compares three access surfaces:

1. **Fixed handoff pack:** one task-specific context file with no follow-up access.
2. **Filesystem agent:** the frozen corpus with ordinary search, file reads, and scripts.
3. **Straylight workspace agent:** `open`, `query`, exact `read`, ordinary
   `write`/`capture`, and persistent `checkpoint` operations. Computation and
   verification use the agent's native tools after source retrieval.

The frozen corpus contains 73 notes and text artifacts from Warmind, Charlemagne, Star Rupture, Switzerland trip planning, Straylight, Metis, N24 RaceWatch, and Home Network Improvements. The 14 tasks score 56 claims across continuation, learning, supersession, incident work, quantitative planning, source authority, constraint changes, artifact safety, and trust-aware handoffs.

```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 agent_work_eval.py validate
python3 agent_work_eval.py run \
  --filesystem-native \
  --concurrency 3 \
  --timeout 420 \
  --out results/2026-07-12-native-main-final.json \
  --report "/Users/aether/obsidian/notes/Projects/Straylight/Native agent work evaluation results - 2026-07-12.md"
```

### Result

- Filesystem: 12/14 cases and 55/56 claims.
- Native Straylight: 14/14 cases, 56/56 claims, and 14/14 persisted
  checkpoints.
- Native used 38.3K mean uncached input versus 34.1K for filesystem access,
  while returning 128.7K recorded command characters versus 348.5K.
- The native service is ahead on answer quality and durable continuation; broad
  context efficiency and tail latency remain optimization targets.

Primary files:

- `agent_work_eval.py`
- `native_memory.py`
- `eval/work_cases.json`
- `eval/work_answer_schema.json`
- `eval/corpus-v0.2`
- `results/2026-07-12-native-main-final.json`

## Rupture Ops workload suite

The separate `rupture-ops-v0.1` suite expands the agent-work harness around the
actual RuptureOps and StarRupture interaction history. It preserves the v0.2
baseline while adding 12 cards and 48 claims.

This is a concrete fixture for the generalized **subject-plus-product** work
model: learn a subject, track one changing instance, maintain goals and
decisions, build durable artifacts or applications, and verify the resulting
work. Game-specific entities remain fixture data rather than core Straylight
types. The cards cover:

- lossless archive import, deduplication, and selective promotion;
- overlapping prompt history and epistemic-state classification;
- current, historical, selected, planned, unknown, and superseded save state;
- live POI advice and compact field output;
- direct player feedback becoming durable operational learning;
- map assets, named geography, and coordinate-frame safety;
- quantitative campaign revision with buffers and source-pinned arithmetic;
- multi-goal session continuity;
- research-to-product design and an interrupted native-code continuation;
- private/public artifact policy and forked-agent idempotency.

The frozen corpus contains 65 indexed Markdown, structured-data, and source-code
artifacts plus retained binary map, icon, and audio assets for a future
multimodal lane. The current scored cards do not require binary interpretation.

```bash
cd /Users/Shared/projects/straylight
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json validate
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json run \
  --filesystem-native \
  --concurrency 3 \
  --timeout 420 \
  --out results/2026-07-12-native-rupture-regraded-v1.json \
  --report "/Users/aether/obsidian/notes/Projects/Straylight/Native Rupture Ops evaluation results - 2026-07-12.md"
```

Result: filesystem passed 6/12 cases and 41/48 claims; native Straylight
passed 11/12 and 47/48 and persisted 12/12 checkpoints.

Primary files:

- `eval/rupture_ops_cases.json`
- `eval/corpus-rupture-ops-v0.1`
- `Projects/Straylight/Rupture Ops interaction patterns and architecture validation - 2026-07-11.md` in the vault

## Personal coordination suite

The frozen `personal-coordination-v0.1` suite is a separate, wholly synthetic
fixture for generalized personal coordination. Its 15 cards and 60 claims use
a small domain-neutral kernel: stable typed profiles, source-bearing claims,
qualified relations, temporal revisions, and resumable checkpoints. Person,
organization, group, place, event, arrangement, resource, work-item, and
artifact all use the same profile envelope.

The cards cover identity ambiguity, reviewed and reversible equivalence,
derived person dossiers, role provenance, canonical contract normalization,
recurring-event exceptions, schedule supersession, independent participation
state, readiness gates, handoffs, independent booking/payment/allocation/
availability/use state, vacation and game continuity, weekly change-impact
briefs, read-only authorization, and auditable minor-safe redacted exports.
Organizer and guardian authority remain fact-scoped. Names, contacts, places,
and evidence are synthetic; contacts use reserved example domains.

The corpus contains 29 compact indexed Markdown, JSON, and CSV artifacts and
36 chunks. Each rubric cites one or more frozen source paths, and deterministic
tests verify source satisfiability, canonical contract shape, redaction receipt
completeness, read-only denial behavior, and fixed-pack evidence availability.
Scoring uses deterministic concept-token groups so harmless paraphrase does not
fail a claim, while citations, negation, forbidden conclusions, and stable
identifiers remain explicit gates.

```bash
python3 -m json.tool eval/personal_coordination_cases.json
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json validate
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json run \
  --filesystem-native \
  --concurrency 3 \
  --timeout 420 \
  --out results/2026-07-12-native-personal-regraded-v1.json
```

Result: filesystem passed 13/15 cases and 58/60 claims; native Straylight
passed 14/15 and 59/60, persisted 14/14 eligible checkpoints, and persisted
none for the read-only card.

Primary files:

- `eval/personal_coordination_cases.json`
- `eval/corpus-personal-coordination-v0.1`
- `tests/test_agent_work_eval.py`

## Retrieval regression lane

The earlier deterministic harness remains as a narrower regression test for evidence availability. It compares direct file selection, a one-shot context pack, and a deterministic workspace policy over 20 questions and 53 evidence items.

```bash
python3 straylight_eval.py validate --vault eval/corpus-v0.1
python3 straylight_eval.py run \
  --vault eval/corpus-v0.1 \
  --baseline results/2026-07-10-v0.1.json \
  --out results/2026-07-10-v0.2.json
```

The retrieval lane is not the product evaluation. Its tuned workspace, direct, and one-shot methods each passed 17/20 cases; one-shot recovered the most exact evidence at the lowest context cost.

## Frozen data

- Agent-work corpus SHA-256: `b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2`
- Rupture Ops indexed-corpus SHA-256: `aa9c33e39777f0899dacecee61a94f5fd11ec1315f7a6e820a41bc217e1a9803`
- Rupture Ops full artifact-tree SHA-256: `aa434eb3ffd5f4b6b9c766000d34ec6c30fd14b5ec6f94a4ff625306396b7b50`
- Personal coordination manifest SHA-256: `af92c68c6ac5abefb0d93d42d4445ab3cd3616b8b1283292125cc5a946aa77a6`
- Personal coordination indexed-corpus SHA-256: `1f2d62e8f27d2309bdb9353ff349277e038a58b4753c6f3199fd608e9c97ff18`
- Personal coordination artifact-tree SHA-256: `1f2d62e8f27d2309bdb9353ff349277e038a58b4753c6f3199fd608e9c97ff18`
- Retrieval corpus SHA-256: `7e92b3cc21ff20c80964e89d84eb44aad0bb38b4b7d5fe70105850ce8455c1bf`

These fixtures contain local personal and project context selected for evaluation. Review them before publishing or moving this directory to a remote repository.
