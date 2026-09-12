# Dreamer subject research

Revised design, September 10, 2026. After holding implementation during design,
the owner requested a Claude Code/Fable build-and-test handoff. That delegated
implementation/testing may proceed after coordinating ownership; the preparing
Codex session remains documents-only. Deployment, production manual runs and
CONTROL/publication changes are not authorized by this handoff. Deployed code at
`36d1da1` still uses the broader freshness rule described in the implementation
reference below. Use the owner's available Claude Code/Fable allocation for this
development/testing work; do not change Dreamer's deployed model/account/provider.

## Intended behavior

Maintain one useful, dated overview per person, project or topic. Research can
search, follow links, reconcile evidence and resume across runs. Finish supported
work despite peripheral gaps; keep those gaps visible and queued. Source records
and their history remain untouched. Radley is an acceptance example, not a special
case or the boundary of subject coverage.

The product promises best-known understanding with an honest review date and
background maintenance, not a guarantee that nothing related has changed at the
instant of every read. This section supersedes the older all-admitted-source
freshness requirements. It does not supersede source access or owner decisions.

## Evidence and discovery have different jobs

- A search hit, admitted header or followed lead means "consider this." It does
  not automatically make the entire record a dependency of the overview.
- Retain exact source versions supporting or materially qualifying the actual
  conclusions. Reliance includes contradictory evidence, uncertainty and reasons
  for excluding a claim, not just the footnotes selected for display. Do not use
  citation omission to hide reliance on a source.
- A new match or a source version change is a reason to assess an update, not
  proof that the summary is wrong. Neither keyword overlap nor recency alone
  establishes materiality. A capped search cannot prove an exhaustive absence.

Use existing source selectors, research notes and pending changes. These are
behavioral distinctions, not a request for a claim graph, a new dependency
database, or another model-facing receipt protocol. The runner owns mechanical
versions/hashes; the model assesses meaning against exact primary evidence.

## Finish a bounded pass

Set and persist one evidence cutoff when a subject pass starts. Resume that same
pass after interruption; discovery rounds, retries and new writes do not advance
its cutoff or refill its time allowance. Use the existing version store to pin
evidence no later than that cutoff. Search may surface newer leads, but queue
post-cutoff material for the next pass instead of silently mixing it into an
older snapshot. This does not require a copied corpus or an exhaustive snapshot
search index; disclose incomplete discovery and avoid completeness claims.

Acceptance means the overview is supported by its pinned evidence, with material
conflicts encountered within that pass resolved or clearly qualified. Save it as
reviewed through the cutoff; do not require every admitted lead to equal its
latest head. Saving notes and retaining a draft likewise do not require unrelated
sources to stop changing. Completion of this pass does not acknowledge or discard
post-cutoff changes or imply the subject will never need more research.
Peripheral unresolved leads can remain queued after acceptance; they must not
force a false "all research done" disposition or block otherwise supported work.

Newer information cannot move the finish line indefinitely. If a known newer
correction contradicts the proposed overview, retain the dated result but mark
the affected conclusion as needing correction; do not expose it as verified
current. Queue focused follow-up. A fixed cutoff is not permission to ignore an
already known contradiction.

## Refresh what matters

Background change handling retains a bounded list of potentially relevant changes
for the existing subject job. Review the changed evidence against the retained
overview and draft. If it does not alter any conclusion, keep the prose and finish
the review. If it changes or qualifies a conclusion, revise the affected content
while preserving useful work. A whole-document edit is fine; no per-claim storage
engine is required. If significance remains unresolved, say so and retain the
follow-up rather than certifying the claim or repeatedly restarting all research.

The rental-reminder case must finish even while unrelated booking/refund or
delivery fields change. A newly discovered cancellation that contradicts an older
booking claim must still be considered, even if the new note was never cited.
Use semantic evidence, not path/name exceptions for either example. Reuse the
existing scheduling fairness and execution bounds; ordinary lead churn does not
consume a model-repair allowance or count as completed research.

## Useful, honest, fast reads

