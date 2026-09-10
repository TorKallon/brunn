# Dreamer subject research

Implementation design, September 9, 2026. Reviewed by Astra Ultra before implementation.

## Behavior

The existing ChatGPT-backed runner researches one canonical person, project or topic at a time. It can repeatedly search and follow exact references, retain evidence and unfinished leads, produce a useful overview despite isolated gaps, and continue within or across nightly runs. Sources remain in the existing entry/version store. Review and publication remain separate from research.

The design review requires explicit canonical summary selection, shared checks for newly relevant evidence, replay safety across interleaved rounds, fair scheduling and strict location isolation.

## Server protocol

The normal admission advertises `research_protocol: 1`. Existing clients and legacy narrative/location contracts continue to work. All mutations require the existing runner credential, active attempt fence and state CAS. Operation IDs are UUIDs. Manual runs can supply at most 16 exact `requested_subject_refs`; the scheduler retains those identities until an accepted proposal or supported no-change disposition, including across selection crashes and yields.

`research-next` accepts the ordinary attempt envelope plus `operation_id`. It selects a due canonical subject and returns ordinary admission fields plus `research`, or `research: null` when the selected work is exhausted. A research object contains `subject_ref`, `subject_path`, `title`, `output_path`, `version`, `snapshot_generation`, `sources` (exact Input headers), `reviewed_sources`, `notes`, `pending_queries`, `pending_targets`, `status` and `round`. The global state retains only active research identity and bounded scheduling/replay bookkeeping. Subjects persist at `dreams/research/<canonical UUID>.md`, using protected server-owned metadata `dreamer_research`.

`narrative-discover` retains its legacy contract and additionally accepts `subject_ref`, `research_version`, `operation_id`, `queries` and `targets`. Targets are source paths or entry references supplied by the model after reading source evidence. Exact identities take precedence. When an exact path is absent, directory-preserving variants can resolve an imported wiki path with the `sources/` prefix and `.md` or `.markdown` extension. All fallback variants must identify exactly one accessible entry; qualified paths never fall back to a basename in another directory. Absolute paths and external URLs remain unresolved. The original target is reported resolved only after its selected identity passes the normal source and capacity checks. Each round rechecks permissions/current versions, retains its exact evidence snapshot, and can acquire newer sources without changing the location/attempt frozen generation. It returns the refreshed admission/research object. Repeated normalized queries and unchanged targets are no-ops; changed evidence permits retry. Every operation ID is payload-bound and remains replayable after intervening operations.

`research-progress` accepts `subject_ref`, `research_version`, `operation_id`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets` and `status` (`researching`, `waiting`, `no_change`). Conclusions are compact, source-backed work notes, never private reasoning. Reviewed selectors must resolve in admitted evidence. Restore notes only after dependency validation. Waiting/no-change ends that subject's turn; later changes or a retry time can make it due again.

Discovery uses ordinary Brunn search anchors and bounded lexical fallback pairs instead of requiring every word in a natural-language query to match. Each fallback retains its own Dreamer-filtered SQL sample. Results are scored against the original query, merged by entry identity, and capped at eight headers per sort. At most six input queries each use two sorts, with up to two anchors and four fallback fetches per sort; exact source admission still enforces permissions and source policy.

Discovery deduplication includes a stable retrieval-policy version. A retained query from an older policy can run once under improved search even when its sources have not changed. Repeating that query under the same policy remains bounded, and exact operation-receipt replay still returns its saved acknowledgement without repeating search or writes.

Research checkpoint ranges use the same end-of-document behavior as source reads: an in-bounds start with a bounded requested end past EOF records the actual last source line and validates the exact excerpt. All sources and notes commit together only after admission, current-version, permission and freshness checks. Empty sources, invalid starts/order and oversized original spans still fail. Candidate and publication citations remain strict, and saved normalized checkpoints are revalidated strictly.

Research checkpoints persist compact selectors (entry reference, version, line range and validated path). Validation reopens exact source bodies, but their hydrated excerpts are omitted from the research record and admission so a broad evidence set cannot fill the notebook with duplicate source text. Existing excerpt-bearing records remain readable and become compact on their next save; immutable historical versions remain unchanged. Candidate and publication evidence still retains its exact hydrated excerpts and existing byte limits.

Model output, saved research notes and historical projection share one 12 KiB UTF-8 byte limit. The prompt targets 4,000–6,000 bytes to leave room for multibyte text; an oversized response reports the actual byte count and limit for correction. Notes are never automatically truncated, and ordinary exact-source validation still applies before accepting any work.

Records explicitly typed `briefing_edition` are generated presentation output, excluded from Dreamer primary evidence even when an older edition lacks a schema field. Ordinary notes under `Briefings/` and unrelated `metadata.briefing` annotations remain eligible. Retained inputs are rechecked through bounded exact-version metadata reads and exclusions are recorded without claiming model processing. Research refresh removes excluded headers; immutable candidates or summaries that depended on them remain invalid until regenerated. Change scans exclude generated editions on both sides before pagination, while preserving ordinary-to-generated and generated-to-ordinary transitions. Raw briefing reads, edits and publication retain their existing behavior.

Dreamer header search uses migration 0098's `dreamer_lexical_candidates` function, excluding generated editions before recent-entry, chunk-sampling and result limits. Ordinary workspace search keeps its existing function and behavior. Deploy the migration through the worker before the API that calls it. Historical location lookup and retained context apply the same evidence rule; existing Review candidates are checked before approval or application, including those without subject scopes.

The existing `candidates` endpoint additionally accepts top-level `subject_ref`, `research_version`, `operation_id` and optional `research_progress`. Every subject candidate carries `subject_ref`; the server attaches a validated `subject_scope`. Summary destination must equal the job's stable `output_path`, and it must cite the canonical source. The research view supplies `output_version` directly. Candidate validation, accepted review identity, research progress and replay receipt commit atomically. Subject evidence is authorized by the job's admitted exact versions and snapshot, not by the original input batch. Legacy/location sources retain their existing fences. Approved/deferred/rejected candidates cannot be silently replaced. Evidence changes invalidate a stale subject approval to `needs_changes`, preserving its bytes, hash and decision history until a newly reviewed revision is approved.

## Research loop

Use one model response envelope, `dream.research.step.v1`, with `action` (`discover`, `submit`, `yield`, `done`), `queries`, `targets`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets`, `candidates`, `processed_inputs`, and `findings`. Empty arrays are permitted. `submit` uses the existing candidate format and validators. `discover` requests another source round; `yield` retains a specific unresolved need; `done` records a supported no-change result. The wrapper forwards precise validation feedback for bounded local correction before yielding a repeatedly failing subject.

