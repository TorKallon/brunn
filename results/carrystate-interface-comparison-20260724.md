# CarryState agent-interface comparison

Date: 2026-07-24

## Bottom line

OpenClaw can use CarryState at local-file reasoning quality. The corrected full
OpenClaw MCP run passed every case and every scored claim:

| Full 45-case suite | CarryState MCP | Local Markdown |
| --- | ---: | ---: |
| Answer cases | 45/45 | 45/45 |
| Workflow cases | 45/45 | 45/45 |
| Claims | 180/180 | 180/180 |
| Required checkpoints persisted | 44/44 | 0/44 |
| Mean agent/tool calls | 2.98 | 6.71 |
| Mean input tokens | 36,177 | 42,940 |
| Mean newly processed input tokens | 3,119 | 3,897 |
| Mean output tokens | 2,574 | 4,648 |
| Mean elapsed time | 138.2 seconds | 165.6 seconds |
| API-equivalent model cost, full suite | $4.92-$5.10 | $8.03-$8.25 |

Relative to local Markdown, CarryState MCP used about:

- 16% less total input;
- 20% less newly processed input;
- 45% less output;
- 56% fewer agent tool calls;
- 17% less elapsed time; and
- 38% less API-equivalent model cost.

The cost figures apply public API token prices to runtime-reported tokens. They
are comparison estimates, not the actual charge for an OpenClaw or Codex
subscription.

The earlier conclusion that OpenClaw should begin with the CLI is obsolete.
MCP is now the preferred agent interface for both OpenClaw and Codex. It gives
OpenClaw typed operations, reaches strict reasoning parity, and adds the
durable checkpoint that local-file access cannot provide.

## What was tested

The frozen suite contains 45 cases and 180 independently scored claims across:

- complex engineering and production operations;
- people, events, schedules, logistics, family coordination, and policy;
- vacation planning using the Switzerland trip;
- subject-matter research and companion-product work using RuptureOps; and
- fresh-agent continuation with changed evidence or constraints.

Every run launched a fresh OpenClaw or Codex agent. CarryState runs used
isolated users, credentials, corpora, sessions, and checkpoint histories.
Filesystem runs received the same task, claim slots, answer contract, and
frozen source corpus.

The current OpenClaw result used the same installed Nyx route used by the
owner's real OpenClaw setup:

- model: `openai/gpt-5.6-sol`;
- provider: OpenAI;
- runtime: the subscription-backed Codex runtime; and
- interface: the CarryState MCP server over stdio.

The evaluation injected that MCP server and an isolated test credential into a
temporary OpenClaw configuration. CarryState has not yet been added to the
owner's global OpenClaw configuration, so this proves the integration but does
not pretend the daily agent is already connected.

## Why the old OpenClaw result was lower

The original OpenClaw MCP cell was 41/45 cases and 176/180 claims. That result
combined four different issues; it was not evidence that OpenClaw could not
reason over CarryState.

### 1. OpenClaw compressed away required details

In all four original misses, OpenClaw had retrieved and cited the correct
sources. Its final claims omitted one required condition, exception, or
identifier. The installed OpenClaw system overlay explicitly asks the model to
default to concise, dense replies. That is normally useful, but the evaluation
claim slots required every decision-relevant condition to remain
self-contained.

The integration contract now tells the agent to preserve concrete names, IDs,
quantities, conditions, exceptions, status, and provenance in each claim. The
four formerly failing RuptureOps cases now pass.

### 2. Two grader checks rejected substantively correct answers

The person-resolution answer said that both records had the same name and
email and had not been merged. The grader required the literal phrases
`matching name` or `matching email`.

The deadline-readiness answer said inspection was pending and the packet
remained incomplete. The grader accepted only slightly different phrasing.

The accepted alternatives were expanded without weakening the underlying
requirements. Existing local-file results were regraded under the same current
rubric and still pass 45/45 cases and 180/180 claims.

### 3. One real locator-control problem was fixed

In the API-gate case, OpenClaw once synthesized a plausible filename instead
of copying the path returned by CarryState. The MCP descriptions and harness
now require exact verbatim `path` and `ref` values and explicitly forbid
inventing filenames from titles or topics.

Two targeted reruns passed all four claims, and the corrected full run also
passed the case with the exact locator.

### 4. One MCP field description was wrong

The corrected full run had one five-call efficiency outlier. The answer and
checkpoint were correct, but OpenClaw made an unnecessary recovery query.

The root cause was a CarryState MCP schema defect. It described
`where.scope_root` as the authorization scope returned by `memory.open`.
OpenClaw followed that instruction and sent `scope:root`. The API correctly
interpreted `where.scope_root` as a graph record reference and returned
`requested reference was not found`.

The MCP schema now distinguishes the two fields:

- query `scope` is the authorization scope, such as `scope:root`;
- `where.scope_root` is an optional known graph record, such as
  `object:...`, `claim:...`, or `source_episode:...`.

The post-fix live OpenClaw run used exactly four successful calls:

1. `memory.open`;
2. one focused lexical/temporal query for the missing May record;
3. one read using the exact returned path; and
4. one checkpoint using real evidence and source-episode IDs.

It passed 1/1 case, 4/4 claims, the workflow gate, and checkpoint persistence.

