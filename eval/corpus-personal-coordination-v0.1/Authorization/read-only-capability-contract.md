# Read-only capability contract

Credential `credential:observer-1301` belongs to `user:u-1301` and is limited
to `scope:personal-coordination`. Its allowed operations are `memory.open`,
`memory.query`, `memory.read`, `memory.compute`, and `memory.verify` against one
snapshot-pinned session.

The credential must receive stable error `capability_denied` for
`memory.checkpoint`, `memory.save`, `memory.stage`, correction, deletion, and
dream-management operations. Scope membership, source authorship, object
ownership, or a relation role does not add capabilities to the credential.

Read operations may create ephemeral session and audit records. Those records
must not change corpus revisions, staged content, canonical objects, claims,
relations, evidence, embeddings, or dream state. The required checkpoint-shaped
benchmark answer is an output proposal only; it is not persisted by this
read-only credential.

Source: `source:credential-policy-r1`. Policy: `policy:read-only-agent@v1`.