Before submitting, the model makes a focused later-outcome check for unresolved information material to the current overview, unless equivalent valid research is already retained. Queries use the subject and current-state domain rather than requiring an older plan's date or vocabulary; returned exact sources must be read. Negative or capped results do not prove absence. This check should lead to a supported overview with dated state and local uncertainty, while peripheral leads remain pending; it does not require resolving every caveat or repeating equivalent searches.

If a discover step's checkpoint is definitively rejected because a cited source changed (`409 dreamer_source_changed`) or subject coverage needs refresh (`400 research_refresh_required`), the runner still performs that step's requested discovery. It sends a new fenced discovery operation containing only queries and targets, retains no rejected conclusions, and tells the next model turn to reread the refreshed evidence. These two typed errors are recognized only on `research-progress`. Other validation, permission, fencing and uncertain transport failures retain their existing handling. Repeated refresh rejections count toward the repair limit; discovery success alone does not reset them, while an accepted evidence checkpoint does.

Continue while time, quota and useful progress remain. Checkpoint each meaningful round and deduplicate unchanged work. Give each subject at most ten minutes or half the remaining run time, preserving room for other work. Seed direct People notes and registered project hubs with durable pagination, then use existing pending input for other topics. Persist service order and requested priorities across restarts. Expose model rounds, reconciled change pages, completed subject passes and continuations separately from remaining historical backlog. Omitted progress fields preserve saved conclusions and leads; an explicit empty value clears them.

At research-loop entry, reserve a fixed selection margin of the smaller of sixty seconds or one tenth of the time remaining. A new subject must have more than that margin within its allocation (the smaller of ten minutes or half the remaining run). Recheck after selection latency, before each fresh model invocation, and after discovery or repair checkpoint writes. An undersized invocation yields its saved work; an undersized whole-run tail stops selection. Always merge an acknowledged selection response first, and honor an exhausted queue before applying the second time check. A selected subject stays durably available without an extra waiting checkpoint when the model has not started. Already dispatched operations retain their existing timeout, failure and replay handling.

Before each invocation, the prompt reports the approximate remaining subject time from the same duration passed to the model executor. This runner-authored planning context sits outside the source input and includes later discovery or correction rounds within the existing subject deadline. It asks the model to reserve time for exact reads and a complete response, submitting a useful supported overview when ready or preserving unresolved work through the existing actions. Another round may occur in a later attempt. The hint does not extend deadlines, force submission, change durable state or relax validation.

Actionable response, candidate and checkpoint validation errors are retained as optional `research.repair_feedback`, separate from factual notes and citations. The wrapper saves only a bounded phase and public message through an operational-only `research-progress` request before a corrective invocation. Its exact operation receipt confirms custody even when stale-source projection withholds the diagnostic. The request cannot include rejected notes, selectors, candidates, routed work or an input disposition, and still requires the active attempt, research identity, CAS and payload-bound replay checks. The model cannot write this field.