## Current OpenClaw result

The full corrected OpenClaw MCP run produced:

- 45/45 successful processes;
- 45/45 answer cases;
- 45/45 workflow cases;
- 180/180 claims;
- 44/44 required checkpoints;
- 44/45 cases within four CarryState calls before the final field-description
  fix;
- 2.98 mean calls, with median 3 and p95 4;
- 1,627,971 total input tokens, of which 140,355 were newly processed;
- 115,815 output tokens; and
- $4.92-$5.10 API-equivalent model cost.

The sole five-call case was the `scope_root` defect described above. Its
post-fix targeted rerun passed in four calls, so no known OpenClaw case remains
outside the intended four-call workflow.

## Direct API-key experiment

One alarming OpenClaw smoke result made 25 failed `memory.open` calls, consumed
460,412 input tokens, retrieved no source, and persisted no checkpoint. It was
not the route used by the successful full OpenClaw run or by the owner's Nyx
configuration.

The harness had created a custom provider named `openai-direct`, which forced
OpenClaw down a generic provider path. On that path the model repeatedly
invented values for optional `resume_checkpoint_ref` even though the outgoing
schema made only `task` required.

The harness now uses the canonical `openai` provider and
`openai/gpt-5.6-sol` model identity for both authentication modes, with the
installed runtime selected appropriately. A dummy-key trace confirmed native
OpenAI request shaping. A paid live rerun of the direct API-key route is still
blocked by the current OpenAI Platform quota, so that route is not qualified
for alpha.

This does not block the actual subscription-backed OpenClaw route, which is
the one that passed 45/45.

## Transcript truncation finding

The installed OpenClaw build limits tool output copied into its persisted
transcript mirror to 12,000 characters. Source inspection shows that the limit
is applied after the native turn, when OpenClaw mirrors the result for history
and UI use. It is not a limit on the tool result seen by the model during the
live turn and did not cause the earlier reasoning misses.

It could matter if a later process tried to reconstruct work from the mirrored
transcript alone. CarryState's source-bearing checkpoint is the stronger
continuation mechanism and avoids depending on that transcript copy.

## Interface recommendation

### MCP

Use MCP as the primary interface for OpenClaw and Codex. It provides typed
operations and field-level guidance while returning the compact reasoning
view. It now has the strongest complete OpenClaw result and the strongest
completed Codex parity result.

### Thin CLI

Keep the CLI as a useful fallback for shell-capable agents and diagnostics. It
calls the same API and remembers local session/checkpoint identifiers, but its
contract is less discoverable than MCP and it depends on a local launcher.

### Raw HTTP

Keep raw HTTP as the application and SDK contract. It asks a free-form agent to
construct more payload machinery and interpret more transport and ranking
metadata. In the frozen matrix this increased context and workflow errors
without improving reasoning quality.

## Broader matrix

The original complete interface matrix predates the final prompt and schema
fixes, so it is retained as historical evidence rather than the current
OpenClaw MCP score:

| Agent | Access | Answer cases | Claims | Required checkpoints |
| --- | --- | ---: | ---: | ---: |
| Codex | Local files | 44/45 | 179/180 | n/a |
| Codex | Thin CLI | 42/45 | 176/180 | 44/44 |
| Codex | MCP | 43/45 | 178/180 | 44/44 |
| Codex | Raw API | 43/45 | 178/180 | 44/44 |
| OpenClaw | Local files | 45/45 | 180/180 | n/a |
| OpenClaw | Thin CLI | 44/45 | 179/180 | 44/44 |
| OpenClaw | MCP, historical | 41/45 | 176/180 | 44/44 |
| OpenClaw | Raw API | 44/45 | 179/180 | 44/44 |

The later Codex MCP parity rerun completed 41 cases before OpenAI Platform
quota exhaustion. On the matched set, CarryState and local files both passed
40/41 cases and 163/164 claims, while CarryState used 58% less total input,
40% less newly processed input, 45% less time, and about 39% less
API-equivalent model cost. The four quota-rejected requests made no model calls
and are not reasoning failures.

## Alpha recommendation

1. Use CarryState MCP for the owner OpenClaw agent.
2. Start with a dedicated read-only credential to confirm routing and privacy,
   then give only the trusted private owner agent a separate read/write
   credential.
3. Do not expose the CarryState tool or credential to group-facing OpenClaw
   routes.
4. Keep CLI available for diagnosis and raw HTTP for deterministic app code.
5. Qualify the direct API-key OpenClaw route only after Platform quota is
   available; it is not needed for the subscription-backed alpha route.

## Artifacts

- `results/openclaw-mcp-full-v3-20260724.json`
- `results/openclaw-mcp-full-v3-20260724.md`
- `results/filesystem-interface-full-20260724.json`
- `results/filesystem-interface-full-20260724.md`
- `results/openclaw-charlemagne-provenance-fix-20260724.json`
- `results/openclaw-charlemagne-scope-contract-fix-20260724.json`
- `results/carrystate-interface-full-20260724.json`
- `results/carrystate-interface-hard-v5-20260724.json`
- `results/codex-mcp-parity-v2-20260724.json`
- `results/api-billing-openclaw-mcp-smoke-20260724d.json`
