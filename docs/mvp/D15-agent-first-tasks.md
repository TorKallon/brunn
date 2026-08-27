# D15 — Agent-first Tasks

Status: Accepted for implementation — measured storage choice complete; build in progress
Date: 2026-08-27
Depends on: D12, D13, notifications, documents, and the secret vault
Gated by: all twelve gates in the owner-approved 2026-08-26 task specification
Runtime flags: `STRAYLIGHT_TODOIST_SYNC_ENABLED` (default `false`) and the existing APNs gate

## Decision and measured spike

The canonical record is one versioned workspace entry per task at
`.straylight/tasks/<uuid>.md`, with `kind: task`, schema `task.v1`, readable
Markdown, and lossless typed metadata. Mutations use the simplified-core entry
upsert, giving task state entry history, workspace changes, optimistic
concurrency, account deletion, export, and import. Task entries never create
lexical or semantic chunks.

A transactional `task_index` is the rebuildable query projection. On local
PostgreSQL, 2,000 synthetic tasks and 500 timed samples measured:

| Shape | p95 | mean | max |
| --- | ---: | ---: | ---: |
| Decode candidate fields from entry JSON | 2.784 ms | 2.564 ms | 4.787 ms |
| Read the indexed projection | 0.295 ms | 0.273 ms | 3.867 ms |

The oldest-ready `EXPLAIN (ANALYZE, BUFFERS)` used
`spike_task_oldest_idx`, executed in 0.022 ms, and had no sequential scan. The
entry-plus-projection choice therefore preserves file-native authority with
large headroom under the 50 ms candidates and 100 ms project-state gates.

## Canonical schema and precedence

The Markdown body holds title and optional notes. Portable metadata holds
`kind`, `schema`, and a `task` envelope containing every product-spec §4.1
field. Enrichable fields are cells `{value, source, set_at, note?}` where source
is `owner`, `agent:<id>`, `todoist`, or `derived`; identifiers, title, version,
and done/drop timestamps are structural. Unknown API fields are rejected.

Precedence is enforced before changing the entry: owner cells are immutable to
agents, pulls, and derivation; agent cells change only through an explicit
correction; Todoist cells refresh only while still Todoist-sourced. Completion,
drop, and pull deletion are monotonic actions with reversible history, never
hard deletion. Every mutation requires `expected_version`.

Migration `0071_agent_first_tasks.sql` adds direct-`user_id`, foreign-keyed,
RLS-enabled-and-forced tables:

| Table | Purpose |
| --- | --- |
| `task_index` | Projection with entry/version, state, typed dates/cost, contexts, project, pins, provenance, recurrence, source timestamps; partial candidate/project/done indexes and contexts GIN |
| `task_contexts`, `task_context_aliases` | Open registry, aliases, merge lineage |
| `task_projects`, `task_project_aliases` | Registry, paths, interest, aliases |
| `task_corrections`, `task_audit_events` | Immutable field and registry audit facts |
| `task_surface_defaults` | Available contexts by user and surface |
| `task_external_refs` | Idempotent system ids and recurrence occurrence keys |
| `task_checkpoint_links` | Explicit/fallback project attribution |
| `task_settings` | Windows, leads, timezone, and quiet override |
| `task_integration_config` | Owner-only saved Todoist mode and configuration generation |
| `task_sync_state` | Content-free cursor, run/outcome/error, manual request |

Every table has account-cascade behavior and policies based on
`app.current_user_id`; two-user tests cover it. Core entry policies grant
`task.write` only for `.straylight/tasks/%`, never arbitrary memory or chunks.
`rebuild_task_projection` runs in the same transaction after normal upsert and
import; deletion or a non-task current version removes the projection. The
five initial contexts (`phone`, `home`, `errands`, `quick`, `online`) and iOS
`phone,online` / web `online` defaults are seeded per user.

Capabilities become `task.read`, `task.write`, and `integration.manage`.
Read-only credentials get only task.read; ordinary save credentials get read
and write; integration.manage is owner-web-only.

## Deterministic engine

`task_engine` is a side-effect-free Rust module whose inputs include task
snapshot, project state, available contexts, settings, view, and explicit
`as_of`. It returns visibility, tier, order key, reason, and provenance
markers. It cannot read a clock/database or call a model.

Table fixtures cover ready/context-AND/parked/waiting visibility; overdue and
seven-day hard pressure; numeric daily rate then accrued amount then flag age;
three-day soft pressure; explicit/derived hot projects; oldest fallback and
created-at ties; inferred markers; pins; third-snooze parking; and time travel.
`urgent` is tiers 1–2 and may be empty. `next` defaults five/max 25 and reports
remaining/backlog. `triage` defaults ten. `all` requires the literal deliberate
view and cursor pagination. Default page rendering is five unique tasks total,
seven only with pins—not five per card.