Serve the saved canonical overview, with its last-reviewed/evidence-cutoff time
distinct from its save/publication time. Show a pending refresh or unresolved
correction when applicable. Pending refresh is not the same as invalid evidence:
it does not by itself remove the useful overview or force fallback to raw notes.
Do not keep displaying a known contradicted conclusion as current. Qualify or
withhold the affected content; if the renderer cannot isolate it, show the dated
overview as needing correction rather than silently treating it as current truth.

Summary retrieval performs no model work, writes no research checkpoints and does
not scan the corpus/change history. Background work maintains refresh status;
foreground reads still enforce actual caller access and bounded evidence
availability checks. Deleted/inaccessible supporting evidence is not merely a
refresh lag: withhold content that cannot be served under the existing access
rules. Do not prune old authority manifests to evade those rules. Legacy summaries
must not be relabeled under the new contract without evidence-backed regeneration.
Exact historical source reads remain exact and access-controlled.

### Daytime answers use newer source evidence

Successful source writes are durable and available to exact/lexical retrieval
without waiting for Dreamer. This means captured information is available, not
that every real-world change was captured or that an older overview has already
been rewritten. The normal schedule is 02:00 America/Los_Angeles with a three-hour attempt budget; startup catch-up
and manual runs are not restricted to the night or gated on the owner being idle.

For a current-state question, the answering agent uses the nightly overview as a
dated baseline and checks relevant newer source evidence with bounded, targeted
search/read. A confirmed correction or later outcome takes precedence over an
obsolete summary claim; a newer timestamp alone does not settle a conflict. If
the relevant update cannot be checked, qualify the answer instead of asserting
that nothing changed. This is ordinary question answering, not a foreground
Dreamer run, a corpus scan, or a new reconciliation service. An explicitly dated
summary can still be read as a snapshot without this current-answer claim.

This consumer behavior is an acceptance requirement, not a claim that the
deployed summary endpoint already merges newer evidence. Its existing stale
summary fallback returns source material, not a reconciled account of all later
changes. Nightly consolidation alone cannot guarantee correct daytime answers.

## Automation and scope

The intended end state is automatic maintenance of reversible **derived
summaries**, with source links and prior versions available. It is not automatic
execution of tasks, edits to original source records, or blanket approval of other
Dreamer actions. Existing owner holds and corrections remain authoritative.

Research acceptance and publication remain separate. After the revised behavior
has been implemented and demonstrated, the owner must explicitly authorize the
limited automatic-publication mode. Until then, keep report-only behavior and
existing decisions unchanged. This design edit does not enable that mode.

Reuse the current runner/account, subject identities, entry/version store,
retained drafts, pending changes and scheduler. Do not add a new memory backend,
graph, checkpoint hierarchy or blanket revalidation of everything ever seen.
Keep location/phone capture and its separate evidence contract out of this change.
The nightly project-status phase is not subject research and produces no
candidates.

## Acceptance and implementation handoff — Claude Code/Fable

Before implementation, agree ownership with Brunn 2 and recheck the working tree.
The owner's Claude Code/Fable handoff releases the earlier hold for that delegated
build/test work, not production rollout. Cover intake, progress saves, draft recovery, Review,
publication and current reads consistently; changing only candidate intake would
leave the same over-broad rule blocking another stage.

The fixed acceptance bar is:

1. Produce useful person, project and topic overviews from identities alone,
   following linked primary evidence and retaining isolated unresolved details.
2. Reproduce the uncited rental-reminder update during drafting and recovery.
   Complete the supported overview without a restart or a subject-specific rule.
3. Introduce a material contradiction in a previously uncited source. Correct or
   qualify the affected conclusion; unchanged citations alone cannot certify it.
4. Interrupt and resume a pass. Preserve the cutoff, useful draft and pending
   post-cutoff changes; accept without duplicate proposals or lost work.
5. Keep accepting bounded passes under continuing nonmaterial writes. Show the
   difference between a completed pass and outstanding refresh/backlog work.
6. Verify source-access loss, owner holds, exact historical reads, source
   preservation, overlapping-summary behavior and location isolation.
