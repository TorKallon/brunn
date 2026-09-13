# Task timing and task surfaces

Implementation contract for the September 12 task-view requirements. Additive to
`task.v1`; no backlog relabeling, new priority taxonomy, or second scheduler.

## Capture and storage

Keep `soft_due` (intend to do), `hard_due` (actual cutoff), `ready_at` (resurface),
and `cost_of_delay` (ongoing monetary cost). These can coexist. A desired date is
not evidence of a hard deadline. Original capture text and sourced cells remain.

New optional sourced `consequence` value:

```json
{
  "description": "Plants may be lost if care slips",
  "severity": "serious",
  "timing": { "kind": "after_due", "days": 2 }
}
```

Severity is `ordinary` or `serious`, not a probability score. Timing is optional:
unknown means unspecified, not harmless. `after_due.days` (0–3650) is an explicit
tolerance relative to this occurrence's `soft_due`. Alternatively use
`{"kind":"window","starts_on":"2026-09-15","ends_on":"2026-09-17"}`;
`ends_on` is optional. This preserves an evidenced window, not a made-up instant
of harm. Calendar dates use the configured task timezone, or the native routine's
explicit timezone. Use the existing `hard_due` for an actual cutoff; do not copy
that cutoff into a second field. Agents must not invent severity, cadence, dates,
or ownership. Ask for consequential missing facts, not a mandatory questionnaire.

Native sourced `recurrence`:

```json
{"kind":"native","mode":"after_completion","every_days":14,"timezone":"America/Los_Angeles"}
```

`calendar` additionally requires `anchor_on` (YYYY-MM-DD). `every_days` is
1–3650. This deliberately supports day/week cadence, not an invented general
RRULE interpreter. Existing Todoist recurrence keeps its existing implementation.
The current occurrence's planned date is `soft_due`; do not duplicate it inside
recurrence. After-completion recurrence uses the actual completion's local date.
Calendar recurrence advances past the current occurrence and completion date,
skipping missed slots. Completion creates exactly one linked next occurrence in
the same transaction and retains history. Reopen/re-complete cannot spawn a
second successor. Absolute deadlines, risk windows and monetary costs do not
silently repeat; relative risk tolerance does. Stop recurrence by clearing the
rule before completion. Deferral does not complete an occurrence or re-anchor it.

## Surfaces and reminders

Needs attention combines real deadlines, active monetary costs, due routines and
active consequence windows. Preview three with total/Show all; imminent serious
items are not hidden by that cap. Next has five independent nonurgent slots.
Today membership stays deliberate; higher sections show its badge instead of a
duplicate row. Quick is three known tasks of at most five minutes, below Next.
Done today is collapsed. Timing-sensitive is a deliberate view including future,
deferred and unknown-timing records, not an additional long default section.

Tomorrow means next local morning using the existing quiet-hours end. It changes
only resurfacing, preserves constraints, warns inline about a conflict, and can
be undone. No automatic parking after repeated snoozes. Today honors deferral.
Routine reminders honor completion and deferral, and use the same timing code
as ranking. Real imminent serious constraints remain visible. No new notification
channel or blanket permission expansion; existing quiet hours still apply.

Raw app capture is immediate. Timing not yet interpreted must be explicit in
the capture result and review UI; saving a sentence is not a scheduled reminder.
Structured timing capture/edit is available without waiting for nightly dreaming.

Candidate responses add `timing`, `must_show`, `hard_due`, `hard_due_source` and
`ready_at`. Deadline confirmation uses that field's own source, not an unrelated
agent-inferred consequence. `available` is the independent nonurgent ranking;
`timing` is the paginated deliberate timing-sensitive view. Existing `next` and
web/API fields remain compatible.

## Verification gates

Compatibility with old/null-valued tasks and Todoist; strict new input validation;
source protection; soft/hard coexistence; unknown and relative risk; owner-local
dates/DST; calendar and actual-completion recurrence; retry/reopen idempotence;
deferral/undo without deadline mutation; ranking and reminder agreement;
notification delivery rechecks; independent Next budget and capped Quick;
accessible iOS/web flows and existing completion/delete regression tests.

Railway releases cover API, worker, MCP and web. Native source/build verification
is reported separately from installation on a phone.
