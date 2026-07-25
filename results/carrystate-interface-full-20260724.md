# CarryState interface evaluation

- Run: `interface-full-v2-20260724a`
- Model: `gpt-5.6-sol` at `xhigh` reasoning
- Fresh-agent runs: 270
- Cases per complete cell: 45
- Claims per complete cell: 180

## Overall matrix

| Agent | Interface | Workflow pass | Claims | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| codex | cli | 42/45 | 176/180 | 0.9673 | 106835.5 | 28044.4 | 3681.1 | 3.27 | 89.54 |
| codex | mcp | 43/45 | 178/180 | 0.9588 | 133023.0 | 32136.3 | 4139.0 | 3.38 | 100.67 |
| codex | api | 41/45 | 178/180 | 0.9696 | 146882.1 | 37570.1 | 4724.2 | 3.76 | 109.63 |
| openclaw | cli | 43/45 | 179/180 | 0.9619 | 30396.8 | 2839.9 | 1560.5 | 3.18 | 96.04 |
| openclaw | mcp | 41/45 | 176/180 | 0.9610 | 32376.1 | 2481.0 | 1567.7 | 3.42 | 106.83 |
| openclaw | api | 42/45 | 179/180 | 0.9672 | 41539.2 | 3110.7 | 1679.1 | 3.84 | 115.89 |

## Distribution and API-equivalent cost

| Agent | Interface | Input median / p95 | Uncached median / p95 | Seconds median / p95 | Calls median / p95 | Estimated cost per run |
|---|---|---:|---:|---:|---:|---:|
| codex | cli | 114732 / 153123 | 29304 / 37667 | 89.5 / 121.8 | 4 / 4 | $0.290 to $0.325 |
| codex | mcp | 141531 / 185643 | 31195 / 52239 | 95.3 / 146.3 | 4 / 4 | $0.335 to $0.375 |
| codex | api | 157691 / 220444 | 36971 / 51323 | 108.5 / 173.2 | 4 / 5 | $0.384 to $0.431 |
| openclaw | cli | 30452 / 37909 | 2307 / 3767 | 99.2 / 136.4 | 3 / 4 | $0.075 to $0.078 |
| openclaw | mcp | 31363 / 47155 | 2484 / 3345 | 106.7 / 151.6 | 4 / 4 | $0.074 to $0.077 |
| openclaw | api | 43201 / 48719 | 2883 / 4222 | 110.9 / 170.9 | 4 / 5 | $0.085 to $0.089 |

## Paired differences

Token deltas are right interface minus left interface on the same task and agent.

| Comparison | Cases | Wins | Losses | Score delta | Input delta | Uncached delta | Output delta |
|---|---:|---:|---:|---:|---:|---:|---:|
| codex:mcp-minus-cli | 45 | 2 | 1 | -0.0086 | +26187.5 | +4091.9 | +457.9 |
| codex:api-minus-cli | 45 | 2 | 3 | +0.0022 | +40046.6 | +9525.7 | +1043.1 |
| codex:api-minus-mcp | 45 | 1 | 3 | +0.0108 | +13859.1 | +5433.8 | +585.2 |
| openclaw:mcp-minus-cli | 45 | 2 | 4 | -0.0009 | +1979.3 | -358.9 | +7.2 |
| openclaw:api-minus-cli | 45 | 1 | 2 | +0.0053 | +11142.3 | +270.8 | +118.6 |
| openclaw:api-minus-mcp | 45 | 3 | 2 | +0.0062 | +9163.0 | +629.7 | +111.4 |

## Suite detail

| Suite | Cell | Workflow pass | Claims | Mean input | Mean uncached |
|---|---|---:|---:|---:|---:|
| work | codex/cli | 13/13 | 52/52 | 131132.5 | 32592.2 |
| work | codex/mcp | 13/13 | 52/52 | 151436.2 | 42025.7 |
| work | codex/api | 13/13 | 52/52 | 140482.5 | 38456.7 |
| work | openclaw/cli | 13/13 | 52/52 | 32372.5 | 2381.1 |
| work | openclaw/mcp | 13/13 | 52/52 | 33249.2 | 2391.4 |
| work | openclaw/api | 13/13 | 52/52 | 42521.4 | 3235.2 |
| personal | codex/cli | 14/15 | 59/60 | 87158.9 | 24285.3 |
| personal | codex/mcp | 14/15 | 59/60 | 119525.7 | 27690.0 |
| personal | codex/api | 15/15 | 60/60 | 142204.9 | 35726.0 |
| personal | openclaw/cli | 14/15 | 59/60 | 26871.9 | 3763.6 |
| personal | openclaw/mcp | 15/15 | 60/60 | 30328.2 | 2321.8 |
| personal | openclaw/api | 15/15 | 60/60 | 40397.5 | 3106.9 |
| rupture | codex/cli | 11/12 | 47/48 | 102420.1 | 28201.4 |
| rupture | codex/mcp | 11/12 | 47/48 | 127769.9 | 29615.2 |
| rupture | codex/api | 10/12 | 46/48 | 154259.8 | 41406.5 |
| rupture | openclaw/cli | 12/12 | 48/48 | 32086.9 | 2390.9 |
| rupture | openclaw/mcp | 8/12 | 44/48 | 34544.2 | 2714.9 |
| rupture | openclaw/api | 11/12 | 47/48 | 44874.1 | 2975.4 |
| transition | codex/cli | 4/5 | 18/20 | 113290.2 | 27120.6 |
| transition | codex/mcp | 5/5 | 20/20 | 138248.4 | 25813.2 |
| transition | codex/api | 3/5 | 20/20 | 159846.0 | 31590.0 |
| transition | openclaw/cli | 4/5 | 20/20 | 31779.0 | 2339.0 |
| transition | openclaw/mcp | 5/5 | 20/20 | 31046.2 | 2630.2 |
| transition | openclaw/api | 3/5 | 20/20 | 34406.4 | 3123.2 |

## Protocol

- Every cell received the same task, claim slots, evidence discipline, answer schema, model, and reasoning effort; only the access surface differed.
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
python3 interface_eval.py run --resume-run-id interface-full-v2-20260724a --out results/carrystate-interface-full-20260724.json --report results/carrystate-interface-full-20260724.md
```
