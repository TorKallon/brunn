# Avery Lin identity resolution review

Review time: 2026-07-08T10:30:00-07:00. Reviewer source:
`source:identity-review-2026-07-08`.

- `person:p-101` and `person:p-102` remain distinct stable objects. Their
  matching name and matching email created `possibly_same_as`, status
  `unreviewed`; those fields did not trigger a merge.
- `person:p-103` has reviewed relation `same_as` -> `person:p-101`, status
  `confirmed`, after explicit owner review of the two source records. Both
  stable IDs and all source claims are retained for audit.
- `possibly_same_as` is ambiguous candidate linkage. Only the reviewed
  `same_as` relation is confirmed equivalence. A name, alias, or email is
  source-bearing evidence, never a canonical identity key.
