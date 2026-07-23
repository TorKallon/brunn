# Avery Lin identity equivalence reversal

At 2026-07-10T09:15:00-07:00, authoritative evidence
`evidence:owner-correction-2026-07-10` established that `person:p-103` is not
the same person as `person:p-101`.

Relation revision `relation:identity-103-r2`, version `v2`, supersedes
`relation:identity-103-r1`, version `v1`. The new revision marks the prior
`same_as` equivalence `refuted`; it does not delete either person object or
choose a surviving ID.

Both stable IDs remain. Existing inbound relation
`relation:participation-103` still points to `person:p-103`; no references are
rewritten to `person:p-101`. Derived dossiers and retrieval aliases that used
the old equivalence are invalidated and rebuilt from the corrected relation
revision.

Source: `source:identity-review-2026-07-10`.
