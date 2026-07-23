# Straylight

Straylight is an agent-first context and durable-work layer. It gives agents a
source-preserving place to learn, resume complex work, inspect artifacts,
reason across changing state, verify claims, and leave useful durable
checkpoints. It is more than retrieval or memory: the core product is a
multi-user, snapshot-pinned workspace that later agents can safely advance.

The product stack is Rust, PostgreSQL with pgvector, MinIO, OpenAI text
embeddings, a TypeScript SPA, and a typed TypeScript MCP server. Every component
runs in Docker. Python and SQLite remain only in the frozen evaluation harness
and do not define product behavior.

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

- `apps/api`: Rust API, worker, domain contracts, migrations, retrieval,
  automatic capture, canonical writes, staging, checkpoints, deletion, and
  Phase 0 dreaming
- `apps/web`: TypeScript/React audit and control SPA
- `apps/mcp`: typed stdio MCP adapter
- `infra`: Postgres role initialization and pinned MinIO build/policy
- `docs/Architecture.md`: system design and trust boundaries
- `docs/Specification.md`: alpha behavior and acceptance contract
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

See `docs/Operations.md` for migrations, MCP, comparative evaluation, and
storage notes.

## Evaluations

The harnesses test whether a fresh agent can capture ordinary source material,
receive safe fresh learned context, recover canonical knowledge, resume complex
work, inspect artifacts, reason across superseded state, revise plans, verify
claims, and leave a useful durable checkpoint.

### 2026-07-22 retrieval simplification evaluation

The current implementation keeps the canonical write and dreaming contracts
unchanged while simplifying the agent-facing read path. `memory.open` is the
first evidence packet, open retrieval keeps related source sections coherent,
targeted query defaults to eight source leads, and exact reads support natural
range and neighbor forms. MCP and evaluation adapters return a compact
reasoning view while the HTTP audit representation remains complete.

The fresh comparison below excludes the retired personal/work-boundary card.
Scores are deterministic regrades of the original frozen answers after the
same conservative concept matcher was applied to both conditions. Raw run
files remain alongside the regraded results.

| Suite | Filesystem | Native Straylight | Interpretation |
| --- | ---: | ---: | --- |
| Main active work | 12/13 cases, 51/52 claims | **13/13, 52/52** | Native wins one case and one claim; mean score is also higher, 0.978 vs 0.971. |
| Rupture Ops | 10/12, 45/48 | **10/12, 46/48** | Cases tie; native wins one claim and mean score, 0.955 vs 0.940. |
| Personal coordination | 14/15, 59/60 | **15/15, 60/60** | Native wins one case and one claim; filesystem retains the higher mean detail score, 0.975 vs 0.966. |
| Changed-evidence transitions | 5/5, 20/20 | **5/5, 20/20** | Quality ties; native commits five parent-, revision-, and source-linked children. |

Across all four suites, native passes 43/45 cases and 178/180 claims versus
41/45 and 175/180 for flat files. Native reduces uncached input on Rupture Ops
by 12% and on transitions by 49%; it remains 12% higher on main one-shot work
and 38% higher on the small personal corpus. Mean elapsed time is also still
higher for native one-shot work, so retrieval latency and compact personal
responses remain explicit optimization targets. On main and Rupture Ops,
native service output is about 80% and 64% smaller than filesystem command
output respectively.

The last pre-change service answers, regraded with the same matcher and with
the retired boundary card removed, passed 41/45 cases and 174/180 claims. The
new run therefore gains two cases and four claims overall. Main improves from
12/13 and 51/52 to 13/13 and 52/52; personal improves from 13/15 and 57/60 to
15/15 and 60/60; transitions remain perfect. Rupture Ops keeps 46/48 claims
and raises mean score from 0.950 to 0.955, but moves from 11/12 to 10/12 whole
cases. Service output falls by 53% on main, 70% on personal, and 60% on Rupture
Ops versus those pre-change runs. These are separate stochastic agent runs, so
the aggregate and audited claim changes are more meaningful than attributing
every individual case movement to retrieval alone.

Current result files:

- `results/2026-07-22-simplification-full-main-regraded.json`
- `results/2026-07-22-simplification-full-rupture-regraded.json`
- `results/2026-07-22-simplification-full-personal-regraded.json`
- `results/2026-07-22-simplification-full-transitions.json`

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
  --out results/2026-07-12-native-transitions-optimized.json \
  --report "/Users/aether/obsidian/notes/Projects/Straylight/Native checkpoint transition evaluation results - 2026-07-12.md"
```

Result:

- The raw deterministic score tied at 4/5 cards and 19/20 claims. Manual review
  found both condition-specific misses were narrow phrase-grader misses rather
  than contradictory answers.
- Native Straylight committed 5/5 correctly linked, source-preserving child
  checkpoints and stayed within the four-call gate on every card.
- Native resume reduced cumulative input 38%, uncached input 29%, shell calls
  44%, and mean elapsed time 15% versus filesystem reconstruction.
- Complete bounded N-to-N+1 source deltas are returned inline. Four cards used
  `resume -> checkpoint`; one used `resume -> query -> read -> checkpoint`.
- A live immediate post-index probe completed in 110 ms without an embedding
  dependency on the continuation critical path.

Primary v0.3 files:

- `transition_eval.py`
- `native_memory.py`
- `eval/transition_cases.json`
- `eval/transition-deltas`
- `apps/api/src/read_service.rs`
- `results/2026-07-12-native-transitions-optimized.json`

## Agent-work evaluation

The primary v0.2 suite compares three access surfaces:

1. **Fixed handoff pack:** one task-specific context file with no follow-up access.
2. **Filesystem agent:** the frozen corpus with ordinary search, file reads, and scripts.
3. **Straylight workspace agent:** `open`, `query`, `read`, `compute`, `verify`, and persistent `checkpoint` operations.

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
