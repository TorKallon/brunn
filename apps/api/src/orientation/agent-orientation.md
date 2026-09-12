# Brunn agent orientation

Brunn does work in the background. Your writes are inputs to that work, not final state.

## Tasks
- Capture stores what you send. Enrichment (project, contexts, dates, cost, estimate) is
  done by agents later, sourced `agent:<id>`, and never overwrites an `owner`-sourced value.
- When the owner asks to mark finished work done, agents should use `task.update` with
  `type: complete`, `source: agent:<id>` and `completed_via: agent:<id>`, even for owner-set
  tasks. Read the current task/version first; the owner's confirmation is sufficient.
  This records completion, not permission to execute unfinished work. Check recurrence:
  completing an occurrence may create the next one; stopping the series is a separate decision.
- Quick tasks are `estimate_minutes` ≤ 5. "Flag as quick" means `estimate_minutes` with
  source `owner`. Quickness is never a context.
- `today_since` is the owner's Today list: things that came up and still need doing. It
  persists until completed or swept. Use `task.update` action `add_today` when the owner
  says "today" and `sweep` when they say it can wait. `today_pin` is different: a one-day
  priority pick, at most five.
- Contexts are places or means (home, phone, online) and use AND semantics. Do not mint a
  context for a duration, priority, or mood.
- Task settings and contexts are changed only through `task.settings` and `task.contexts`;
  there is no settings screen for them.

## Briefings
- One topic per item; more items beat merged ones. The 30-second summary is the item
  headlines, so a headline is one topic in one bold sentence. `summary_md` is not accepted.
- `why_it_matters` only when it says something specific to that item; otherwise empty.
- `detail_md` never restates a figure already in the headline or body. Keep derived or
  forward-looking lines.
- Projects section: one `tracker` item per project whose status changed overnight
  (`project.list` → `status_since` is today), escalations first. Unchanged colours are not
  reported.

## Projects
- The Dreamer judges each project's colour (grey, green, yellow, red) nightly from its
  tasks, hub note, checkpoints, and the rules written in its context. It is the only thing
  that sets `status`; never write status text or colour yourself.
- Rules are prose in the project's own context ("lodging unbooked inside 45 days is yellow,
  30 red"). A project with obligations and no rule stays grey and carries a proposed rule
  in its reason; when the owner confirms, write the rule into the project's context.
- Checkpoints with `project` set are the project's catch-up summary. Write them when work
  finishes, not as status updates.

## Dreaming
- The Dreamer researches subjects nightly and proposes derived overviews for owner review.
  In report-only mode nothing is applied; approvals are held. In full mode, an owner-configured
  `auto_apply_after_hours` policy lets concrete pending proposals publish on the first run after
  that review window. Each revision starts a fresh window; rejection, deferral, correction
  requests and unavailable evidence prevent automatic publication. Questions need an answer.
  Only change `dreams/CONTROL.md` with explicit owner authorization. An automatic publication
  follows that standing policy; never record it as an owner approval click.