7. Repeated unchanged reads return the saved overview with honest status, no
   model calls and no corpus scan. Measure latency against the same raw-read
   baseline; response-byte savings alone do not establish faster answers.
8. Save a material correction after the nightly overview, then ask a current-state
   question before the next run. The answer reflects the correction using the
   existing source retrieval path, without rerunning Dreamer. The dated overview
   remains available and the source correction is preserved.

Use focused deterministic checks plus one bounded integrated acceptance/recovery
pass in an isolated dev/test environment. A production live pass remains a
separate owner-approved step; do not claim it was exercised by isolated tests.
Report actual readable output, accepted revisions, elapsed time, model rounds,
retained work and retrieval measurements. Synthetic canaries and test counts are
supporting checks, not the result. If the same blocker remains, stop and report
the exact failure; do not automatically expand the architecture or repeat
fix/deploy/manual-retry cycles. Existing tests asserting that every uncited lead
change invalidates the whole summary describe the superseded contract, not a
requirement to preserve that behavior.

## Deployed implementation reference — not the revised contract

The notes below retain the earlier implementation design and protocol details
for compatibility work. They were reviewed before that implementation and were
checked against the `36d1da1` baseline during this design revision. Their blanket
freshness/invalidation requirements are superseded by the sections above and
must not be reapplied as acceptance requirements for the revision. Existing
authorization, replay, custody and owner-decision behavior remains the baseline.
See also [Operations.md](Operations.md) for the deployed draft-saving behavior.

### Server protocol

The normal admission advertises `research_protocol: 1`. Existing clients and legacy narrative/location contracts continue to work. All mutations require the existing runner credential, active attempt fence and state CAS. Operation IDs are UUIDs. Manual runs can supply at most 16 exact `requested_subject_refs`; the scheduler retains those identities until an accepted proposal or supported no-change disposition, including across selection crashes and yields.

`research-next` accepts the ordinary attempt envelope plus `operation_id`. It selects a due canonical subject and returns ordinary admission fields plus `research`, or `research: null` when the selected work is exhausted. A research object contains `subject_ref`, `subject_path`, `title`, `output_path`, `version`, `snapshot_generation`, `sources` (exact Input headers), `reviewed_sources`, `notes`, `pending_queries`, `pending_targets`, `status` and `round`. The global state retains only active research identity and bounded scheduling/replay bookkeeping. Subjects persist at `dreams/research/<canonical UUID>.md`, using protected server-owned metadata `dreamer_research`.

`narrative-discover` retains its legacy contract and additionally accepts `subject_ref`, `research_version`, `operation_id`, `queries` and `targets`. Targets are source paths or entry references supplied by the model after reading source evidence. Exact identities take precedence. When an exact path is absent, directory-preserving variants can resolve an imported wiki path with the `sources/` prefix and `.md` or `.markdown` extension. All fallback variants must identify exactly one accessible entry; qualified paths never fall back to a basename in another directory. Absolute paths and external URLs remain unresolved. The original target is reported resolved only after its selected identity passes the normal source and capacity checks. Each round rechecks permissions/current versions, retains its exact evidence snapshot, and can acquire newer sources without changing the location/attempt frozen generation. It returns the refreshed admission/research object. Repeated normalized queries and unchanged targets are no-ops; changed evidence permits retry. Every operation ID is payload-bound and remains replayable after intervening operations.