Timeout or successful discovery does not erase a saved correction or reset the rejection count. An accepted reviewed checkpoint clears response/checkpoint corrections; a candidate correction survives those checkpoints until an accepted candidate or supported no-change disposition. Source access or policy loss withholds the correction and clears it before the affected dependency is pruned. Eligible source updates still require ordinary evidence reconciliation. The diagnostic never certifies a fact, consumes work, changes a review decision, or relaxes publication validation.

Exact historical research reads check the same source exclusions against both the original and current dependency metadata. Withholding a cached note or diagnostic does not rewrite its immutable historical record.

When source freshness invalidates working notes, the job retains one server-owned `revalidation_checkpoint` pointing to an existing immutable version of that same notebook. Selection, discovery and a stale waiting checkpoint capture previously accepted work before clearing current conclusions; rejected model output is never captured. The optional field distinguishes an uninitialized legacy job from an explicitly empty or retained pointer. Legacy empty jobs inspect only the latest sixteen earlier versions once, choosing the newest structurally valid notebook without searching around an access failure. Fresh replacement or completion initializes the pointer as empty so later selection cannot resurrect old work.

The runner receives only an audited `research.revalidation_context`, never the raw pointer. Its notes, compact old selectors, unfinished leads and bounded prior discovery coverage are historical context for rechecking. Every original dependency must still pass exact historical access and original/current source-policy checks, including uncited sources removed from the active manifest. The projection validates original selectors against their exact snapshot, is limited to 96 KiB, and never follows an embedded checkpoint pointer. Missing or withheld context leaves ordinary current-source research available.

Historical context does not admit sources, certify current facts, process input or authorize publication. The model must reopen currently admitted versions, reconcile changes, carry forward supported conclusions and unfinished leads, and check existing discovery coverage before repeating historical queries. An accepted progress checkpoint clears the pointer only when it explicitly replaces both notes and reviewed selectors with nonempty current-evidence work, or completes a supported no-change disposition. Candidate requests clear it only when at least one candidate is actually accepted. Empty progress, source refresh, timeout, repair-only writes, rejected or zero-result submissions, and exact replay preserve it. Replay still performs current access checks without rewriting durable state.

## Comparing overlapping inputs

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

## Canonical views and freshness

Published metadata contains server-validated `subject_ref` and `subject_scope`. A canonical source's current-state read prefers its subject overview over a newer narrow summary that merely cites the source. Exact source reads remain exact; current reads fall back to source material when freshness cannot be proven.

A shared helper checks bounded source changes after `subject_scope.checked_generation` against the canonical reference/path/name, verified aliases and every admitted dependency, including uncited sources and literal wiki links. Store server-validated dependency headers separately from claim citations. Check current and previous versions, including deleted/renamed records and removed mentions. Relevant changes are stale; truncated, inaccessible or uncheckable coverage is unchecked. Exclude generated research/run changes before the page limit. Durable reconciliation retains a pinned upper generation and scan cursor; the wrapper drains unfinished pages before model work. Run the checks at candidate intake, application and reads, with full dependency access validation before any freshness early return. Search rank is never proof of absence, and a partial page never advances the published freshness boundary.

Identity names come from the canonical filename and explicit structured metadata `title`, `alias` and `aliases`. Display titles inferred from Markdown section headings do not establish identity. When this derivation changes, existing manifests become stale and retain their exact bytes and dependencies until reviewed again.

Research paths/metadata join protected reads/search/access auditing. Never serve cached conclusions after dependency access loss. Strip all research state from location prompts; preserve the existing location evidence packet and Sunday stop rules.

## Verification and release

Test discovery of a linked primary source absent from initial search; current outcome replacing an old plan; useful overview with one unresolved fact; replay A after B; crash/restart; changed/deleted/renamed/revoked sources; new relevant source outside original directories; canonical overview preference; search/state cap behavior; stuck-subject fairness and location isolation. Verify actual model quality across Radley and unrelated subjects, and measure current-state retrieval against source reads. Retest Sunday.

The disposable production canary exercises subject-name search alongside exact linked-source discovery, then creates and updates a typed generated briefing after publishing a synthetic subject overview. Canonical and direct overview reads must stay fresh, exact briefing reads must remain unchanged, and a later ordinary source correction must still cause current-source fallback.

Deploy API before the compatible runner. Keep the existing dedicated account connected and current owner decisions. This implementation does not implicitly change production CONTROL. Verify real production research progress and concrete Review candidates after deployment.
