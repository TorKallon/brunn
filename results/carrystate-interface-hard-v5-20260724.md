# CarryState interface evaluation

- Run: `interface-hard-v5-20260724a`
- Model: `gpt-5.6-sol` at `xhigh` reasoning
- Fresh-agent runs: 24
- Cases per complete cell: 4
- Claims per complete cell: 16

## Overall matrix

| Agent | Interface | Workflow pass | Claims | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| codex | cli | 2/4 | 15/16 | 0.9610 | 138536.0 | 36008.0 | 7352.5 | 4.00 | 161.78 |
| codex | mcp | 4/4 | 16/16 | 0.9646 | 167949.0 | 40269.0 | 6590.0 | 3.75 | 142.49 |
| codex | api | 3/4 | 14/16 | 0.9469 | 160779.2 | 43595.2 | 7743.0 | 3.75 | 164.99 |
| openclaw | cli | 4/4 | 16/16 | 0.9504 | 35439.0 | 3439.0 | 3077.5 | 4.25 | 158.81 |
| openclaw | mcp | 4/4 | 16/16 | 0.9752 | 41167.5 | 3535.5 | 3544.0 | 3.75 | 161.81 |
| openclaw | api | 4/4 | 16/16 | 0.9681 | 49962.8 | 4138.8 | 3554.2 | 4.00 | 184.58 |

## Distribution and API-equivalent cost

| Agent | Interface | Input median / p95 | Uncached median / p95 | Seconds median / p95 | Calls median / p95 | Estimated cost per run |
|---|---|---:|---:|---:|---:|---:|
| codex | cli | 141863 / 157633 | 36904 / 56510 | 161.2 / 175.4 | 4 / 5 | $0.452 to $0.497 |
| codex | mcp | 155331 / 221382 | 38851 / 61894 | 142.6 / 184.5 | 4 / 4 | $0.463 to $0.513 |
| codex | api | 151210 / 206539 | 39272 / 61131 | 145.7 / 231.0 | 4 / 4 | $0.509 to $0.563 |
| openclaw | cli | 35010 / 43700 | 3130 / 4804 | 159.6 / 194.7 | 4 / 5 | $0.126 to $0.130 |
| openclaw | mcp | 41187 / 50985 | 3516 / 4084 | 155.8 / 206.2 | 4 / 4 | $0.143 to $0.147 |
| openclaw | api | 48803 / 59317 | 4100 / 5108 | 192.5 / 204.3 | 4 / 4 | $0.150 to $0.155 |

## Paired differences

Token deltas are right interface minus left interface on the same task and agent.

| Comparison | Cases | Wins | Losses | Score delta | Input delta | Uncached delta | Output delta |
|---|---:|---:|---:|---:|---:|---:|---:|
| codex:mcp-minus-cli | 4 | 2 | 0 | +0.0035 | +29413.0 | +4261.0 | -762.5 |
| codex:api-minus-cli | 4 | 2 | 1 | -0.0142 | +22243.2 | +7587.2 | +390.5 |
| codex:api-minus-mcp | 4 | 0 | 1 | -0.0177 | -7169.8 | +3326.2 | +1153.0 |
| openclaw:mcp-minus-cli | 4 | 0 | 0 | +0.0248 | +5728.5 | +96.5 | +466.5 |
| openclaw:api-minus-cli | 4 | 0 | 0 | +0.0177 | +14523.8 | +699.8 | +476.8 |
| openclaw:api-minus-mcp | 4 | 0 | 0 | -0.0071 | +8795.2 | +603.2 | +10.2 |

## Suite detail

| Suite | Cell | Workflow pass | Claims | Mean input | Mean uncached |
|---|---|---:|---:|---:|---:|
| rupture | codex/cli | 1/2 | 7/8 | 148904.5 | 36904.5 |
| rupture | codex/mcp | 2/2 | 8/8 | 184011.0 | 48075.0 |
| rupture | codex/api | 1/2 | 6/8 | 170348.0 | 48492.0 |
| rupture | openclaw/cli | 2/2 | 8/8 | 39226.5 | 3130.5 |
| rupture | openclaw/mcp | 2/2 | 8/8 | 49934.5 | 3598.5 |
| rupture | openclaw/api | 2/2 | 8/8 | 55043.5 | 4099.5 |
| transition | codex/cli | 1/2 | 8/8 | 128167.5 | 35111.5 |
| transition | codex/mcp | 2/2 | 8/8 | 151887.0 | 32463.0 |
| transition | codex/api | 2/2 | 8/8 | 151210.5 | 38698.5 |
| transition | openclaw/cli | 2/2 | 8/8 | 31651.5 | 3747.5 |
| transition | openclaw/mcp | 2/2 | 8/8 | 32400.5 | 3472.5 |
| transition | openclaw/api | 2/2 | 8/8 | 44882.0 | 4178.0 |

## Protocol

- Every cell received the same task, claim slots, retrieval policy, answer schema, model, and reasoning effort.
- Every run used a separate CarryState user, credential, corpus, session history, and writable checkpoint history.
- CLI retained session IDs and compacted service responses. MCP supplied typed tools and the same compact reasoning view. HTTP returned raw JSON and required the agent to manage identifiers and payloads.
- A workflow pass requires a passing answer and the expected durable checkpoint. Transition cases additionally require exact parent, corpus revision, old-source, delta-source, and four-call lineage gates.
- Input includes cached input. Uncached input is the newly billed or newly processed portion reported by the runtime; output includes visible and hidden reasoning tokens when the provider reports them together.
- The dollar range is an API-price equivalent, not a claim about the user's actual Codex or OpenClaw bill. Its low end charges reported uncached input at the standard rate; its high end conservatively treats all reported uncached input as cache writes.
- Pricing was checked 2026-07-24 against https://developers.openai.com/api/docs/models/gpt-5.6-sol; every complete-run token total was below the 272,000-token long-context threshold.

## Reproduce

```bash
cd /Users/Shared/projects/straylight
python3 interface_eval.py validate
python3 interface_eval.py run --resume-run-id interface-hard-v5-20260724a --out results/carrystate-interface-hard-v5-20260724.json --report results/carrystate-interface-hard-v5-20260724.md
```