## Contexts, projects, corrections, recurrence

Context creation normalizes to lowercase kebab and checks exact aliases,
shared normalized tokens, and small Damerau-Levenshtein edits. A suggestion
blocks creation without `confirm_new`; merge is explicit, transactionally
rewrites task cells as corrections, records the old slug as an alias, archives
the source, and audits. Nothing auto-merges.

Project interest is explicit for 14 days, then derives hot from task/checkpoint
activity within seven days, parked from no activity for 60 days, otherwise
normal. `memory.checkpoint` gains optional `state.project`. Its durable metadata
and Markdown include project and checkpoint state. Without project, registered
hub/repo paths are matched against source/state refs by longest prefix.
`project.state` returns the latest linked checkpoint objective/current state/
next actions/open questions/time plus urgent count, next three, waiting ages,
parked count, interest, and last activity.

Recurrence uses unique `(user, series, occurrence_key)` refs. Straylight RRULE
completion materializes one next occurrence. A Todoist roll-forward upserts the
same key; owner-first completion may pre-materialize a parseable next instance
that a later pull updates. Unparseable source recurrence completes the current
item and enters triage, creating neither a guessed date nor duplicate.

## HTTP and MCP contracts

All routes are under `/v1/workspace`, derive identity only from AuthContext,
enforce typed validation and CSRF for cookie mutations, and return generic
not-found across ownership boundaries.

| Route | Capability |
| --- | --- |
| `POST /tasks/capture`, `PATCH /tasks/{id}` | task.write |
| `GET /tasks/candidates`, `/tasks/corrections`, `/tasks/done-summary` | task.read |
| `GET/POST /contexts`, `POST /contexts/merge`, `PATCH /contexts/{slug}`, `PUT /contexts/available/{surface}` | read/write as applicable |
| `GET /projects`, `GET /projects/{slug}/state`, `PUT /projects/{slug}/interest` | read/read/write |
| `GET/PUT /tasks/settings` | read/write |
| `GET /integrations/todoist/status` | task.read |
| `PUT /integrations/todoist/config`, `POST /integrations/todoist/pull` | integration.manage |

Both hosted and local MCP profiles expose these tools with these exact intent
contracts:

- `task.capture`: capture one or many from raw text, first consulting
  corrections, contexts, and projects; infer project aliases, call→phone,
  errands verbs, needs-Nyx→home, consequential date type, evidenced cost, and
  obvious estimate; source every enrichment; ask at most one clarifying
  question and only for consequential hard/soft ambiguity; never overwrite an
  owner value or return a backlog.
- `task.candidates`: deterministic reasons/provenance, default next/five,
  maximum 25, context AND; urgent/triage are bounded intents and all is used
  only on an explicit owner request; `as_of` is for deterministic testing.
- `task.update`: one sourced correction or one action—complete, reopen,
  snooze, drop, wait_on, unpark, pin_today, unpin, confirm_hard, or
  downgrade_to_soft—with expected version; completion returns done count.
- `task.corrections`: bounded recent enrichment feedback, never learned logic.
- `task.contexts`: list/create/merge/archive/set-available with suggestion
  blocking and explicit audited merge.
- `project.list`: registry and interest, never a task wall.
- `project.state`: checkpoint state, next three, and rollups for one project.
- `project.set_interest`: expected-state hot/normal/parked 14-day override.
- `task.sync_status`: content-free environment gate, saved/effective mode,
  token-configured boolean, last run/outcome/error, and next run; no secrets.

## Guard

The worker evaluates with explicit `as_of` and user timezone. Deduplication keys
are `task-deadline:<id>:7d`, `:48h`, `:due-day`, `task-cost:<id>:set`, and
`task-cost:<id>:week:<local-week>`. Existing inbox/outbox uniqueness gives at
most once. Target `{type:"task",task_ref:"<uuid>"}` routes to
`straylight://task/<uuid>`; APNs text remains generic and content is fetched
after authentication.

Quiet hours delay device delivery but retain the inbox/ledger event. Only an
owner-set or actual Todoist-deadline-sourced hard date inside 24 hours may use
the configured override. Agent/derived and Todoist p1/label-promoted dates show
`inferred — confirm?` and never break quiet hours. Soft dates never push. Real
owner-device delivery is excluded before 07:00 America/Los_Angeles.

## Todoist pull decision

The client targets current unified Todoist API v1, including its separate
date-only deadline and incremental `/api/v1/sync`. Legacy REST v2/Sync v9 are
migration paths, not the new target. A personal token alone cannot provide the
creator-webhook flow without an OAuth app, so v1 polling runs every five
minutes within documented limits plus manual pull.