`research-progress` accepts `subject_ref`, `research_version`, `operation_id`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets` and `status` (`researching`, `waiting`, `no_change`). Conclusions are compact, source-backed work notes, never private reasoning. Reviewed selectors must resolve in admitted evidence. Restore notes only after dependency validation. Waiting/no-change ends that subject's turn; later changes or a retry time can make it due again.

Discovery uses ordinary Brunn search anchors and bounded lexical fallback pairs instead of requiring every word in a natural-language query to match. Each fallback retains its own Dreamer-filtered SQL sample. Results are scored against the original query, merged by entry identity, and capped at eight headers per sort. At most six input queries each use two sorts, with up to two anchors and four fallback fetches per sort; exact source admission still enforces permissions and source policy.

Dreamer search checks each candidate’s exact current source policy in the same statement snapshot as its lexical fetch, before treating an anchor as found or selecting the final eight headers. Static exclusions do not invalidate a completed search audit. An emitted hit that changes or becomes inaccessible before admission still invalidates the audit. The upstream 64-entry sample per fetch remains bounded and may underfill after filtering; ordinary workspace search is unchanged.

Discovery deduplication includes a stable retrieval-policy version. A retained query from an older policy can run once under improved search even when its sources have not changed. Repeating that query under the same policy remains bounded, and exact operation-receipt replay still returns its saved acknowledgement without repeating search or writes.

The current research view also carries a bounded, server-owned `discovery_audit`. It identifies the latest actual query batch, the retrieval policy, its search generation, and the result counts and limits for each query/sort. Validity is `current`, `outdated`, or `legacy_or_unknown`; only a relevant current audit can justify reusing a prior search. Counts and a notebook's claim that a search completed do not establish current search coverage. Output limits and internal sampling mean even a completed query with few results can miss evidence.

An audit persists through operations that perform no search, while source, scope or historical-authority changes invalidate it. Its generation precedes actual search, with bounded relevant-change checks through commit and read; unrelated/generated changes do not by themselves invalidate it. A query-bearing duplicate request requires a matching current latest audit. An old hash without an audit, or an A query after its audit was replaced by B, executes again. Exact operation replay still executes no search. The audit is excluded from historical checkpoint projection and does not count as source review or runner progress.

The audit retains its original historical notebook versions independently of the current unfinished-work list. Adding or reconciling a checkpoint does not invalidate a completed search. Every audit read and reuse still validates those exact original versions and their complete source permissions and policy, including after retirement. New searches capture the history offered at that search; already invalidated records require a real search before becoming current again.

Research checkpoint ranges use the same end-of-document behavior as source reads: an in-bounds start with a bounded requested end past EOF records the actual last source line and validates the exact excerpt. All sources and notes commit together only after admission, current-version, permission and freshness checks. Empty sources, invalid starts/order and oversized original spans still fail. Candidate and publication citations remain strict, and saved normalized checkpoints are revalidated strictly.

Research checkpoints persist compact selectors (entry reference, version, line range and validated path). Validation reopens exact source bodies, but their hydrated excerpts are omitted from the research record and admission so a broad evidence set cannot fill the notebook with duplicate source text. Existing excerpt-bearing records remain readable and become compact on their next save; immutable historical versions remain unchanged. Candidate and publication evidence still retains its exact hydrated excerpts and existing byte limits.

Model output, saved research notes and historical projection share one 12 KiB UTF-8 byte limit. The prompt targets 4,000–6,000 bytes to leave room for multibyte text; an oversized response reports the actual byte count and limit for correction. Notes are never automatically truncated, and ordinary exact-source validation still applies before accepting any work.

Records explicitly typed `briefing_edition` are generated presentation output, excluded from Dreamer primary evidence even when an older edition lacks a schema field. Ordinary notes under `Briefings/` and unrelated `metadata.briefing` annotations remain eligible. Retained inputs are rechecked through bounded exact-version metadata reads and exclusions are recorded without claiming model processing. Research refresh removes excluded headers; immutable candidates or summaries that depended on them remain invalid until regenerated. Change scans exclude generated editions on both sides before pagination, while preserving ordinary-to-generated and generated-to-ordinary transitions. Raw briefing reads, edits and publication retain their existing behavior.

Dreamer header search uses migration 0098's `dreamer_lexical_candidates` function, excluding generated editions before recent-entry, chunk-sampling and result limits. Ordinary workspace search keeps its existing function and behavior. Deploy the migration through the worker before the API that calls it. Historical location lookup and retained context apply the same evidence rule; existing Review candidates are checked before approval or application, including those without subject scopes.

The existing `candidates` endpoint additionally accepts top-level `subject_ref`, `research_version`, `operation_id` and optional `research_progress`. Every subject candidate carries `subject_ref`; the server attaches a validated `subject_scope`. Summary destination must equal the job's stable `output_path`, and it must cite the canonical source. The research view supplies `output_version` directly. Candidate validation, accepted review identity, research progress and replay receipt commit atomically. Subject evidence is authorized by the job's admitted exact versions and snapshot, not by the original input batch. Legacy/location sources retain their existing fences. Approved/deferred/rejected candidates cannot be silently replaced. Evidence changes invalidate a stale subject approval to `needs_changes`, preserving its bytes, hash and decision history until a newly reviewed revision is approved.

### Research loop

Use one model response envelope, `dream.research.step.v1`, with `action` (`checkpoint`, `discover`, `submit`, `yield`, `done`), `queries`, `targets`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets`, `candidates`, `processed_inputs`, and `findings`. `checkpoint` is offered only when the API advertises `research.checkpoint_protocol: dream.research.checkpoint.v1`. It saves a nonempty source-backed partial result through `research-progress` and continues without a discovery request. It cannot submit candidates, process inputs or route/retire proposals. `submit` uses the existing candidate format and validators. `discover` requests missing evidence; `yield` retains a specific unresolved need; `done` records a supported no-change result. The wrapper forwards precise validation feedback for bounded local correction before yielding a repeatedly failing subject.

