# CarryState interface evaluation

- Run: `interface-20260725T002745-0700`
- Model: `gpt-5.6-sol` at `xhigh` reasoning
- Fresh-agent runs: 2
- Cases per complete cell: 1
- Claims per complete cell: 4
- Repetitions: 1
- Randomization seed: `8207292676607004745`
- Quality-comparison status: **not publishable**
- Quality blocker: run is not the full predeclared matrix
- Quality blocker: fewer than five randomized repetitions were run
- Quality blocker: filesystem and CarryState matched pairs are incomplete
- Quality blocker: same-user Seatbelt containment is screening-only; publishable containment requires a separate OS identity or VM
- Quality blocker: one or more model request/response contracts is incomplete
- Quality blocker: parent-owned token accounting is incomplete
- Quality blocker: deterministic keyword rubrics are screening evidence; blind semantic or human adjudication is required for a quality claim

## Overall matrix

| Agent | Interface | Workflow pass | Claims | Valid answers | Process failures | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| codex | filesystem | 0/1 | 4/4 | 1 | 0 | 1.0000 | 0.0 | 0.0 | 0.0 | 0.00 | 218.09 |
| openclaw | filesystem | 0/1 | 3/4 | 1 | 0 | 0.9433 | 0.0 | 0.0 | 0.0 | 0.00 | 150.58 |

## Paired differences

Token deltas are right interface minus left interface on the same task and agent.

| Comparison | Paired runs | Wins | Losses | Score delta (95% CI) | Input delta | Uncached delta | Output delta |
|---|---:|---:|---:|---:|---:|---:|---:|

## Suite detail

| Suite | Cell | Workflow pass | Claims | Process failures | Mean input | Mean uncached |
|---|---|---:|---:|---:|---:|---:|
| work | codex/filesystem | 0/1 | 4/4 | 0 | 0.0 | 0.0 |
| work | openclaw/filesystem | 0/1 | 3/4 | 0 | 0.0 | 0.0 |

## Protocol

- Within each agent, every interface received the same task, claim slots, evidence discipline, model ID, and reasoning effort. Codex and OpenClaw are not treated as interchangeable runtimes: their system prompts, tool plumbing, and structured-output enforcement differ.
- Primary workflow and claim rates include every attempted run. Provider, authentication, timeout, malformed-output, and runner failures therefore count as misses; evaluated-only rates remain secondary diagnostics in the JSON.
- Every CarryState run used a separate service user, credential, corpus, session history, and writable checkpoint history. Each filesystem control used a private read-only copy of the same frozen operational corpus.
- CLI retained session IDs and compacted service responses. MCP supplied typed tools and the same compact reasoning view. HTTP returned raw JSON and required the agent to manage identifiers and payloads. Filesystem controls used ordinary local search and reads without CarryState.
- The evaluator gives each agent an isolated home and temporary directory. Network access is restricted to exact parent-owned model, CarryState, and native-inspection ports. The parent injects real credentials and records service, checkpoint, download, and inspection receipts; agent-written traces are diagnostic only.
- Deterministic claim rubrics are a screening signal, not final quality adjudication. A result cannot be published as parity evidence without a complete matched matrix, at least five randomized repetitions, and blind semantic or human review.
- CarryState workflow passes require a passing answer and the expected durable checkpoint. CarryState transition cases additionally require exact parent, corpus revision, old-source, delta-source, and four-call lineage gates. Filesystem workflow passes grade the answer only.
- Input includes cached input. Uncached input is the newly billed or newly processed portion reported by the runtime; output includes visible and hidden reasoning tokens when the provider reports them together.
- API-equivalent cost is unavailable because parent-owned token receipts are incomplete.


## Reproduce

```bash
cd /Users/Shared/projects/straylight
python3 interface_eval.py validate
python3 interface_eval.py run --resume-run-id interface-20260725T002745-0700 --repetitions 1 --randomization-seed 8207292676607004745 --out results/alpha-filesystem-runtime-smoke-v2-20260725.json --report results/alpha-filesystem-runtime-smoke-v2-20260725.md
```
