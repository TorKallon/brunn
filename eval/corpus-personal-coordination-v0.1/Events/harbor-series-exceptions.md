# Harbor Practice sparse occurrence exceptions

Source `source:harbor-exceptions-r3` overlays, but does not rewrite,
`event-series:harbor-practice` in `America/Los_Angeles`.

- `event-occurrence:harbor-2026-08-12` is canceled. Its original time remains
  2026-08-12T17:00:00-07:00; current status is `canceled`.
- `event-occurrence:harbor-2026-08-19` is overridden. Original time is
  2026-08-19T17:00:00-07:00, current time is 2026-08-19T18:30:00-07:00, and
  actual start is 2026-08-19T18:42:00-07:00.
- `event-occurrence:harbor-special-2026-08-22` is an added occurrence with its
  own identity. Original time is `null`, current time is
  2026-08-22T10:00:00-07:00, and actual time is `null`.
- The 2026-08-05 and 2026-08-26 occurrences inherit the series unchanged.