Before submitting, the model makes a focused later-outcome check for unresolved information material to the current overview. It may reuse an equivalent check only with a relevant current discovery audit, or an exact current primary-source link it followed and read. Queries use the subject and current-state domain rather than requiring an older plan's date or vocabulary; returned exact sources must be read. Negative or capped results do not prove absence. This check should lead to a supported overview with dated state and local uncertainty, while peripheral leads remain pending; it does not require resolving every caveat or repeating verified equivalent searches.

If a progress save is definitively rejected because a cited source changed (`409 dreamer_source_changed`) or subject coverage needs refresh (`400 research_refresh_required`), a discover step still performs its requested discovery; a checkpoint step performs a header refresh with empty queries and targets. The new fenced discovery operation carries no rejected conclusions or dispositions. The next model turn receives the safe current admission and retained correction. These two typed errors are recognized only on `research-progress`. Other validation, permission, fencing and uncertain transport failures retain their existing handling. Repeated refresh rejections count toward the repair limit; discovery success alone does not reset them, while an acknowledged accepted evidence checkpoint does.

Continue while time, quota and useful progress remain. Ask the model to return a small useful unit of source review promptly, then continue from its acknowledged checkpoint. Give each subject at most twenty minutes or half the remaining run time, preserving room for other work. This fixed allowance covers discovery, saved source review and final composition; progress does not refill it. The larger cap addresses observed model-round timeouts after successful source review, at the cost of fewer difficult subjects per hour. Seed direct People notes and registered project hubs with durable pagination, then use existing pending input for other topics. Persist service order and requested priorities across restarts. Expose model rounds, reconciled change pages, completed subject passes and continuations separately from remaining historical backlog. Omitted progress fields preserve saved conclusions and leads; clearing or replacing current notes does not implicitly retire earlier unfinished checkpoints.

At research-loop entry, reserve a fixed selection margin of the smaller of sixty seconds or one tenth of the time remaining. A new subject must have more than that margin within its allocation (the smaller of twenty minutes or half the remaining run). Recheck after selection latency, before each fresh model invocation, and after discovery or repair checkpoint writes. An undersized invocation yields its saved work; an undersized whole-run tail stops selection. Always merge an acknowledged selection response first, and honor an exhausted queue before applying the second time check. A selected subject stays durably available without an extra waiting checkpoint when the model has not started. Already dispatched operations retain their existing timeout, failure and replay handling.

Before each invocation, the prompt reports the approximate remaining subject time from the same duration passed to the model executor. This runner-authored planning context sits outside the source input and includes later discovery or correction rounds within the existing subject deadline. It asks the model to reserve time for exact reads and a complete response, submitting a useful supported overview when ready or preserving unresolved work through the existing actions. Another round may occur in a later attempt. The hint does not extend deadlines, force submission, change durable state or relax validation.

