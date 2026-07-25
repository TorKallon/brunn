# CarryState interface evaluation

- Run: `codex-mcp-parity-v2-20260724a`
- Model: `gpt-5.6-sol` at `xhigh` reasoning
- Fresh-agent runs: 45
- Cases per complete cell: 45
- Claims per complete cell: 180

## Overall matrix

| Agent | Interface | Evaluated workflow pass | Evaluated claims | Process failures | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| codex | mcp | 40/41 | 163/164 | 4 | 0.8945 | 109872.1 | 28432.2 | 5550.0 | 2.82 | 92.16 |

## Distribution and API-equivalent cost

| Agent | Interface | Input median / p95 | Uncached median / p95 | Seconds median / p95 | Calls median / p95 | Estimated cost per run |
|---|---|---:|---:|---:|---:|---:|
| codex | mcp | 98610 / 218116 | 26545 / 48985 | 79.5 / 157.3 | 3 / 4 | $0.349 to $0.385 |

## Paired differences

Token deltas are right interface minus left interface on the same task and agent.

| Comparison | Cases | Wins | Losses | Score delta | Input delta | Uncached delta | Output delta |
|---|---:|---:|---:|---:|---:|---:|---:|

## Suite detail

| Suite | Cell | Evaluated workflow pass | Evaluated claims | Process failures | Mean input | Mean uncached |
|---|---|---:|---:|---:|---:|---:|
| work | codex/mcp | 13/13 | 52/52 | 0 | 141356.2 | 34409.7 |
| personal | codex/mcp | 15/15 | 60/60 | 0 | 93161.0 | 25502.7 |
| rupture | codex/mcp | 11/12 | 47/48 | 0 | 133736.7 | 34898.7 |
| transition | codex/mcp | 1/1 | 4/4 | 4 | 20872.0 | 6159.8 |

## Protocol

- Every cell received the same task, claim slots, evidence discipline, answer schema, model, and reasoning effort; only the access surface differed.
- Process failures are reported separately from evaluated answer and claim quality; a provider, authentication, timeout, or runner failure does not become a reasoning miss.
- Every run used a separate CarryState user, credential, corpus, session history, and writable checkpoint history.
- CLI retained session IDs and compacted service responses. MCP supplied typed tools and the same compact reasoning view. HTTP returned raw JSON and required the agent to manage identifiers and payloads. Filesystem controls used ordinary local search and reads without CarryState.
- CarryState workflow passes require a passing answer and the expected durable checkpoint. CarryState transition cases additionally require exact parent, corpus revision, old-source, delta-source, and four-call lineage gates. Filesystem workflow passes grade the answer only.
- Input includes cached input. Uncached input is the newly billed or newly processed portion reported by the runtime; output includes visible and hidden reasoning tokens when the provider reports them together.
- The dollar range is an API-price equivalent, not a claim about the user's actual Codex or OpenClaw bill. Its low end charges reported uncached input at the standard rate; its high end conservatively treats all reported uncached input as cache writes.
- Pricing was checked 2026-07-24 against https://developers.openai.com/api/docs/models/gpt-5.6-sol; every complete-run token total was below the 272,000-token long-context threshold.

## Reproduce

```bash
cd /Users/Shared/projects/straylight
python3 interface_eval.py validate
python3 interface_eval.py run --resume-run-id codex-mcp-parity-v2-20260724a --out results/codex-mcp-parity-v2-20260724.json --report results/codex-mcp-parity-v2-20260724.md
```