The client type exposes only sync/read parsing—no create, update, complete,
delete, command, or generic authenticated-request method. Static and recorded
request tests assert that no mutating surface or commands exist.
`todoist-api-token` is retrieved internally only when environment gate and
saved mode allow it, held in a redacted header wrapper, and never logged,
serialized, exported, checkpointed, or returned. Production enablement checks
secret metadata only. `off` creates no work/backlog; `import_once` runs once per
configuration generation; `pull` retains the prior cursor and changes nothing
on failure. Mapping, precedence, deletion/completion, triage, and recurrence
are exactly product-spec §9.

## iOS and Web under Night Signal

iOS reads through its owner cookie session. A separate opaque Keychain bearer
is attached only to task mutations and notification registration; its complete
capability array is `task.write` plus existing notification-management and
nothing broader. UI action gates inspect exact task.write, never legacy
save/checkpoint `read_only`; without it the data remains visible but view-only.

Today renders the union of conditional Urgent, Next five, Done today, and
context chips above the briefing, capped at five unique rows (seven with pins).
“5 more” is explicit; a later bounded feed is at most 25 and never `all`.
Projects shows registry cards and checkpoint-derived detail. A validated
lowercase UUID `straylight://task/<id>` survives cold login and opens fetched
detail. Completion uses status-green feedback and haptic; long press exposes
actions. The bounded, user-bound cache stays complete-file-protected. No new
entitlement is added.

Web adds the same conditional Urgent/Next/Done/Projects cards, accessible
reasons/provenance and defensive global cap. Explicit 5-more then `/tasks`
Show all provides server pagination and filters. Settings adds Contexts,
Engine, Todoist, and Operations; Todoist separately displays environment gate,
saved mode, effective state, and token-configured boolean. Alerts handles task
targets. Styles use existing tokens: danger for hard, warning for cost,
signal-soft for provenance, status green—not brand blue—for completion, and
reduced motion.

## Telemetry and threat model

Metrics are content-free and bounded to view, tier, outcome/error code, or
corrected field name: latency, tier counts, guard publications, pull outcomes/
duration, context mint/merge, and corrections. No titles, notes, slugs,
external ids, or secrets enter metrics/logs. The panel shows only timestamps
and codes.

Controls include forced RLS/direct ownership; exact scoped capability and
constant-time token checks; CSRF; indistinguishable cross-user ids; sanitized
Markdown; strict UUID deep links; fixed HTTPS Todoist origin/redacted errors;
transactional import validation/projection repair; and idempotent guard,
external-ref, and occurrence keys. Secret canaries scan entries, projections,
chunks, errors, exports, logs, and checkpoints.

## Test and release plan

Gates 1–10 are engine tables; bounded React/Swift tests; 2,000-task p95 and
EXPLAIN; context/merge audits; precedence/corrections; two-user RLS plus every
mutation denial and secret canary; byte-exact history export/import plus
`memory.changes`; guard ledger/typed route; recorded Todoist v1 fixture; and
Night Signal token/contrast audits. Gate 11 runs all Rust/API/database, MCP,
production contract, retrieval fingerprint, Web, iOS, diff, and added-line
credential checks at every green milestone.

Gate 12 is automated against one disposable API, worker, PostgreSQL, object
store, MCP, browser, and simulator stack:

| Scenario | Required recorded proof |
| --- | --- |
| 12a | MCP capture→reasoned Next→complete→Done; context block; correction; explicit and path-fallback checkpoint project state |
| 12b | `as_of` 7d/48h/day once-only inbox, typed route, quiet suppression, inferred marker/no override |
| 12c | Real browser sign-in; conditional Urgent; globally capped Next/reasons/provenance; complete/snooze/confirm; 5-more/all/filter/page; context merge; Todoist kill switch |
| 12d | Same-stack simulator XCUITest: Today/actions/chips/deep link and exact-capability view-only/write variants |
| 12e | Recorded API fixture twice, mapping, precedence, completion/deletion, both recurrence paths, kill switch, no mutation surface |
| 12f | Task/history byte-exact round trip and task changes in `memory.changes` |
| 12g | Exact production readiness revision, hosted tool list, hosted capture visible with reason and dashboard-completed/dropped, zero API/Web 5xx |

Evidence is JUnit/JSON, Playwright trace/screenshots, Xcode result bundle, SQL
plans, export hashes, deployment ids, and content-free log queries. Unit tests
or a diff never substitute for a scenario.

Only a single revision passing gates 1–11 and 12a–g deploys. Migration precedes
API, worker, hosted MCP profile, and Web; readiness and smoke follow each.
Todoist remains off absent secret metadata. Rollback disables Todoist/APNs,
rolls services back, and preserves canonical task versions; it never writes to
Todoist.

## References

- `sources/Projects/Straylight/Agent-first tasks - spec and Codex handoff - 2026-08-26.md`
- [Night Signal](../Brand.md)
- [D12](D12-operational-simplification.md)
- [D13](D13-client-integration-and-canaries.md)
- <https://developer.todoist.com/api/v1/>