Failed Codex invocations use bounded top-level JSONL error events to distinguish explicit account usage exhaustion from transient HTTP rate limiting, authentication, context-window, connection and provider failures. Source, tool and reasoning items, stderr and arbitrary `quota` or `429` substrings cannot establish a limit. A later terminal failure supersedes earlier retry errors; successful completion clears them. Unknown or malformed diagnostics remain unclassified execution failures. Only explicit usage exhaustion stops research with `account_limits`; other failures keep the existing bounded retry behavior.

Probe and subject-research failures retain up to eight recent records containing only a runner-generated stage, elapsed time, fixed category, event kind and numeric HTTP/exit status. Raw messages, source content, URLs and credentials are never retained in these diagnostics. Terminal receipts and owner Review receive the latest whole diagnostic records within the existing 2,000-character detail field; older runtime reports remain readable. This reporting change does not alter model, account, deadlines, evidence checks or publication permissions.

Actionable response, candidate and checkpoint validation errors are retained as optional `research.repair_feedback`, separate from factual notes and citations. The wrapper saves only a bounded phase and public message through an operational-only `research-progress` request before a corrective invocation. Its exact operation receipt confirms custody even when stale-source projection withholds the diagnostic. The request cannot include rejected notes, selectors, candidates, routed work or an input disposition, and still requires the active attempt, research identity, CAS and payload-bound replay checks. The model cannot write this field.

Timeout or successful discovery does not erase a saved correction or reset the rejection count. An accepted reviewed checkpoint clears response/checkpoint corrections; a candidate correction survives those checkpoints until an accepted candidate or supported no-change disposition. Source access or policy loss withholds the correction and clears it before the affected dependency is pruned. Eligible source updates still require ordinary evidence reconciliation. The diagnostic never certifies a fact, consumes work, changes a review decision, or relaxes publication validation.

Exact historical research reads check the same source exclusions against both the original and current dependency metadata. Withholding a cached note or diagnostic does not rewrite its immutable historical record.

Protocol-aware writes upgrade a notebook atomically to `dream.research.v2`. Server-owned `checkpoint_versions` names exact immutable versions of that same notebook. At most four unfinished units fit, including the nonempty current head, within a combined 96 KiB potential projection. Every save reserves capacity, including bounded growth in discovery/operational fields, before any future refresh. Refresh moves an already reserved current head into history before clearing stale conclusions. A partial replacement preserves the prior head unless explicitly reconciled, retaining independently unfinished work across repeated refreshes. Overflow rejects the whole transaction without eviction; operational repair/waiting remains available.

The new API exposes audited `research.checkpoint_contexts`, `checkpoint_context_status` and an exact `current_checkpoint` origin. Each origin has `entry_ref`, `version` and `snapshot_generation`. Every historical unit uses its own original full dependency manifest and exact source snapshot, including uncited dependencies later removed from the active manifest. Embedded checkpoint pointers are never traversed. If one historical unit fails access or policy checks, the whole historical collection is withheld and retained. An independently audited fresh current head remains usable. Historical context never admits sources, certifies current facts, processes input or authorizes publication.

Optional `reconciled_checkpoints` names at most four distinct, currently offered origins. Nonempty findings must explicitly say how their useful conclusions and unfinished leads were incorporated or deliberately superseded using current evidence. This is a model judgment; the server proves identity, visibility, exact evidence, bounds and atomic custody. Only named units retire. Unchanged current accepted selectors can carry forward for checkpointing, with current validation; historical claims and candidate claims still require exact primary-source reads. Done/no-change must reconcile all remaining work. An accepted partial candidate retains unresolved history, priorities and immediate retry eligibility; zero accepted candidates retire no units. Invalid reconciliation rejects the entire request, including candidate and input effects.

