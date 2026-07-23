Created: 2026-05-17 13:45 PDT
Updated: 2026-05-17 13:45 PDT
Status: Complete

Related: [[N24 RaceWatch]], [[INDEX|Shared knowledge index]]

# 2026 Production Wrap-up

The 2026 N24 RaceWatch public race run worked well. The durable implementation truth and next-race playbooks now live in the repo:

- `/Users/shared/projects/n24-racewatch/docs/2026-production-wrap-up.md`
- `/Users/shared/projects/n24-racewatch/docs/race-day-operations-playbook.md`
- `/Users/shared/projects/n24-racewatch/docs/post-race-shutdown-playbook.md`

Key retained artifacts:

- Live raw capture: `/Users/shared/projects/n24-racewatch/.local-data/n24-racewatch/captures/2026-05-17T00-45-16Z-race-live-window`
- Portable replay fixture: `/Users/shared/projects/n24-racewatch/testdata/live-captures/2026-05-17-max-night-stint-keyframes.json`
- Promo screenshots: `/Users/shared/projects/n24-racewatch/docs/promo/screenshots/2026-05-17T02-44-47-298Z-live-production/`

Operational notes:

- Future race-day production should use the shared repo at `/Users/shared/projects/n24-racewatch`, not the old OpenClaw checkout.
- OpenClaw N24 crons were removed after the race; do not revive them by default.
- Codex-derived insights should run through local Codex heartbeat tooling, not direct OpenAI API calls.
- Stop N24 tmux/Codex runners promptly once the race is over so post-session loops do not consume Codex runtime.
