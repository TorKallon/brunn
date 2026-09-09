# Dreamer subject research

Implementation design, September 9, 2026. Reviewed by Astra Ultra before implementation.

## Behavior

The existing ChatGPT-backed runner researches one canonical person, project or topic at a time. It can repeatedly search and follow exact references, retain evidence and unfinished leads, produce a useful overview despite isolated gaps, and continue within or across nightly runs. Sources remain in the existing entry/version store. Review and publication remain separate from research.

The design review requires explicit canonical summary selection, shared checks for newly relevant evidence, replay safety across interleaved rounds, fair scheduling and strict location isolation.

## Server protocol

The normal admission advertises `research_protocol: 1`. Existing clients and legacy narrative/location contracts continue to work. All mutations require the existing runner credential, active attempt fence and state CAS. Operation IDs are UUIDs. Manual runs can supply at most 16 exact `requested_subject_refs`; the scheduler retains those identities until an accepted proposal or supported no-change disposition, including across selection crashes and yields.

`research-next` accepts the ordinary attempt envelope plus `operation_id`. It selects a due canonical subject and returns ordinary admission fields plus `research`, or `research: null` when the selected work is exhausted. A research object contains `subject_ref`, `subject_path`, `title`, `output_path`, `version`, `snapshot_generation`, `sources` (exact Input headers), `reviewed_sources`, `notes`, `pending_queries`, `pending_targets`, `status` and `round`. The global state retains only active research identity and bounded scheduling/replay bookkeeping. Subjects persist at `dreams/research/<canonical UUID>.md`, using protected server-owned metadata `dreamer_research`.

`narrative-discover` retains its legacy contract and additionally accepts `subject_ref`, `research_version`, `operation_id`, `queries` and `targets`. Targets are exact source paths or entry references supplied by the model after reading source evidence. Each round rechecks permissions/current versions, retains its exact evidence snapshot, and can acquire newer sources without changing the location/attempt frozen generation. It returns the refreshed admission/research object. Repeated normalized queries and unchanged targets are no-ops; changed evidence permits retry. Every operation ID is payload-bound and remains replayable after intervening operations.

`research-progress` accepts `subject_ref`, `research_version`, `operation_id`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets` and `status` (`researching`, `waiting`, `no_change`). Conclusions are compact, source-backed work notes, never private reasoning. Reviewed selectors must resolve in admitted evidence. Restore notes only after dependency validation. Waiting/no-change ends that subject's turn; later changes or a retry time can make it due again.

The existing `candidates` endpoint additionally accepts top-level `subject_ref`, `research_version`, `operation_id` and optional `research_progress`. Every subject candidate carries `subject_ref`; the server attaches a validated `subject_scope`. Summary destination must equal the job's stable `output_path`, and it must cite the canonical source. The research view supplies `output_version` directly. Candidate validation, accepted review identity, research progress and replay receipt commit atomically. Subject evidence is authorized by the job's admitted exact versions and snapshot, not by the original input batch. Legacy/location sources retain their existing fences. Approved/deferred/rejected candidates cannot be silently replaced. Evidence changes invalidate a stale subject approval to `needs_changes`, preserving its bytes, hash and decision history until a newly reviewed revision is approved.

## Research loop

Use one model response envelope, `dream.research.step.v1`, with `action` (`discover`, `submit`, `yield`, `done`), `queries`, `targets`, `notes`, `reviewed_sources`, `pending_queries`, `pending_targets`, `candidates`, `processed_inputs`, and `findings`. Empty arrays are permitted. `submit` uses the existing candidate format and validators. `discover` requests another source round; `yield` retains a specific unresolved need; `done` records a supported no-change result. The wrapper forwards precise validation feedback for bounded local correction before yielding a repeatedly failing subject.

Continue while time, quota and useful progress remain. Checkpoint each meaningful round and deduplicate unchanged work. Give each subject at most ten minutes or half the remaining run time, preserving room for other work. Seed direct People notes and registered project hubs with durable pagination, then use existing pending input for other topics. Persist service order and requested priorities across restarts. Expose model rounds, reconciled change pages, completed subject passes and continuations separately from remaining historical backlog. Omitted progress fields preserve saved conclusions and leads; an explicit empty value clears them.

## Canonical views and freshness

Published metadata contains server-validated `subject_ref` and `subject_scope`. A canonical source's current-state read prefers its subject overview over a newer narrow summary that merely cites the source. Exact source reads remain exact; current reads fall back to source material when freshness cannot be proven.

A shared helper checks bounded source changes after `subject_scope.checked_generation` against the canonical reference/path/name, verified aliases and every admitted dependency, including uncited sources and literal wiki links. Store server-validated dependency headers separately from claim citations. Check current and previous versions, including deleted/renamed records and removed mentions. Relevant changes are stale; truncated, inaccessible or uncheckable coverage is unchecked. Exclude generated research/run changes before the page limit. Durable reconciliation retains a pinned upper generation and scan cursor; the wrapper drains unfinished pages before model work. Run the checks at candidate intake, application and reads, with full dependency access validation before any freshness early return. Search rank is never proof of absence, and a partial page never advances the published freshness boundary.

Identity names come from the canonical filename and explicit structured metadata `title`, `alias` and `aliases`. Display titles inferred from Markdown section headings do not establish identity. When this derivation changes, existing manifests become stale and retain their exact bytes and dependencies until reviewed again.

Research paths/metadata join protected reads/search/access auditing. Never serve cached conclusions after dependency access loss. Strip all research state from location prompts; preserve the existing location evidence packet and Sunday stop rules.

## Verification and release

Test discovery of a linked primary source absent from initial search; current outcome replacing an old plan; useful overview with one unresolved fact; replay A after B; crash/restart; changed/deleted/renamed/revoked sources; new relevant source outside original directories; canonical overview preference; search/state cap behavior; stuck-subject fairness and location isolation. Verify actual model quality across Radley and unrelated subjects, and measure current-state retrieval against source reads. Retest Sunday.

Deploy API before the compatible runner. Keep the existing dedicated account connected and current owner decisions. This implementation does not implicitly change production CONTROL. Verify real production research progress and concrete Review candidates after deployment.
