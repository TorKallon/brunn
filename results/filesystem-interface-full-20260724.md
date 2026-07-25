# CarryState interface evaluation

- Run: `filesystem-full-v1-20260724a`
- Model: `gpt-5.6-sol` at `xhigh` reasoning
- Fresh-agent runs: 90
- Cases per complete cell: 45
- Claims per complete cell: 180

## Overall matrix

| Agent | Interface | Workflow pass | Claims | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| codex | filesystem | 44/45 | 179/180 | 0.9853 | 278600.1 | 50390.3 | 8330.1 | 0.00 | 171.17 |
| openclaw | filesystem | 45/45 | 180/180 | 0.9874 | 42939.9 | 3897.0 | 4647.7 | 0.00 | 165.57 |

## Distribution and API-equivalent cost

| Agent | Interface | Input median / p95 | Uncached median / p95 | Seconds median / p95 | Calls median / p95 | Estimated cost per run |
|---|---|---:|---:|---:|---:|---:|
| codex | filesystem | 182682 / 728175 | 36867 / 107320 | 166.8 / 282.2 | 0 / 0 | $0.616 to $0.679 |
| openclaw | filesystem | 34606 / 87229 | 3255 / 8418 | 148.8 / 325.3 | 0 / 0 | $0.178 to $0.183 |

## Paired differences

Token deltas are right interface minus left interface on the same task and agent.

| Comparison | Cases | Wins | Losses | Score delta | Input delta | Uncached delta | Output delta |
|---|---:|---:|---:|---:|---:|---:|---:|

## Suite detail

| Suite | Cell | Workflow pass | Claims | Mean input | Mean uncached |
|---|---|---:|---:|---:|---:|
| work | codex/filesystem | 13/13 | 52/52 | 240101.3 | 44930.8 |
| work | openclaw/filesystem | 13/13 | 52/52 | 42069.8 | 4674.2 |
| personal | codex/filesystem | 15/15 | 60/60 | 148959.1 | 26318.0 |
| personal | openclaw/filesystem | 15/15 | 60/60 | 25623.4 | 2600.5 |
| rupture | codex/filesystem | 11/12 | 47/48 | 523909.5 | 91824.2 |
| rupture | openclaw/filesystem | 12/12 | 48/48 | 66410.5 | 4031.8 |
| transition | codex/filesystem | 5/5 | 20/20 | 178877.4 | 37360.6 |
| transition | openclaw/filesystem | 5/5 | 20/20 | 40821.8 | 5442.6 |

## Protocol

- Every cell received the same task, claim slots, evidence discipline, answer schema, model, and reasoning effort; only the access surface differed.
- Every CarryState run used a separate service user, credential, corpus, session history, and writable checkpoint history. Each filesystem control used a separate run directory over the same frozen corpus.
- CLI retained session IDs and compacted service responses. MCP supplied typed tools and the same compact reasoning view. HTTP returned raw JSON and required the agent to manage identifiers and payloads. Filesystem controls used ordinary local search and reads without CarryState.
- CarryState workflow passes require a passing answer and the expected durable checkpoint. CarryState transition cases additionally require exact parent, corpus revision, old-source, delta-source, and four-call lineage gates. Filesystem workflow passes grade the answer only.
- Input includes cached input. Uncached input is the newly billed or newly processed portion reported by the runtime; output includes visible and hidden reasoning tokens when the provider reports them together.
- The dollar range is an API-price equivalent, not a claim about the user's actual Codex or OpenClaw bill. Its low end charges reported uncached input at the standard rate; its high end conservatively treats all reported uncached input as cache writes.
- Pricing was checked 2026-07-24 against https://developers.openai.com/api/docs/models/gpt-5.6-sol; every complete-run token total was not guaranteed below the 272,000-token long-context threshold.

## Reproduce

```bash
cd /Users/Shared/projects/straylight
python3 interface_eval.py validate
python3 interface_eval.py run --resume-run-id filesystem-full-v1-20260724a --out results/filesystem-interface-full-20260724.json --report results/filesystem-interface-full-20260724.md
```
