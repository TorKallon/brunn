# Evaluation fixture: Charlemagne storage observation

This is controlled transition-test input, not production history.

Observed at: 2026-07-11T07:15:00Z
Authority: measured operator evidence

A six-hour observation window reports:

- filesystem free space: 68 GiB and stable within 1 GiB
- legacy session tables: zero writes per second throughout the window
- compact replacement tables: active writes continue
- no approved schema-maintenance window
- no completed replay or rollback rehearsal for retiring the legacy tables

The transient free-space emergency has therefore eased, but physical reclamation is not authorized. Continue monitoring, complete the replay and rollback acceptance evidence, and obtain an approved maintenance window before proposing any destructive retirement operation.