Every protocol-aware semantic progress/candidate save requires an exact `checkpoint_receipt` containing operation identity, protocol, recorded status, mechanical progress flags and `subject_complete`. Missing or malformed acknowledgment stops uncertain continuation. Replay rechecks visibility, changes no custody and provides no new-progress credit. Across a subject turn, the runner compares accumulated exact line coverage so reordered, split or alternating selectors and cosmetic note changes cannot restart its budget. Actual reduction of unresolved units also counts. Existing two-repair/two-no-progress, 32-round and time/fairness bounds remain.

Legacy v1 clients retain the singular `revalidation_context` contract until upgrade; absent versus explicitly empty legacy pointers remain distinct. The existing one-time sixteen-version legacy lookup imports at most one unit and never searches around an access failure. An old runner against the new API cannot erase v2 history by omitting reconciliation. An old API rejects `dream.research.v2` before writing, preventing rollback from silently dropping unknown custody fields. Deploy API first, then runner; do not roll the API back below v2 support after jobs upgrade.

### Comparing overlapping inputs

Separate imported files can describe the same subject or successive snapshots
of one dataset. Admission therefore supplies `comparison_proposals`: at most
four complete, accessible, fresh pending ordinary subject summaries, within a
64 KiB serialized bound. Exact cited primary-source version overlap retrieves
comparisons, with a citation of the selected canonical source ranked first.
Overlap does not itself establish duplication. Oversized comparisons are
omitted rather than truncated; absence from this bounded list is not proof of
absence from the workspace.

Each comparison carries its body, compact original source selectors, canonical
identity and exact `pointer` (`item_id`, `candidate_hash`, `run_entry_ref`,
`run_version`). Generated prose remains untrusted comparison context, outside
admitted evidence and dependency manifests. The model must reopen primary
sources before using any claim. Location proposals and owner-held decisions
are excluded.

For already-covered work, `done` may supply `covers_existing`. The progress
transaction revalidates the exact comparison, availability, freshness and
direct citations of the selected canonical source and every processed input
at its admitted version. A concurrent correction or owner decision cannot be
bypassed by retrying a state CAS. Ordinary source-based `done` is unchanged.

For useful enrichment of an existing overview, `yield` may supply `follow_up`
with the exact comparison pointer, one retained and explicitly reviewed
`origin_input`, and up to sixteen admitted primary target references including
the origin. The server retains the input, records a bounded identity-only
route, and queues the destination's existing canonical subject. It does not
change either canonical identity, proposal bytes, destination or approval.
Self-routing, cycles, conflicting destinations and capacity failures retain
all work atomically.

When `research.follow_up_protocol` advertises `dream.research.follow_up.v1`,
the same route may instead use `origin_source: {entry_ref, version}` from
current admitted and explicitly reviewed primary evidence. Exactly one origin
kind is allowed. This supports consolidation of existing proposals when no new
input event exists. The source route does not create or resurrect an input and
never increments processed-input counts. It remains in the same durable
follow-up collection, with the originating subject and snapshot recorded by
the server. Older APIs cannot deserialize a retained source route and fail
closed rather than discard it.

Source routes have server-issued `route_id` and `revision` values. An offered
completion token also binds the current destination comparison pointer.
`resolved_follow_ups` may acknowledge these exact tokens only with `submit`
or `done`, accompanied by a disposition finding. The server validates the
tokens before changing a candidate and resolves work only after an accepted
destination summary cites the current origin, or a fresh source-backed
no-change result. Both require review of the current origin/replacement and
every routed target. A pending stale destination can receive the revision
that repairs it; it cannot justify no-change. Merging targets changes the
route revision, so an old token cannot discard added work.

Missing acknowledgments, source access uncertainty, rejected or zero-ID
submissions and operational checkpoints retain source routes. Resolution
returns operation-bound evidence and preserves the original route, reviewed
replacement and destination in its immutable disposition. After enrichment,
the originating retained job becomes due for an ordinary later comparison,
without resetting its attempt fence. Duplicate retirement remains a separate
whole-draft decision; routing never selects a survivor automatically.

Ordinary current and historical state/run reads omit internal follow-up routes
and route-bearing dispositions. Those records may reference primary evidence
outside a proposal's dependency manifest. The current typed research view
checks route access before exposing actionable work; projection does not
delete the retained server records.

Routes live independently of model-written pending leads. Admission exposes
safe `research.routed_work` and `research.routed_targets`; the wrapper sends
missing references through normal discovery before model work, once per
reference per bounded subject turn. Discovery, cleared notes, restart or a
submission omitting the routed origin cannot complete the route or remove its
priority. An accepted destination summary must cite and explicitly disposition
the exact originating input, or a supported `no_change` must disposition it
while the current destination remains pending, accessible and fresh.
A reviewed current replacement can resolve a changed original, with both
identities retained in an immutable disposition. Owner holds and unavailable
evidence retain unresolved work. A deterministic intake exclusion for a
deleted, generated or sensitive origin can release its route only when no
eligible retained replacement exists. Its immutable `excluded_by_source_policy`
record preserves the route and exclusion reason with `model_processed: false`;
it does not count as completed research. Independently requested subject
priorities survive that cleanup. The once-per-attempt scheduling fence still
applies.

Existing duplicate drafts require a separate whole-draft comparison. With
`done` and `covers_existing`, the model may provide `supersedes_existing` for
a distinct complete proposal of the selected job that was offered as
`retirable`. Useful additions must first reach the surviving view. The server
rechecks both exact identities and source scope and proves the duplicate has
never been owner-reviewed using all durable decision audit versions, not
compact recent history. Incoming enrichment must be resolved before a later
step can retire its destination. It changes only the duplicate's status to
`superseded`, preserves its bytes, hash and run references, and records a
server disposition naming both proposals. Superseded cards reject subsequent
decisions, including from an old mobile view. This routes work and removes
proven redundancy without defining permanent equivalence between source
identities.

### Canonical views and freshness (superseded policy)

Published metadata contains server-validated `subject_ref` and `subject_scope`. A canonical source's current-state read prefers its subject overview over a newer narrow summary that merely cites the source. Exact source reads remain exact; current reads fall back to source material when freshness cannot be proven.

A shared helper checks bounded source changes after `subject_scope.checked_generation` against the canonical reference/path/name, verified aliases and every admitted dependency, including uncited sources and literal wiki links. Store server-validated dependency headers separately from claim citations. Check current and previous versions, including deleted/renamed records and removed mentions. Relevant changes are stale; truncated, inaccessible or uncheckable coverage is unchecked. Exclude generated research/run changes before the page limit. Durable reconciliation retains a pinned upper generation and scan cursor; the wrapper drains unfinished pages before model work. Run the checks at candidate intake, application and reads, with full dependency access validation before any freshness early return. Search rank is never proof of absence, and a partial page never advances the published freshness boundary.

Identity names come from the canonical filename and explicit structured metadata `title`, `alias` and `aliases`. Display titles inferred from Markdown section headings do not establish identity. When this derivation changes, existing manifests become stale and retain their exact bytes and dependencies until reviewed again.

Research paths/metadata join protected reads/search/access auditing. Never serve cached conclusions after dependency access loss. Strip all research state from location prompts; preserve the existing location evidence packet and Sunday stop rules.

### Earlier verification and release baseline

Test discovery of a linked primary source absent from initial search; current outcome replacing an old plan; useful overview with one unresolved fact; replay A after B; crash/restart; changed/deleted/renamed/revoked sources; new relevant source outside original directories; canonical overview preference; search/state cap behavior; stuck-subject fairness and location isolation. Verify actual model quality across Radley and unrelated subjects, and measure current-state retrieval against source reads. Retest Sunday.

The disposable production canary exercises subject-name search alongside exact linked-source discovery, then creates and updates a typed generated briefing after publishing a synthetic subject overview. Canonical and direct overview reads must stay fresh, exact briefing reads must remain unchanged, and a later ordinary source correction must still cause current-source fallback.

Deploy API before the compatible runner. Keep the existing dedicated account connected and current owner decisions. This implementation does not implicitly change production CONTROL. Verify real production research progress and concrete Review candidates after deployment.
